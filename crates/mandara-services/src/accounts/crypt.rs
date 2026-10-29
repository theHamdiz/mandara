//! SHA-512 crypt (`$6$`, Drepper's SHA-crypt as in glibc `crypt(3)`) in
//! pure Rust: AccountsService's `SetPassword` takes a ready-made hash, so
//! the plain password never leaves this process.

use std::io::Read as _;

const PREFIX: &str = "$6$";
const ROUNDS_PREFIX: &str = "rounds=";
/// glibc's implicit round count (written without a `rounds=` field).
const ROUNDS_IMPLICIT: usize = 5_000;
/// Rounds for new hashes: ~20× glibc's default, still a fraction of a
/// second at login.
pub const HASH_ROUNDS: usize = 100_000;
/// Salt length (the format's maximum).
pub const SALT_LEN: usize = 16;
/// crypt(3)'s base-64 alphabet.
const SALT_ALPHABET: &[u8; 64] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn random_salt() -> Result<String, String> {
    let mut bytes = [0u8; SALT_LEN];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).map_err(|e| format!("reading /dev/urandom: {e}"))?;
    // 64 divides 256, so `% 64` is unbiased.
    Ok(bytes.iter().map(|b| SALT_ALPHABET[usize::from(*b) % SALT_ALPHABET.len()] as char).collect())
}

/// `$6$[rounds=N$]salt$hash` for `password` with `salt` and `rounds`.
pub fn hash_with(password: &str, salt: &str, rounds: usize) -> Result<String, String> {
    let params = sha_crypt::Sha512Params::new(rounds).map_err(|e| format!("{e:?}"))?;
    let hash = sha_crypt::sha512_crypt_b64(password.as_bytes(), salt.as_bytes(), &params).map_err(|e| format!("{e:?}"))?;
    Ok(if rounds == ROUNDS_IMPLICIT { format!("{PREFIX}{salt}${hash}") } else { format!("{PREFIX}{ROUNDS_PREFIX}{rounds}${salt}${hash}") })
}

/// A new SHA-512 crypt hash with a random salt.
pub fn hash_password(password: &str) -> Result<String, String> {
    hash_with(password, &random_salt()?, HASH_ROUNDS)
}

/// Checks `password` against a `$6$` hash (constant-time comparison).
pub fn verify(password: &str, hashed: &str) -> bool {
    let Some(rest) = hashed.strip_prefix(PREFIX) else { return false };
    let (rounds, rest) = match rest.strip_prefix(ROUNDS_PREFIX).and_then(|r| r.split_once('$')) {
        Some((n, rest)) => match n.parse() {
            Ok(n) => (n, rest),
            Err(_) => return false,
        },
        None => (ROUNDS_IMPLICIT, rest),
    };
    let Some((salt, _)) = rest.split_once('$') else { return false };
    let Ok(expected) = hash_with(password, salt, rounds) else { return false };
    expected.len() == hashed.len() && expected.bytes().zip(hashed.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test vectors from Drepper's SHA-crypt specification.
    #[test]
    fn matches_reference_vectors() {
        assert_eq!(
            hash_with("Hello world!", "saltstring", ROUNDS_IMPLICIT).unwrap(),
            "$6$saltstring$svn8UoSVapNtMuq1ukKS4tPQd8iKwSMHWjl/O817G3uBnIFNjnQJuesI68u4OTLiBFdcbYEdFCoEOfaS35inz1"
        );
        assert_eq!(
            hash_with("Hello world!", "saltstringsaltst", 10_000).unwrap(),
            "$6$rounds=10000$saltstringsaltst$OW1/O6BYHV6BcXZu8QVeXbDWra3Oeqh0sbHbbMCVNSnCM/UrjmM0Dp8vOuZeHBy/YTBmSK6H9qs/y3RnOaw5v."
        );
    }

    #[test]
    fn new_hashes_verify_and_differ() {
        let a = hash_password("ya salam").unwrap();
        let b = hash_password("ya salam").unwrap();
        assert_ne!(a, b, "random salt");
        assert!(a.starts_with("$6$rounds=100000$"));
        assert!(verify("ya salam", &a));
        assert!(!verify("ya salam!", &a));
        assert!(!verify("x", "$1$md5$nope"));
        let salt = a.split('$').nth(3).unwrap();
        assert_eq!(salt.len(), SALT_LEN);
        assert!(salt.bytes().all(|c| SALT_ALPHABET.contains(&c)));
    }
}
