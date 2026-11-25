//! gcr's secret exchange, protocol `[sx-aes-1]` — how a password typed into
//! the prompter reaches gnome-keyring without crossing the bus in clear text.
//!
//! Byte-compatible with `gcr/gcr-secret-exchange.c` + `egg/egg-dh*.c` +
//! `egg/egg-hkdf.c`:
//!
//! * Diffie-Hellman over the RFC 3526 1536-bit MODP group
//!   (`ietf-ike-grp-modp-1536`, generator 2). Public keys travel as minimal
//!   unsigned big-endian integers (`GCRYMPI_FMT_USG`); private keys are
//!   below p/2; the shared secret is zero-padded to the prime's byte length.
//! * HKDF-SHA256 over the shared secret, no salt (RFC 5869: a zero salt of
//!   hash length), empty info, 16 output bytes → the AES key.
//! * AES-128-CBC with a fresh random 16-byte IV and PKCS#7 padding.
//! * The exchange string is a GKeyFile: group `[sx-aes-1]`, keys `public`,
//!   and with a secret also `secret` and `iv`, values in standard base64.
//!
//! Secrets, the private key and the derived key are wiped on drop.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use num_bigint_dig::BigUint;
use zeroize::{Zeroize, Zeroizing};

/// The protocol name: the key-file group of every exchange string.
pub const PROTOCOL: &str = "sx-aes-1";
/// RFC 3526 §2, the 1536-bit MODP group's prime (a safe prime).
const MODP_1536_PRIME: &str = concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74",
    "020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F1437",
    "4FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED",
    "EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF05",
    "98DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB",
    "9ED529077096966D670C354E4ABC9804F1746C08CA237327FFFFFFFFFFFFFFFF",
);
const GENERATOR: u32 = 2;
/// The prime's length in bytes (the shared secret is padded to it).
const PRIME_BYTES: usize = 192;
/// Private keys keep this many low bits, which puts them below p/2 (egg_dh:
/// "Secret key value must be less than half of p").
const PRIVATE_BITS: usize = 1534;
/// AES-128 key and CBC IV / block length.
const KEY_LEN: usize = 16;
const IV_LEN: usize = 16;
const BLOCK: usize = 16;
/// Longest secret accepted from a peer (gnome-keyring passwords are short).
const MAX_SECRET: usize = 64 * 1024;

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExchangeError {
    /// Not a `[sx-aes-1]` key file.
    Protocol,
    /// The `public` value is missing, not base64, or out of range.
    PublicKey,
    /// `secret` / `iv` is malformed or fails to decrypt.
    Secret,
    /// The system's random source failed.
    Random,
}

impl std::fmt::Display for ExchangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ExchangeError::Protocol => "not an sx-aes-1 secret exchange",
            ExchangeError::PublicKey => "invalid or missing public key",
            ExchangeError::Secret => "invalid or undecryptable secret",
            ExchangeError::Random => "random number generator failed",
        })
    }
}

impl std::error::Error for ExchangeError {}

fn prime() -> BigUint {
    BigUint::parse_bytes(MODP_1536_PRIME.as_bytes(), 16).unwrap_or_default()
}

/// One side of an exchange: a DH key pair plus, once the peer's public key
/// arrived, the derived transport key.
pub struct SecretExchange {
    private: BigUint,
    public: Vec<u8>,
    key: Option<Zeroizing<[u8; KEY_LEN]>>,
}

impl Drop for SecretExchange {
    fn drop(&mut self) {
        self.private.zeroize();
    }
}

impl std::fmt::Debug for SecretExchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretExchange").field("derived", &self.key.is_some()).finish_non_exhaustive()
    }
}

impl SecretExchange {
    /// A fresh key pair from the OS random source.
    pub fn new() -> Result<Self, ExchangeError> {
        let mut bytes = Zeroizing::new([0u8; PRIME_BYTES]);
        getrandom::fill(bytes.as_mut()).map_err(|_| ExchangeError::Random)?;
        Ok(Self::with_private(bytes.as_ref()))
    }

    /// A key pair from fixed private bytes (big-endian; reduced below p/2).
    /// Tests use it for reproducible vectors.
    pub fn with_private(bytes: &[u8]) -> Self {
        let p = prime();
        let mut x = BigUint::from_bytes_be(bytes);
        let mask = (BigUint::from(1u32) << PRIVATE_BITS) - BigUint::from(1u32);
        x &= mask;
        // Degenerate exponents (0, 1) would leak the shared secret.
        if x < BigUint::from(2u32) {
            x += BigUint::from(2u32);
        }
        let public = BigUint::from(GENERATOR).modpow(&x, &p).to_bytes_be();
        Self { private: x, public, key: None }
    }

    /// Our public key (minimal big-endian bytes).
    pub fn public_key(&self) -> &[u8] {
        &self.public
    }

    /// Whether a peer key has been received.
    pub fn is_derived(&self) -> bool {
        self.key.is_some()
    }

    /// The opening string: just our public key.
    pub fn begin(&self) -> String {
        format!("[{PROTOCOL}]\npublic={}\n", B64.encode(&self.public))
    }

    /// Reads the peer's exchange string: derives the transport key from its
    /// public key and decrypts the secret it carries, if any.
    pub fn receive(&mut self, exchange: &str) -> Result<Option<Zeroizing<String>>, ExchangeError> {
        let fields = parse_key_file(exchange)?;
        let get = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
        let peer = get("public").and_then(|v| B64.decode(v).ok()).ok_or(ExchangeError::PublicKey)?;
        self.derive(&peer)?;
        match (get("secret"), get("iv")) {
            (Some(secret), Some(iv)) => {
                let cipher = B64.decode(secret).map_err(|_| ExchangeError::Secret)?;
                let iv = B64.decode(iv).map_err(|_| ExchangeError::Secret)?;
                self.decrypt(&cipher, &iv).map(Some)
            }
            (Some(_), None) => Err(ExchangeError::Secret),
            _ => Ok(None),
        }
    }

    /// The reply string: our public key, plus `secret` encrypted under a
    /// fresh random IV when given. Needs a received peer key to carry one.
    pub fn send(&self, secret: Option<&str>) -> Result<String, ExchangeError> {
        let mut iv = [0u8; IV_LEN];
        if secret.is_some() {
            getrandom::fill(&mut iv).map_err(|_| ExchangeError::Random)?;
        }
        self.send_with_iv(secret, &iv)
    }

    /// [`SecretExchange::send`] with a chosen IV (reproducible tests).
    pub fn send_with_iv(&self, secret: Option<&str>, iv: &[u8; IV_LEN]) -> Result<String, ExchangeError> {
        let mut out = format!("[{PROTOCOL}]\npublic={}\n", B64.encode(&self.public));
        if let Some(secret) = secret {
            let key = self.key.as_ref().ok_or(ExchangeError::PublicKey)?;
            let cipher = encrypt(key, iv, secret.as_bytes());
            out.push_str(&format!("secret={}\niv={}\n", B64.encode(&cipher), B64.encode(iv)));
        }
        Ok(out)
    }

    fn derive(&mut self, peer: &[u8]) -> Result<(), ExchangeError> {
        let p = prime();
        let y = BigUint::from_bytes_be(peer);
        // Reject 0, 1, p-1 and anything ≥ p (small-subgroup / invalid keys).
        if y < BigUint::from(2u32) || y > &p - BigUint::from(2u32) {
            return Err(ExchangeError::PublicKey);
        }
        let mut shared = y.modpow(&self.private, &p);
        let mut ikm = Zeroizing::new([0u8; PRIME_BYTES]);
        let mut raw = Zeroizing::new(shared.to_bytes_be());
        shared.zeroize();
        ikm[PRIME_BYTES - raw.len()..].copy_from_slice(&raw);
        raw.zeroize();
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        hkdf::Hkdf::<sha2::Sha256>::new(None, ikm.as_ref()).expand(&[], key.as_mut()).map_err(|_| ExchangeError::Secret)?;
        self.key = Some(key);
        Ok(())
    }

    fn decrypt(&self, cipher: &[u8], iv: &[u8]) -> Result<Zeroizing<String>, ExchangeError> {
        use cbc::cipher::{BlockDecryptMut as _, KeyIvInit as _, block_padding::Pkcs7};
        let key = self.key.as_ref().ok_or(ExchangeError::PublicKey)?;
        if iv.len() != IV_LEN || cipher.is_empty() || !cipher.len().is_multiple_of(BLOCK) || cipher.len() > MAX_SECRET {
            return Err(ExchangeError::Secret);
        }
        let mut buf = Zeroizing::new(cipher.to_vec());
        let dec = Aes128CbcDec::new_from_slices(key.as_ref(), iv).map_err(|_| ExchangeError::Secret)?;
        let plain = dec.decrypt_padded_mut::<Pkcs7>(&mut buf).map_err(|_| ExchangeError::Secret)?;
        let text = std::str::from_utf8(plain).map_err(|_| ExchangeError::Secret)?;
        Ok(Zeroizing::new(text.to_string()))
    }
}

fn encrypt(key: &[u8; KEY_LEN], iv: &[u8; IV_LEN], plain: &[u8]) -> Vec<u8> {
    use cbc::cipher::{BlockEncryptMut as _, KeyIvInit as _, block_padding::Pkcs7};
    let pad = BLOCK - plain.len() % BLOCK;
    let mut buf = Zeroizing::new(vec![0u8; plain.len() + pad]);
    buf[..plain.len()].copy_from_slice(plain);
    let enc = Aes128CbcEnc::new(key.into(), iv.into());
    // The buffer has room for a full padding block, so this cannot fail.
    match enc.encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len()) {
        Ok(cipher) => cipher.to_vec(),
        Err(_) => Vec::new(),
    }
}

/// Parses the `[sx-aes-1]` group of a GKeyFile: `key=value` lines, `#`
/// comments and blank lines skipped, whitespace around keys and values
/// trimmed. Other groups are ignored; a missing group is an error.
fn parse_key_file(text: &str) -> Result<Vec<(String, String)>, ExchangeError> {
    let header = format!("[{PROTOCOL}]");
    let mut in_group = false;
    let mut seen = false;
    let mut out = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_group = line == header;
            seen |= in_group;
            continue;
        }
        if in_group && let Some((k, v)) = line.split_once('=') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    if seen { Ok(out) } else { Err(ExchangeError::Protocol) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vectors from an independent Python implementation of the algorithm
    // above (gcr's: RFC 3526 group, zero-padded shared secret, HKDF-SHA256
    // with no salt/info, AES-128-CBC + PKCS#7) using the `cryptography`
    // package for AES. Private keys: SHA-512("mandara-prompter") and
    // SHA-512("gnome-keyring") repeated ×3, masked to 1534 bits.
    const PRIV_A: &str = "07e98d23b4e33473791668e1161cdb26c3646f4bd6d307c0e3ec1e44a3e2af9f94d9682087d4085d4f9dbf77de40c70f17d474d7f229bb7808679e57107fe96c07e98d23b4e33473791668e1161cdb26c3646f4bd6d307c0e3ec1e44a3e2af9f94d9682087d4085d4f9dbf77de40c70f17d474d7f229bb7808679e57107fe96c07e98d23b4e33473791668e1161cdb26c3646f4bd6d307c0e3ec1e44a3e2af9f94d9682087d4085d4f9dbf77de40c70f17d474d7f229bb7808679e57107fe96c";
    const PRIV_B: &str = "2b13e26cadc65252d018aa3c31eaedfe2eed9729a0cdf46ad216613caaba8ceec8d3fd12f29c9e5ef23ad1eec9ca8694773fbf8f5791b7471ce27c34e19f3101ab13e26cadc65252d018aa3c31eaedfe2eed9729a0cdf46ad216613caaba8ceec8d3fd12f29c9e5ef23ad1eec9ca8694773fbf8f5791b7471ce27c34e19f3101ab13e26cadc65252d018aa3c31eaedfe2eed9729a0cdf46ad216613caaba8ceec8d3fd12f29c9e5ef23ad1eec9ca8694773fbf8f5791b7471ce27c34e19f3101";
    const PUB_A: &str = "Lo6TQahz0tXfLP5nenJjjzEW0QNU+JNVP/PRXFMH+U0NHqDFObRW9/jvD7qGE43Ib8W+0xdMwr3PULqSSDiNxVdfH6oM8rgq6HTCt8hlo6TdMEqHe0u9TfE8ZRSF0UpVAW/SMgwqje+f2grrVeuZT8IBGa+OsFklSNoMG+xg1dZh/bC72yf3oG3XkFJzZJK763XZ5Of8lVk4Vk6DivZ4YNFlaoG4chMe8TKcW48oWbo4hN+wsoARa+g9mrhQN1Ph";
    const PUB_B: &str = "oHE8lrceoaUfRo9OfS06tVfsoJmjU85aiC//B/zfXz5WmWmo8xFp+9OlNNp69zyT8HwJ3Rv8nSkNm4iIR+Gu5GFQzslu81FNynz7pBR2BOLs1JEbr7Qv3sdboFaEYi9SmM7yee1bePMyvzLsG/0KX5G7EfjhJcZi9KVEkCXLtYOu62tLSMpmMzVQDPcuFx5CBgJibx1pMNbv58qkSGDjKQpewrxOrPqMq6MtsaIw9oFsrK7cckLUEnLtCgMRwhBd";
    const KEY: &str = "fc211e97c650f54775e241ac5167bffa";
    const IV: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
    const SECRET_B64: &str = "5VJKN+46XNiTIYHArr+Nx0PNE9BY3tPZB+zjdO2J0aw=";
    const PLAIN: &str = "pässwörd كلمة";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn matches_the_reference_vectors() {
        let mut a = SecretExchange::with_private(&hex(PRIV_A));
        let mut b = SecretExchange::with_private(&hex(PRIV_B));
        assert_eq!(B64.encode(a.public_key()), PUB_A);
        assert_eq!(B64.encode(b.public_key()), PUB_B);
        assert_eq!(a.receive(&b.begin()).unwrap(), None);
        assert_eq!(a.key.as_deref().map(|k| k.to_vec()), Some(hex(KEY)));
        let sent = a.send_with_iv(Some(PLAIN), &IV).unwrap();
        assert_eq!(sent, format!("[sx-aes-1]\npublic={PUB_A}\nsecret={SECRET_B64}\niv=AAECAwQFBgcICQoLDA0ODw==\n"));
        assert_eq!(b.receive(&sent).unwrap().as_deref().map(String::as_str), Some(PLAIN));
    }

    #[test]
    fn random_pairs_round_trip_both_ways() {
        let mut prompter = SecretExchange::new().unwrap();
        let mut caller = SecretExchange::new().unwrap();
        assert_ne!(prompter.public_key(), caller.public_key());
        // The protocol's order: prompter begins, caller answers, prompter
        // replies with the secret.
        assert_eq!(caller.receive(&prompter.begin()).unwrap(), None);
        assert_eq!(prompter.receive(&caller.send(None).unwrap()).unwrap(), None);
        for secret in ["", "a", "exactly-16-bytes", "x".repeat(300).as_str()] {
            let reply = prompter.send(Some(secret)).unwrap();
            assert_eq!(caller.receive(&reply).unwrap().as_deref().map(String::as_str), Some(secret));
        }
    }

    #[test]
    fn accepts_gkeyfile_variations_and_rejects_garbage() {
        let b = SecretExchange::with_private(&hex(PRIV_B));
        let mut a = SecretExchange::with_private(&hex(PRIV_A));
        let loose = format!("# from gcr\n[other]\npublic=AAAA\n\n[sx-aes-1]\npublic = {PUB_B}  \n");
        assert_eq!(a.receive(&loose).unwrap(), None);
        assert_eq!(a.key.as_deref().map(|k| k.to_vec()), Some(hex(KEY)));
        assert_eq!(a.receive("[sx-aes-2]\npublic=AAAA\n"), Err(ExchangeError::Protocol));
        assert_eq!(a.receive("[sx-aes-1]\n"), Err(ExchangeError::PublicKey));
        assert_eq!(a.receive("[sx-aes-1]\npublic=***\n"), Err(ExchangeError::PublicKey));
        // Degenerate public keys: 1 and p-1.
        assert_eq!(a.receive("[sx-aes-1]\npublic=AQ==\n"), Err(ExchangeError::PublicKey));
        let p_minus_1 = (prime() - BigUint::from(1u32)).to_bytes_be();
        assert_eq!(a.receive(&format!("[sx-aes-1]\npublic={}\n", B64.encode(p_minus_1))), Err(ExchangeError::PublicKey));
        // A tampered ciphertext fails padding (or UTF-8), never panics.
        let good = b.begin();
        a.receive(&good).unwrap();
        let bad = format!("{good}secret=AAAAAAAAAAAAAAAAAAAAAA==\niv=AAECAwQFBgcICQoLDA0ODw==\n");
        assert!(SecretExchange::with_private(&hex(PRIV_A)).receive(&bad).is_err());
        let short_iv = format!("{good}secret={SECRET_B64}\niv=AAEC\n");
        assert_eq!(SecretExchange::with_private(&hex(PRIV_A)).receive(&short_iv), Err(ExchangeError::Secret));
    }

    #[test]
    fn a_secret_needs_a_peer_key() {
        let a = SecretExchange::new().unwrap();
        assert!(!a.is_derived());
        assert_eq!(a.send(Some("pw")), Err(ExchangeError::PublicKey));
        assert!(a.send(None).unwrap().starts_with("[sx-aes-1]\npublic="));
    }
}
