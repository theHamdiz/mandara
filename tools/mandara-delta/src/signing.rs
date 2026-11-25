//! Release signatures: every Mandara release after [`LAST_UNSIGNED`] signs
//! its `SHA256SUMS` with the release key (Ed25519), as `SHA256SUMS.sig`.
//! The listing covers every asset, `update.json` included, so one checked
//! signature vouches for the whole release: someone who can only replace
//! release files (not sign) can't get a download installed.
//!
//! The public keys the shell trusts are compiled in from
//! `release-keys.txt` (one hex key per line, `#` comments); a new key is
//! added there before the old one is retired, so updates keep working
//! across a key change. The private key lives only in the release
//! workflow's secrets (and the owner's backup); `cargo xtask release-key`
//! creates it without ever printing it.

use ring::signature::{ED25519, UnparsedPublicKey};

/// The signature published next to `SHA256SUMS`.
pub const SIG_FILE: &str = "SHA256SUMS.sig";
/// The last release published without a signature; every later one must
/// carry a valid [`SIG_FILE`].
pub const LAST_UNSIGNED: &str = "0.3.3";
/// Length of an Ed25519 public key.
pub const PUBLIC_KEY_LEN: usize = 32;
const SIGNATURE_LEN: usize = 64;
/// The trusted public keys, as shipped.
const RELEASE_KEYS: &str = include_str!("../release-keys.txt");

/// An Ed25519 public key.
pub type PublicKey = [u8; PUBLIC_KEY_LEN];

/// The release keys this build trusts.
pub fn trusted_keys() -> Vec<PublicKey> {
    parse_keys(RELEASE_KEYS)
}

/// Keys from a `release-keys.txt` listing; malformed lines are skipped.
pub fn parse_keys(listing: &str) -> Vec<PublicKey> {
    listing.lines().map(|l| l.split('#').next().unwrap_or_default().trim()).filter_map(|l| unhex(l)?.try_into().ok()).collect()
}

/// Whether `version`'s release must be signed (it's newer than
/// [`LAST_UNSIGNED`]). Unparseable versions must be.
pub fn must_be_signed(version: &str) -> bool {
    match (numbers(version), numbers(LAST_UNSIGNED)) {
        (Some(v), Some(last)) => v > last,
        _ => true,
    }
}

fn numbers(version: &str) -> Option<Vec<u64>> {
    let core = version.trim().trim_start_matches('v').split(['-', '+']).next()?;
    core.split('.').map(|n| n.parse().ok()).collect()
}

/// Checks `sig` (the text of a [`SIG_FILE`]) over `data` against `keys`.
pub fn verify_with(keys: &[PublicKey], data: &[u8], sig: &[u8]) -> Result<(), String> {
    if keys.is_empty() {
        return Err("this build trusts no release key".into());
    }
    let sig = std::str::from_utf8(sig).ok().and_then(|s| unhex(s.trim())).filter(|s| s.len() == SIGNATURE_LEN).ok_or_else(|| format!("{SIG_FILE} is malformed"))?;
    keys.iter().any(|k| UnparsedPublicKey::new(&ED25519, k).verify(data, &sig).is_ok()).then_some(()).ok_or_else(|| format!("{SIG_FILE} doesn't match Mandara's release key"))
}

/// [`verify_with`] the shipped [`trusted_keys`].
pub fn verify(data: &[u8], sig: &[u8]) -> Result<(), String> {
    verify_with(&trusted_keys(), data, sig)
}

/// A release's `SHA256SUMS`, accepted only when its signature checks out,
/// or when `version` predates signing and no signature was published.
/// `sig`: the [`SIG_FILE`], when the release has one.
pub fn checked_sums(keys: &[PublicKey], version: &str, sums: &[u8], sig: Option<&[u8]>) -> Result<String, String> {
    match sig {
        Some(sig) => verify_with(keys, sums, sig)?,
        None if must_be_signed(version) => return Err(format!("v{} isn't signed ({SIG_FILE} is missing)", version.trim_start_matches('v'))),
        None => {}
    }
    Ok(String::from_utf8_lossy(sums).into_owned())
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Key creation and signing (release tooling only, never in the shell).
#[cfg(feature = "sign")]
pub mod sign {
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair as _};

    /// A new key pair: (private key as hex PKCS#8, public key as hex).
    pub fn generate() -> Result<(String, String), String> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| "couldn't create a key")?;
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| "couldn't read the new key")?;
        Ok((crate::hex(pkcs8.as_ref()), crate::hex(pair.public_key().as_ref())))
    }

    /// The public key of a hex PKCS#8 private key.
    pub fn public_key(private_hex: &str) -> Result<super::PublicKey, String> {
        let pair = pair(private_hex)?;
        pair.public_key().as_ref().try_into().map_err(|_| "unexpected public key length".into())
    }

    /// Signs `data`; the text of a [`super::SIG_FILE`].
    pub fn sign(private_hex: &str, data: &[u8]) -> Result<String, String> {
        Ok(crate::hex(pair(private_hex)?.sign(data).as_ref()) + "\n")
    }

    fn pair(private_hex: &str) -> Result<Ed25519KeyPair, String> {
        let bytes = super::unhex(private_hex.trim()).ok_or("the release key isn't hex")?;
        Ed25519KeyPair::from_pkcs8(&bytes).map_err(|_| "the release key isn't an Ed25519 PKCS#8 key".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_after_the_last_unsigned_release_must_be_signed() {
        assert!(!must_be_signed("0.3.3") && !must_be_signed("v0.1.0") && !must_be_signed("0.3.2"));
        assert!(must_be_signed("0.3.4") && must_be_signed("v0.4.0") && must_be_signed("1.0.0") && must_be_signed("0.3.4-rc.1"));
        assert!(must_be_signed("garbage"));
    }

    #[test]
    fn key_listings() {
        let k = "ab".repeat(32);
        let keys = parse_keys(&format!("# the release key\n{k}  # 2026\n\nnot-hex\n{}\n", "ab".repeat(31)));
        assert_eq!(keys, vec![[0xab; 32]]);
    }

    #[test]
    fn the_shipped_listing_has_a_release_key() {
        assert!(!trusted_keys().is_empty(), "release-keys.txt has no key: run `cargo xtask release-key new`");
    }

    #[cfg(feature = "sign")]
    #[test]
    fn signatures_check_out_only_for_the_signed_bytes_and_a_trusted_key() {
        let (private, public) = sign::generate().unwrap();
        let key = sign::public_key(&private).unwrap();
        assert_eq!(crate::hex(&key), public);
        let sums = b"abc  mandara-0.4.0-x86_64-linux-gnu.tar.gz\n";
        let sig = sign::sign(&private, sums).unwrap();
        assert!(verify_with(&[key], sums, sig.as_bytes()).is_ok());
        assert!(verify_with(&[key], b"tampered", sig.as_bytes()).unwrap_err().contains("doesn't match"));
        let (other, _) = sign::generate().unwrap();
        assert!(verify_with(&[sign::public_key(&other).unwrap()], sums, sig.as_bytes()).is_err(), "another key");
        assert!(verify_with(&[], sums, sig.as_bytes()).is_err());
        assert!(verify_with(&[key], sums, b"zz").unwrap_err().contains("malformed"));
        assert_eq!(checked_sums(&[key], "0.4.0", sums, Some(sig.as_bytes())).unwrap().as_bytes(), sums);
        assert!(checked_sums(&[key], "0.4.0", sums, None).unwrap_err().contains("isn't signed"));
        assert!(checked_sums(&[key], "0.3.3", sums, None).is_ok(), "released before signing");
        assert!(checked_sums(&[key], "0.3.3", b"tampered", Some(sig.as_bytes())).is_err(), "a signature, when published, always counts");
    }
}
