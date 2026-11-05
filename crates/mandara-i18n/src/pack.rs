//! Language packs (`.mlang`): one language's translations in a file that is
//! memory-mapped and read in place.
//!
//! A pack is built once from `locales/<code>.toml` ([`build`]) and loaded
//! with a single validation pass ([`Pack::parse`]). After that, a lookup
//! is a hash, usually one index slot and one key comparison, and it hands
//! out `&'static str` slices of the mapping: no parsing, no allocation per
//! string.
//!
//! # Format (version [`FORMAT`], little-endian throughout)
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 4 | magic `MLNG` |
//! | 4 | 2 | format version ([`FORMAT`]) |
//! | 6 | 2 | reserved, 0 |
//! | 8 | 4 | entry count `n` |
//! | 12 | 4 | file length (a truncated file is rejected) |
//! | 16 | 2 | length of the Mandara version string |
//! | 18 | 2 | length of the language code |
//! | 20 | 4 | slot count `s` (`s ≥ m`, `s ≥ n`) |
//! | 24 | 4 | offset of the index |
//! | 28 | 4 | offset of the string blob |
//! | 32 | 4 | length of the string blob |
//! | 36 | 4 | home slot count `m` (`m ≥ 1`) |
//! | 40 | 8 | reserved, 0 |
//! | 48 | … | the Mandara version (`0.10.2`), then the language code (`ar`), UTF-8 |
//!
//! - **Index** (8-byte aligned): `s` slots of 24 bytes, `key_hash: u64,
//!   key_off: u32, key_len: u32, val_off: u32, val_len: u32` (offsets
//!   relative to the blob); an empty slot is all zeros. It is a hash table
//!   **sorted by hash**: an entry's home slot is `(hash × m) >> 64` (the
//!   high half of the 128-bit product, so homes grow with the hash).
//!   Entries are placed in increasing hash order, each in its home slot or,
//!   when that's taken, the next one after the previous entry. So the
//!   non-empty slots read in strictly increasing hash order, and an entry
//!   sits at or after its home with no empty slot in between. `build`
//!   makes `m` twice the entry count, so a lookup usually reads one slot.
//! - **Blob**: every English key (in index order), then every translation,
//!   UTF-8, unaligned. Keys sit together so lookups touch few cache lines.
//!
//! A lookup hashes the key, starts at its home slot and walks forward until
//! it meets the hash (then compares the stored key), a bigger hash or an
//! empty slot (not there).
//!
//! The hash ([`hash`]) is a folded-multiply word hash: with
//! `mum(a, b) = low64(a × b) ^ high64(a × b)` (the 128-bit product),
//! `P0 = 0xa0761d6478bd642f` and `P1 = 0xe7037ed1a0b428db`, start with
//! `h = len(key) ^ P0`, then for each 8-byte little-endian word `w` of the
//! key's UTF-8 bytes (the last one zero-padded) `h = mum(h ^ w, P1)`, and
//! finish with `h = mum(h, P0)`. It needs no dependency, reads eight bytes
//! per step and mixes every input bit into the high bits the index uses. It
//! is part of the format and never changes within a format version.
//! Building fails if two keys of one catalog share a hash, and a lookup
//! always compares the stored key with the one asked for, so a hash
//! collision can never return the wrong string.
//!
//! [`Pack::parse`] checks everything once: magic, format, lengths and
//! offsets, the version and language the caller expects, the index (order,
//! placement, empty slots), that every string lies inside the blob and is
//! valid UTF-8, and that every stored key hashes to its entry. Anything else
//! is an error, never a panic.

use std::fmt;

use crate::Lang;

/// The file's first four bytes.
pub const MAGIC: [u8; 4] = *b"MLNG";
/// Format version written by [`build`] and the only one [`Pack::parse`] reads.
pub const FORMAT: u16 = 1;
/// Extension of a pack file (`ar.mlang`).
pub const EXTENSION: &str = "mlang";
/// Size of the fixed header.
const HEADER_LEN: usize = 48;
/// Size of one index slot.
const ENTRY_LEN: usize = 24;
/// The hash's constants (wyhash's first two primes).
const P0: u64 = 0xa076_1d64_78bd_642f;
const P1: u64 = 0xe703_7ed1_a0b4_28db;

/// The 128-bit product of `a` and `b`, folded to 64 bits.
#[inline]
fn mum(a: u64, b: u64) -> u64 {
    let r = u128::from(a) * u128::from(b);
    (r as u64) ^ ((r >> 64) as u64)
}

/// The pack's index hash of `key` (defined in the module docs).
#[inline]
pub fn hash(key: &str) -> u64 {
    let bytes = key.as_bytes();
    let mut h = (bytes.len() as u64) ^ P0;
    let (words, tail) = bytes.as_chunks::<8>();
    for w in words {
        h = mum(h ^ u64::from_le_bytes(*w), P1);
    }
    if !tail.is_empty() {
        let mut x = [0u8; 8];
        x[..tail.len()].copy_from_slice(tail);
        h = mum(h ^ u64::from_le_bytes(x), P1);
    }
    mum(h, P0)
}

/// Hash two adjacent key slices without allocating a joined string.
fn hash_prefixed(prefix: &str, key: &str) -> u64 {
    let mut h = ((prefix.len() + key.len()) as u64) ^ P0;
    let mut word = [0u8; 8];
    let mut used = 0;
    for byte in prefix.bytes().chain(key.bytes()) {
        word[used] = byte;
        used += 1;
        if used == word.len() {
            h = mum(h ^ u64::from_le_bytes(word), P1);
            word = [0; 8];
            used = 0;
        }
    }
    if used != 0 { h = mum(h ^ u64::from_le_bytes(word), P1); }
    mum(h, P0)
}

/// Why a pack was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackError {
    /// Not a pack at all (wrong magic or too short).
    NotAPack,
    /// A format version this build can't read.
    Format(u16),
    /// Built for another Mandara version.
    Version { found: String, expected: String },
    /// Built for another language.
    Language { found: String, expected: String },
    /// Truncated, inconsistent or not UTF-8.
    Corrupt(&'static str),
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackError::NotAPack => f.write_str("not a Mandara language pack"),
            PackError::Format(v) => write!(f, "language pack format {v} isn't supported (this build reads {FORMAT})"),
            PackError::Version { found, expected } => write!(f, "the language pack is for Mandara {found}, not {expected}"),
            PackError::Language { found, expected } => write!(f, "the language pack is for {found:?}, not {expected:?}"),
            PackError::Corrupt(what) => write!(f, "the language pack is damaged ({what})"),
        }
    }
}

impl std::error::Error for PackError {}

/// Home slots per entry [`build`] uses: a half-full table, so most lookups
/// read their home slot only (measured: a fuller table's extra probes cost
/// more than its smaller size saves).
const SLOTS_PER_ENTRY: usize = 2;

/// The home slot count [`build`] picks for `n` entries (at least 1).
fn home_slots_for(n: usize) -> usize {
    n.saturating_mul(SLOTS_PER_ENTRY).max(1)
}

/// The home slot of `hash` among `m` (monotonic in the hash).
#[inline]
fn home_of(hash: u64, m: usize) -> usize {
    ((u128::from(hash) * m as u128) >> 64) as usize
}

fn align(n: usize, to: usize) -> usize {
    n.div_ceil(to) * to
}

/// Builds a pack for `lang` and Mandara `version` from `(english, translation)`
/// pairs. Fails on an empty key or translation, on two keys with the same
/// hash (including a key listed twice), and when the pack would exceed the
/// format's 4 GiB offsets.
pub fn build<'a>(lang: Lang, version: &str, entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<Vec<u8>, String> {
    let code = lang.code();
    let too_long = |what: &str| format!("{what} is too long for a language pack");
    let version_len = u16::try_from(version.len()).map_err(|_| too_long("the version"))?;
    let lang_len = u16::try_from(code.len()).map_err(|_| too_long("the language code"))?;
    let mut rows: Vec<(u64, &str, &str)> = entries.into_iter().map(|(k, v)| (hash(k), k, v)).collect();
    if let Some((_, k, _)) = rows.iter().find(|(_, k, v)| k.is_empty() || v.is_empty()) {
        return Err(format!("empty key or translation ({k:?})"));
    }
    rows.sort_unstable_by_key(|r| r.0);
    if let Some(w) = rows.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(if w[0].1 == w[1].1 { format!("{:?} is listed twice", w[0].1) } else { format!("{:?} and {:?} share the hash {:016x}; rename one", w[0].1, w[1].1, w[0].0) });
    }
    let n = rows.len();
    let m = home_slots_for(n);
    // Each entry in its home slot, or the first free one after the previous.
    let mut placed = Vec::with_capacity(n);
    let mut next = 0usize;
    for (h, _, _) in &rows {
        let at = home_of(*h, m).max(next);
        placed.push(at);
        next = at + 1;
    }
    let slots = m.max(next);
    let meta_end = HEADER_LEN + version.len() + code.len();
    let index_off = align(meta_end, 8);
    let blob_off = index_off + slots * ENTRY_LEN;
    let blob_len: usize = rows.iter().map(|(_, k, v)| k.len() + v.len()).sum();
    let total = blob_off + blob_len;
    let u32_of = |v: usize| u32::try_from(v).map_err(|_| too_long("the catalog"));
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&u32_of(n)?.to_le_bytes());
    out.extend_from_slice(&u32_of(total)?.to_le_bytes());
    out.extend_from_slice(&version_len.to_le_bytes());
    out.extend_from_slice(&lang_len.to_le_bytes());
    for v in [slots, index_off, blob_off, blob_len, m] {
        out.extend_from_slice(&u32_of(v)?.to_le_bytes());
    }
    out.resize(HEADER_LEN, 0);
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(code.as_bytes());
    out.resize(blob_off, 0);
    // Keys first, then translations: a lookup compares keys only, so they
    // share as few cache lines as possible.
    let (mut key_at, mut val_at) = (0usize, rows.iter().map(|(_, k, _)| k.len()).sum::<usize>());
    for ((h, k, v), slot) in rows.iter().zip(&placed) {
        let e = index_off + slot * ENTRY_LEN;
        out[e..e + 8].copy_from_slice(&h.to_le_bytes());
        for (i, x) in [key_at, k.len(), val_at, v.len()].into_iter().enumerate() {
            out[e + 8 + i * 4..e + 12 + i * 4].copy_from_slice(&u32_of(x)?.to_le_bytes());
        }
        key_at += k.len();
        val_at += v.len();
    }
    for (_, k, _) in &rows {
        out.extend_from_slice(k.as_bytes());
    }
    for (_, _, v) in &rows {
        out.extend_from_slice(v.as_bytes());
    }
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

#[inline]
fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

#[inline]
fn u32_at(b: &[u8], at: usize) -> u32 {
    let mut x = [0u8; 4];
    x.copy_from_slice(&b[at..at + 4]);
    u32::from_le_bytes(x)
}

#[inline]
fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut x = [0u8; 8];
    x.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(x)
}

/// A validated pack over borrowed bytes (in the shell, a leaked memory map,
/// or a leaked buffer for packs built in memory).
#[derive(Clone, Copy, Debug)]
pub struct Pack<'a> {
    lang: Lang,
    version: &'a str,
    home_slots: usize,
    count: usize,
    index: &'a [u8],
    blob: &'a [u8],
}

impl<'a> Pack<'a> {
    /// Validates `data` as `lang`'s pack for Mandara `version` (see the
    /// module docs for every check). One pass over the file.
    pub fn parse(data: &'a [u8], version: &str, lang: Lang) -> Result<Pack<'a>, PackError> {
        let corrupt = PackError::Corrupt;
        if data.len() < HEADER_LEN || data[..4] != MAGIC {
            return Err(PackError::NotAPack);
        }
        let format = u16_at(data, 4);
        if format != FORMAT {
            return Err(PackError::Format(format));
        }
        let count = u32_at(data, 8) as usize;
        if u32_at(data, 12) as usize != data.len() {
            return Err(corrupt("its length doesn't match the header; the file is truncated or padded"));
        }
        let (version_len, lang_len) = (u16_at(data, 16) as usize, u16_at(data, 18) as usize);
        let [slots, index_off, blob_off, blob_len, home_slots] = [20, 24, 28, 32, 36].map(|at| u32_at(data, at) as usize);
        let meta_end = HEADER_LEN + version_len + lang_len;
        let index_len = slots.checked_mul(ENTRY_LEN).ok_or(corrupt("slot count"))?;
        // The sections follow each other in order and end at the file's end.
        let in_order = home_slots >= 1 && slots >= home_slots && slots >= count && meta_end <= index_off && index_off % 8 == 0 && index_off.checked_add(index_len).is_some_and(|end| end <= blob_off) && blob_off.checked_add(blob_len) == Some(data.len());
        if !in_order {
            return Err(corrupt("section offsets"));
        }
        let found_version = std::str::from_utf8(&data[HEADER_LEN..HEADER_LEN + version_len]).map_err(|_| corrupt("version"))?;
        let found_lang = std::str::from_utf8(&data[HEADER_LEN + version_len..meta_end]).map_err(|_| corrupt("language code"))?;
        if found_version != version {
            return Err(PackError::Version { found: found_version.to_string(), expected: version.to_string() });
        }
        if found_lang != lang.code() {
            return Err(PackError::Language { found: found_lang.to_string(), expected: lang.code().to_string() });
        }
        let pack = Pack { lang, version: found_version, home_slots, count, index: &data[index_off..index_off + index_len], blob: &data[blob_off..] };
        pack.validate()?;
        Ok(pack)
    }

    /// The index and every string (see [`Pack::parse`]).
    fn validate(&self) -> Result<(), PackError> {
        let corrupt = PackError::Corrupt;
        let mut previous: Option<u64> = None;
        let mut run_start = 0usize;
        let mut entries = 0usize;
        for slot in 0..self.index.len() / ENTRY_LEN {
            let e = slot * ENTRY_LEN;
            if self.index[e..e + ENTRY_LEN].iter().all(|b| *b == 0) {
                run_start = slot + 1;
                continue;
            }
            let h = u64_at(self.index, e);
            if previous.is_some_and(|p| p >= h) {
                return Err(corrupt("index order"));
            }
            previous = Some(h);
            // A lookup walks from the home slot and stops at an empty one:
            // the entry must sit at or after its home, in the same run.
            let home = home_of(h, self.home_slots);
            if home > slot || home < run_start {
                return Err(corrupt("index placement"));
            }
            let key = self.str_at(e + 8).filter(|k| !k.is_empty()).ok_or(corrupt("a key is empty, isn't valid UTF-8 or lies outside the strings"))?;
            self.str_at(e + 16).ok_or(corrupt("a translation isn't valid UTF-8 or lies outside the strings"))?;
            if hash(key) != h {
                return Err(corrupt("a key doesn't match its hash"));
            }
            entries += 1;
        }
        if entries != self.count {
            return Err(corrupt("entry count"));
        }
        Ok(())
    }

    /// The checked string whose `(offset, length)` sits at index byte `at`.
    fn str_at(&self, at: usize) -> Option<&'a str> {
        let (off, len) = (u32_at(self.index, at) as usize, u32_at(self.index, at + 4) as usize);
        let blob: &'a [u8] = self.blob;
        std::str::from_utf8(blob.get(off..off.checked_add(len)?)?).ok()
    }

    /// The string at index byte `at`, already validated by [`Pack::parse`].
    #[inline]
    fn str_unchecked(&self, at: usize) -> &'a str {
        let (off, len) = (u32_at(self.index, at) as usize, u32_at(self.index, at + 4) as usize);
        let blob: &'a [u8] = self.blob;
        let bytes = &blob[off..off + len];
        // SAFETY: `validate` checked that every key and translation slice is
        // in bounds and valid UTF-8, and the bytes never change (a leaked
        // read-only map of a file that is only ever replaced by rename, or
        // a leaked buffer).
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }

    /// The translation of `key`, if the pack has one.
    #[inline]
    pub fn get(&self, key: &str) -> Option<&'a str> {
        self.get_hashed(hash(key), key.len(), |stored| stored == key.as_bytes())
    }

    /// Look up a component's key without constructing its prefixed form.
    pub fn get_prefixed(&self, prefix: &str, key: &str) -> Option<&'a str> {
        self.get_hashed(hash_prefixed(prefix, key), prefix.len() + key.len(), |stored| {
            stored.strip_prefix(prefix.as_bytes()) == Some(key.as_bytes())
        })
    }

    /// Both lookup forms share the validated index and collision check.
    #[inline]
    fn get_hashed(&self, h: u64, len: usize, matches: impl FnOnce(&[u8]) -> bool) -> Option<&'a str> {
        let mut e = home_of(h, self.home_slots) * ENTRY_LEN;
        while let Some(slot) = self.index.get(e..e + ENTRY_LEN) {
            let at = u64_at(slot, 0);
            if at == h {
                // The stored key must be the one asked for: a hash collision
                // with a key that isn't in the pack finds nothing.
                let (key_off, key_len) = (u32_at(slot, 8) as usize, u32_at(slot, 12) as usize);
                if key_len != len {
                    return None;
                }
                // SAFETY: `validate` checked every slot's key and translation
                // ranges against the blob (and that they're UTF-8); the
                // bytes never change.
                let stored = unsafe { self.blob.get_unchecked(key_off..key_off + key_len) };
                if !matches(stored) {
                    return None;
                }
                let (val_off, val_len) = (u32_at(slot, 16) as usize, u32_at(slot, 20) as usize);
                // SAFETY: as above.
                return Some(unsafe { std::str::from_utf8_unchecked(self.blob.get_unchecked(val_off..val_off + val_len)) });
            }
            // An empty slot (no key) or a bigger hash: not in the pack.
            if at > h || u32_at(slot, 12) == 0 {
                return None;
            }
            e += ENTRY_LEN;
        }
        None
    }

    /// Every `(english, translation)` pair, in hash order.
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, &'a str)> + '_ {
        (0..self.index.len() / ENTRY_LEN).map(|s| s * ENTRY_LEN).filter(|e| u32_at(self.index, e + 12) != 0).map(move |e| (self.str_unchecked(e + 8), self.str_unchecked(e + 16)))
    }

    /// The English key whose translation is `value`.
    pub fn key_of(&self, value: &str) -> Option<&'a str> {
        self.iter().find(|(_, v)| *v == value).map(|(k, _)| k)
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// The Mandara version the pack was built for.
    pub fn version(&self) -> &'a str {
        self.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixed_hash_and_lookup_match_joined_utf8_keys() {
        for prefix in ["", "g:", "greeter:", "مجتمع:"] {
            for key in ["", "a", "1234567", "12345678", "اسم المستخدم", "सत्र सुरू करा"] {
                let joined = format!("{prefix}{key}");
                assert_eq!(hash_prefixed(prefix, key), hash(&joined));
                if joined.is_empty() { continue; }
                let bytes = build(Lang::Ar, "1", [(joined.as_str(), "translated")]).unwrap();
                let pack = Pack::parse(&bytes, "1", Lang::Ar).unwrap();
                assert_eq!(pack.get_prefixed(prefix, key), Some("translated"));
                assert_eq!(pack.get_prefixed(prefix, "missing"), None);
            }
        }
    }

    fn sample() -> Vec<u8> {
        build(Lang::Ar, "1.2.3", [("Settings", "الإعدادات"), ("Hello", "مرحبًا"), ("{n} items", "{n} عناصر")]).unwrap()
    }

    #[test]
    fn the_hash_is_stable() {
        // Pinned values (computed independently from the documented
        // definition): changing the hash breaks every published pack.
        assert_eq!(hash(""), 0xf47c_dffd_9671_363d);
        assert_eq!(hash("a"), 0x489e_983c_3a58_4d86);
        assert_eq!(hash("Settings"), 0xa554_b726_0cf4_7ba2);
        assert_eq!(hash("The new version is {size} smaller"), 0xd067_3a29_84bf_fe29);
    }

    #[test]
    fn round_trips_and_finds_every_key() {
        let bytes = sample();
        let p = Pack::parse(&bytes, "1.2.3", Lang::Ar).unwrap();
        assert_eq!((p.len(), p.version(), p.lang()), (3, "1.2.3", Lang::Ar));
        assert_eq!(p.get("Settings"), Some("الإعدادات"));
        assert_eq!(p.get("{n} items"), Some("{n} عناصر"));
        assert_eq!(p.get("Missing"), None);
        assert_eq!(p.get(""), None);
        assert_eq!(p.key_of("مرحبًا"), Some("Hello"));
        assert_eq!(p.iter().count(), 3);
    }

    #[test]
    fn empty_and_large_catalogs_work() {
        let empty = build(Lang::De, "1", std::iter::empty()).unwrap();
        let empty = Pack::parse(&empty, "1", Lang::De).unwrap();
        assert!(empty.is_empty() && empty.get("x").is_none());
        // Short keys differing in single bytes: no collisions, every one found.
        let keys: Vec<(String, String)> = (0..50_000).map(|i| (format!("key {i}"), format!("Wert {i}"))).collect();
        let bytes = build(Lang::De, "1", keys.iter().map(|(k, v)| (k.as_str(), v.as_str()))).unwrap();
        let p = Pack::parse(&bytes, "1", Lang::De).unwrap();
        assert_eq!(p.home_slots, 100_000, "twice the catalog");
        for (k, v) in &keys {
            assert_eq!(p.get(k), Some(v.as_str()));
        }
        assert_eq!(p.get("key 50000"), None);
    }

    #[test]
    fn duplicates_and_empty_strings_are_refused() {
        assert!(build(Lang::Ar, "1", [("a", "x"), ("a", "y")]).unwrap_err().contains("twice"));
        assert!(build(Lang::Ar, "1", [("a", "")]).is_err());
        assert!(build(Lang::Ar, "1", [("", "x")]).is_err());
    }

    #[test]
    fn the_wrong_version_or_language_is_refused() {
        let bytes = sample();
        assert_eq!(Pack::parse(&bytes, "1.2.4", Lang::Ar).unwrap_err(), PackError::Version { found: "1.2.3".into(), expected: "1.2.4".into() });
        assert!(matches!(Pack::parse(&bytes, "1.2.3", Lang::Fa), Err(PackError::Language { .. })));
    }

    #[test]
    fn damaged_files_are_errors_not_panics() {
        let good = sample();
        // Every truncation.
        for len in 0..good.len() {
            assert!(Pack::parse(&good[..len], "1.2.3", Lang::Ar).is_err(), "truncated to {len}");
        }
        // Every single-byte change is either rejected or still serves only
        // valid strings (a changed translation byte that stays UTF-8).
        for at in 0..good.len() {
            let mut bad = good.clone();
            bad[at] ^= 0xa5;
            if let Ok(p) = Pack::parse(&bad, "1.2.3", Lang::Ar) {
                for (k, v) in p.iter() {
                    assert_eq!(p.get(k), Some(v));
                }
            }
        }
        let mut wrong_magic = good.clone();
        wrong_magic[0] = b'X';
        assert_eq!(Pack::parse(&wrong_magic, "1.2.3", Lang::Ar).unwrap_err(), PackError::NotAPack);
        let mut future = good;
        future[4] = 9;
        assert_eq!(Pack::parse(&future, "1.2.3", Lang::Ar).unwrap_err(), PackError::Format(9));
    }

    #[test]
    fn a_key_with_the_same_slot_but_not_in_the_pack_finds_nothing() {
        let bytes = build(Lang::Ar, "1", [("Hello", "مرحبًا")]).unwrap();
        let p = Pack::parse(&bytes, "1", Lang::Ar).unwrap();
        assert_eq!(p.get("Hello"), Some("مرحبًا"));
        // One entry: every key's home is slot 0 or 1; none but "Hello" matches.
        for k in ["Hellp", "hello", "Hello ", "H"] {
            assert_eq!(p.get(k), None, "{k}");
        }
    }
}
