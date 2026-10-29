//! Two-factor codes: HOTP (RFC 4226) and TOTP (RFC 6238) over HMAC-SHA1 /
//! SHA-256 / SHA-512, `otpauth://` URIs, QR decoding (`rqrr`), and account
//! storage in the freedesktop Secret Service (gnome-keyring, KeePassXC,
//! KWallet's compat layer) over D-Bus — secrets never touch disk in
//! plaintext. Codes copied to the clipboard carry the password-manager hint
//! (so clipboard histories skip them) and are cleared after a timeout.
//!
//! The keyring is opened only when asked (the panel opening, a command), so
//! no unlock prompt ever appears unprompted.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use hmac::{Hmac, Mac};
use mandara_i18n::{t, tf};
use serde::Serialize;

use crate::actor::{Envelope, ServiceHandle, reply};

/// Attribute every Mandara item carries (`application=mandara-authenticator`).
pub const APPLICATION: &str = "mandara-authenticator";
const ATTR_APPLICATION: &str = "application";
const ATTR_ACCOUNT: &str = "mandara-account-id";
const ATTR_SCHEMA: &str = "xdg:schema";
const SCHEMA: &str = "org.mandara.Authenticator";
/// Default TOTP step and code length.
pub const DEFAULT_PERIOD: u64 = 30;
pub const DEFAULT_DIGITS: u32 = 6;
const MIN_DIGITS: u32 = 6;
const MAX_DIGITS: u32 = 10;
const MAX_PERIOD: u64 = 300;
/// Seconds after which a copied code is wiped from the clipboard.
pub const DEFAULT_CLEAR_SECONDS: u64 = 30;
/// Shortest secret accepted (RFC 4226 requires ≥ 128 bits but real
/// services issue 80-bit secrets).
const MIN_SECRET_BYTES: usize = 10;

// ── Base32 (RFC 4648) ────────────────────────────────────────────────────

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Decodes base32, ignoring case, spaces, dashes and padding.
pub fn base32_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() * 5 / 8);
    let (mut buffer, mut bits) = (0u64, 0u32);
    for ch in input.chars() {
        if ch.is_whitespace() || ch == '-' || ch == '=' {
            continue;
        }
        let up = ch.to_ascii_uppercase() as u8;
        let value = BASE32.iter().position(|b| *b == up).ok_or_else(|| tf("Invalid character {char} in the secret", &[("char", &format!("{ch:?}"))]))?;
        buffer = (buffer << 5) | value as u64;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// Encodes base32 without padding.
pub fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u64, 0u32);
    for b in bytes {
        buffer = (buffer << 8) | *b as u64;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

// ── OTP ──────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Algorithm {
    #[default]
    Sha1,
    Sha256,
    Sha512,
}

impl Algorithm {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_uppercase().replace('-', "").as_str() {
            "SHA1" => Ok(Algorithm::Sha1),
            "SHA256" => Ok(Algorithm::Sha256),
            "SHA512" => Ok(Algorithm::Sha512),
            other => Err(tf("Unsupported algorithm {name}", &[("name", &other)])),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Algorithm::Sha1 => "SHA1",
            Algorithm::Sha256 => "SHA256",
            Algorithm::Sha512 => "SHA512",
        }
    }

    fn mac(self, key: &[u8], message: &[u8]) -> Vec<u8> {
        fn run<M: Mac + hmac::digest::KeyInit>(key: &[u8], message: &[u8]) -> Vec<u8> {
            let mut m = <M as hmac::digest::KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
            m.update(message);
            m.finalize().into_bytes().to_vec()
        }
        match self {
            Algorithm::Sha1 => run::<Hmac<sha1::Sha1>>(key, message),
            Algorithm::Sha256 => run::<Hmac<sha2::Sha256>>(key, message),
            Algorithm::Sha512 => run::<Hmac<sha2::Sha512>>(key, message),
        }
    }
}

/// RFC 4226 HOTP value for `counter`, `digits` long (zero-padded).
pub fn hotp(secret: &[u8], counter: u64, digits: u32, algorithm: Algorithm) -> String {
    let mac = algorithm.mac(secret, &counter.to_be_bytes());
    let offset = (mac[mac.len() - 1] & 0x0f) as usize;
    let binary = ((mac[offset] as u32 & 0x7f) << 24) | ((mac[offset + 1] as u32) << 16) | ((mac[offset + 2] as u32) << 8) | mac[offset + 3] as u32;
    let digits = digits.clamp(1, MAX_DIGITS);
    let modulo = 10u64.pow(digits);
    format!("{:0width$}", binary as u64 % modulo, width = digits as usize)
}

/// RFC 6238 TOTP value at Unix time `unix` with step `period`.
pub fn totp(secret: &[u8], unix: u64, period: u64, digits: u32, algorithm: Algorithm) -> String {
    hotp(secret, unix / period.max(1), digits, algorithm)
}

/// Seconds left in the current TOTP step.
pub fn seconds_left(unix_millis: u64, period: u64) -> f64 {
    let p = period.max(1) * 1000;
    (p - unix_millis % p) as f64 / 1000.0
}

/// Groups a code for reading: `123 456`, `1234 5678`, `12 345 678`… — 6
/// and 8 split in half, others in threes from the right.
pub fn group_code(code: &str) -> String {
    let n = code.chars().count();
    if n == 6 || n == 8 {
        let (a, b) = code.split_at(n / 2);
        return format!("{a} {b}");
    }
    let chars: Vec<char> = code.chars().collect();
    let mut out = String::new();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && (n - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(*c);
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum OtpKind {
    Totp { period: u64 },
    Hotp { counter: u64 },
}

/// One account. The secret is wiped from memory on drop and never printed.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct Account {
    pub id: String,
    pub issuer: String,
    /// The account name (usually an e-mail address or user name).
    pub label: String,
    #[serde(skip)]
    pub secret: Vec<u8>,
    pub kind: OtpKind,
    pub algorithm: Algorithm,
    pub digits: u32,
}

impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Account").field("id", &self.id).field("issuer", &self.issuer).field("label", &self.label).field("secret", &"<redacted>").field("kind", &self.kind).finish()
    }
}

impl Drop for Account {
    fn drop(&mut self) {
        for b in self.secret.iter_mut() {
            // SAFETY: `b` is a valid, aligned, exclusively borrowed byte.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
}

impl Account {
    /// The current code (TOTP at `unix`; HOTP at its stored counter).
    pub fn code(&self, unix: u64) -> String {
        match self.kind {
            OtpKind::Totp { period } => totp(&self.secret, unix, period, self.digits, self.algorithm),
            OtpKind::Hotp { counter } => hotp(&self.secret, counter, self.digits, self.algorithm),
        }
    }

    /// `Issuer (label)`, or whichever is present.
    pub fn title(&self) -> String {
        match (self.issuer.is_empty(), self.label.is_empty()) {
            (false, false) => format!("{} ({})", self.issuer, self.label),
            (false, true) => self.issuer.clone(),
            _ => self.label.clone(),
        }
    }

    /// One or two letters for a monogram (`GitHub` → `G`, `Proton Mail` → `PM`).
    pub fn initials(&self) -> String {
        let name = if self.issuer.is_empty() { &self.label } else { &self.issuer };
        // `Demo · GitHub` → `GitHub`: a prefix before a middle dot is a tag.
        let name = name.rsplit(" · ").next().unwrap_or(name);
        let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        let first = |w: &str| w.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        match words.as_slice() {
            [] => "?".into(),
            [one] => first(one),
            [a, b, ..] => format!("{}{}", first(a), first(b)),
        }
    }

    /// Case-insensitive match on issuer and label.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty() || self.issuer.to_lowercase().contains(&q) || self.label.to_lowercase().contains(&q)
    }

    /// The `otpauth://` URI (what is stored as the keyring secret).
    pub fn to_uri(&self) -> String {
        let (kind, extra) = match self.kind {
            OtpKind::Totp { period } => ("totp", format!("&period={period}")),
            OtpKind::Hotp { counter } => ("hotp", format!("&counter={counter}")),
        };
        let path = if self.issuer.is_empty() { percent_encode(&self.label) } else { format!("{}:{}", percent_encode(&self.issuer), percent_encode(&self.label)) };
        let issuer = if self.issuer.is_empty() { String::new() } else { format!("&issuer={}", percent_encode(&self.issuer)) };
        format!("otpauth://{kind}/{path}?secret={}{issuer}&algorithm={}&digits={}{extra}", base32_encode(&self.secret), self.algorithm.name(), self.digits)
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(v) => {
                        out.push(v);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'@') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A fresh random account id (16 hex digits).
pub fn new_id() -> String {
    use std::io::Read as _;
    let mut bytes = [0u8; 8];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).is_err() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
        bytes = (nanos as u64 ^ std::process::id() as u64).to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parses an `otpauth://totp|hotp/Issuer:label?secret=…` URI.
pub fn parse_uri(uri: &str) -> Result<Account, String> {
    let uri = uri.trim();
    let rest = uri.strip_prefix("otpauth://").ok_or_else(|| t("Not an otpauth:// link").to_string())?;
    let (kind, rest) = rest.split_once('/').ok_or_else(|| t("The link has no account name").to_string())?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let params: HashMap<String, String> = query
        .split('&')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.to_ascii_lowercase(), percent_decode(v)))
        })
        .collect();
    let path = percent_decode(path);
    let (path_issuer, label) = match path.split_once(':') {
        Some((i, l)) => (i.trim().to_string(), l.trim().to_string()),
        None => (String::new(), path.trim().to_string()),
    };
    let issuer = params.get("issuer").cloned().filter(|i| !i.is_empty()).unwrap_or(path_issuer);
    let secret = base32_decode(params.get("secret").ok_or_else(|| t("The link has no secret").to_string())?)?;
    if secret.len() < MIN_SECRET_BYTES {
        return Err(t("The secret is too short").into());
    }
    let digits = match params.get("digits") {
        Some(d) => d.parse::<u32>().ok().filter(|d| (MIN_DIGITS..=MAX_DIGITS).contains(d)).ok_or_else(|| tf("Unsupported code length {digits}", &[("digits", d)]))?,
        None => DEFAULT_DIGITS,
    };
    let algorithm = params.get("algorithm").map(|a| Algorithm::parse(a)).transpose()?.unwrap_or_default();
    let kind = match kind.to_ascii_lowercase().as_str() {
        "totp" => OtpKind::Totp {
            period: match params.get("period") {
                Some(p) => p.parse::<u64>().ok().filter(|p| (1..=MAX_PERIOD).contains(p)).ok_or_else(|| tf("Unsupported period {period}", &[("period", p)]))?,
                None => DEFAULT_PERIOD,
            },
        },
        "hotp" => OtpKind::Hotp { counter: params.get("counter").and_then(|c| c.parse().ok()).ok_or_else(|| t("An HOTP link needs a counter").to_string())? },
        other => return Err(tf("Unknown code type {kind}", &[("kind", &other)])),
    };
    Ok(Account { id: new_id(), issuer, label, secret, kind, algorithm, digits })
}

/// Every QR code payload found in an RGBA image.
pub fn decode_qr(width: u32, height: u32, rgba: &[u8]) -> Vec<String> {
    if rgba.len() < (width as usize) * (height as usize) * 4 {
        return Vec::new();
    }
    let w = width as usize;
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(width as usize, height as usize, |x, y| {
        let i = (y * w + x) * 4;
        // Rec. 601 luma.
        ((rgba[i] as u32 * 299 + rgba[i + 1] as u32 * 587 + rgba[i + 2] as u32 * 114) / 1000) as u8
    });
    img.detect_grids().into_iter().filter_map(|g| g.decode().ok().map(|(_, content)| content)).collect()
}

/// Accounts from every otpauth QR code in an image.
pub fn accounts_from_image(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<Account>, String> {
    let payloads = decode_qr(width, height, rgba);
    if payloads.is_empty() {
        return Err(t("No QR code found in that area").into());
    }
    let accounts: Vec<Account> = payloads.iter().filter_map(|p| parse_uri(p).ok()).collect();
    if accounts.is_empty() {
        if payloads.iter().any(|p| p.starts_with("otpauth-migration://")) {
            return Err(t("Google Authenticator export codes aren't supported yet; export accounts one by one").into());
        }
        return Err(t("The QR code isn't a 2FA (otpauth://) code").into());
    }
    Ok(accounts)
}

// ── Clipboard ────────────────────────────────────────────────────────────

fn copy_generation() -> &'static AtomicU64 {
    static GEN: OnceLock<AtomicU64> = OnceLock::new();
    GEN.get_or_init(|| AtomicU64::new(0))
}

/// Copies a code flagged as a secret (clipboard histories, including
/// Mandara's, skip it) and wipes the clipboard after `clear_after` unless
/// something else was copied meanwhile.
pub fn copy_secret(text: &str, clear_after: Option<Duration>) -> Result<(), String> {
    use wl_clipboard_rs::copy::{ClipboardType, MimeSource, MimeType, Options, Seat, Source};
    let generation = copy_generation().fetch_add(1, Ordering::SeqCst) + 1;
    let serving = Arc::new(AtomicBool::new(true));
    let (tx, rx) = std::sync::mpsc::channel();
    let bytes = text.as_bytes().to_vec();
    let flag = serving.clone();
    std::thread::Builder::new()
        .name("mandara-otp-clipboard".into())
        .spawn(move || {
            let mut opts = Options::new();
            opts.foreground(true);
            let text = |mime: &str| MimeSource { source: Source::Bytes(bytes.clone().into_boxed_slice()), mime_type: MimeType::Specific(mime.into()) };
            let sources = vec![
                text("text/plain;charset=utf-8"),
                text("text/plain"),
                text("UTF8_STRING"),
                text("STRING"),
                MimeSource { source: Source::Bytes(b"secret".to_vec().into_boxed_slice()), mime_type: MimeType::Specific("x-kde-passwordManagerHint".into()) },
            ];
            match opts.prepare_copy_multi(sources) {
                Ok(prepared) => {
                    let _ = tx.send(Ok(()));
                    let _ = prepared.serve();
                }
                Err(e) => {
                    let _ = tx.send(Err(tf("Could not set the clipboard: {error}", &[("error", &e)])));
                }
            }
            flag.store(false, Ordering::SeqCst);
        })
        .map_err(|e| e.to_string())?;
    rx.recv().map_err(|_| t("The clipboard thread failed").to_string())??;
    if let Some(after) = clear_after {
        let _ = std::thread::Builder::new().name("mandara-otp-clear".into()).spawn(move || {
            std::thread::sleep(after);
            // Still ours (nothing newer copied, still being served)?
            if copy_generation().load(Ordering::SeqCst) == generation && serving.load(Ordering::SeqCst) {
                if let Err(e) = wl_clipboard_rs::copy::clear(ClipboardType::Regular, Seat::All) {
                    tracing::warn!("could not clear the clipboard: {e}");
                }
            }
        });
    }
    Ok(())
}

// ── Storage ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// No Secret Service on the session bus.
    Unavailable(String),
    /// The keyring stayed locked (unlock prompt dismissed).
    Locked,
    Other(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Unavailable(e) => write!(f, "{}", tf("No keyring service: {error}", &[("error", e)])),
            StoreError::Locked => write!(f, "{}", t("The keyring is locked")),
            StoreError::Other(e) => write!(f, "{e}"),
        }
    }
}

/// Where accounts live.
pub trait Store: Send {
    fn load(&mut self) -> Result<Vec<Account>, StoreError>;
    fn save(&mut self, account: &Account) -> Result<(), StoreError>;
    fn delete(&mut self, id: &str) -> Result<(), StoreError>;
    /// Human name of the backend (`Secret Service`, `Demo`).
    fn name(&self) -> &'static str;
}

/// In-memory store: tests and the `--demo` UI preview.
#[derive(Default)]
pub struct MemoryStore {
    pub accounts: Vec<Account>,
}

impl Store for MemoryStore {
    fn load(&mut self) -> Result<Vec<Account>, StoreError> {
        Ok(self.accounts.clone())
    }

    fn save(&mut self, account: &Account) -> Result<(), StoreError> {
        match self.accounts.iter_mut().find(|a| a.id == account.id) {
            Some(slot) => *slot = account.clone(),
            None => self.accounts.push(account.clone()),
        }
        Ok(())
    }

    fn delete(&mut self, id: &str) -> Result<(), StoreError> {
        let before = self.accounts.len();
        self.accounts.retain(|a| a.id != id);
        if before == self.accounts.len() { Err(StoreError::Other(tf("No account {id}", &[("id", &id)]))) } else { Ok(()) }
    }

    fn name(&self) -> &'static str {
        "Demo"
    }
}

/// Demo accounts built from the RFC test secrets — clearly labelled as
/// demo data in the UI; never mixed with real accounts.
pub fn demo_accounts() -> Vec<Account> {
    let rfc = b"12345678901234567890".to_vec();
    let mk = |issuer: &str, label: &str, kind: OtpKind, algorithm: Algorithm, digits: u32, secret: Vec<u8>| Account { id: new_id(), issuer: issuer.into(), label: label.into(), secret, kind, algorithm, digits };
    vec![
        mk("Demo · GitHub", "octocat", OtpKind::Totp { period: DEFAULT_PERIOD }, Algorithm::Sha1, 6, rfc.clone()),
        mk("Demo · Proton Mail", "layla@example.com", OtpKind::Totp { period: DEFAULT_PERIOD }, Algorithm::Sha256, 6, b"12345678901234567890123456789012".to_vec()),
        mk("Demo · Mandara Cloud", "admin", OtpKind::Totp { period: 60 }, Algorithm::Sha512, 8, b"1234567890123456789012345678901234567890123456789012345678901234".to_vec()),
        mk("Demo · Bank", "123-456", OtpKind::Hotp { counter: 0 }, Algorithm::Sha1, 6, rfc),
    ]
}

/// The freedesktop Secret Service over the session bus (plain session:
/// secrets travel over the local bus only, the same as libsecret's
/// fallback; they are never written to disk by Mandara).
pub struct SecretService {
    conn: zbus::Connection,
    session: zbus::zvariant::OwnedObjectPath,
}

const SS_DEST: &str = "org.freedesktop.secrets";
const SS_PATH: &str = "/org/freedesktop/secrets";
const SS_SERVICE: &str = "org.freedesktop.Secret.Service";
const SS_COLLECTION: &str = "org.freedesktop.Secret.Collection";
const SS_ITEM: &str = "org.freedesktop.Secret.Item";
const SS_PROMPT: &str = "org.freedesktop.Secret.Prompt";
const NO_PROMPT: &str = "/";

type SecretStruct = (zbus::zvariant::OwnedObjectPath, Vec<u8>, Vec<u8>, String);

fn other(e: impl fmt::Display) -> StoreError {
    StoreError::Other(e.to_string())
}

impl SecretService {
    /// Connects and opens a session (no prompts).
    pub fn connect() -> Result<Self, StoreError> {
        crate::runtime::block_on(async {
            let conn = crate::runtime::session().await.map_err(|e| StoreError::Unavailable(e.to_string()))?;
            let service = crate::actor::proxy(&conn, SS_DEST, SS_PATH, SS_SERVICE).await.map_err(|e| StoreError::Unavailable(e.to_string()))?;
            let (_, session): (zbus::zvariant::OwnedValue, zbus::zvariant::OwnedObjectPath) =
                service.call("OpenSession", &("plain", zbus::zvariant::Value::from(""))).await.map_err(|e| StoreError::Unavailable(e.to_string()))?;
            Ok(Self { conn, session })
        })
    }

    fn attributes() -> HashMap<&'static str, &'static str> {
        HashMap::from([(ATTR_APPLICATION, APPLICATION)])
    }

    /// Our item paths, `(unlocked, locked)` — read-only, never prompts.
    pub fn search(&self) -> Result<(Vec<zbus::zvariant::OwnedObjectPath>, Vec<zbus::zvariant::OwnedObjectPath>), StoreError> {
        crate::runtime::block_on(async {
            let service = crate::actor::proxy(&self.conn, SS_DEST, SS_PATH, SS_SERVICE).await.map_err(other)?;
            service.call("SearchItems", &(Self::attributes(),)).await.map_err(other)
        })
    }

    /// Runs a prompt (if any) and waits for its outcome.
    async fn prompt(&self, path: &zbus::zvariant::OwnedObjectPath) -> Result<(), StoreError> {
        use futures::StreamExt as _;
        if path.as_str() == NO_PROMPT {
            return Ok(());
        }
        let prompt = crate::actor::proxy(&self.conn, SS_DEST, path.as_str(), SS_PROMPT).await.map_err(other)?;
        let mut completed = prompt.receive_signal("Completed").await.map_err(other)?;
        prompt.call_method("Prompt", &("",)).await.map_err(other)?;
        let msg = completed.next().await.ok_or(StoreError::Locked)?;
        let (dismissed, _): (bool, zbus::zvariant::OwnedValue) = msg.body().deserialize().map_err(other)?;
        if dismissed { Err(StoreError::Locked) } else { Ok(()) }
    }

    async fn unlock(&self, paths: &[zbus::zvariant::OwnedObjectPath]) -> Result<(), StoreError> {
        if paths.is_empty() {
            return Ok(());
        }
        let service = crate::actor::proxy(&self.conn, SS_DEST, SS_PATH, SS_SERVICE).await.map_err(other)?;
        let (_, prompt): (Vec<zbus::zvariant::OwnedObjectPath>, zbus::zvariant::OwnedObjectPath) = service.call("Unlock", &(paths,)).await.map_err(other)?;
        self.prompt(&prompt).await
    }

    async fn item_id(&self, path: &zbus::zvariant::OwnedObjectPath) -> Option<String> {
        let item = crate::actor::proxy(&self.conn, SS_DEST, path.as_str(), SS_ITEM).await.ok()?;
        let attrs: HashMap<String, String> = item.get_property("Attributes").await.ok()?;
        attrs.get(ATTR_ACCOUNT).cloned()
    }

    async fn find_paths(&self, id: &str) -> Result<Vec<zbus::zvariant::OwnedObjectPath>, StoreError> {
        let service = crate::actor::proxy(&self.conn, SS_DEST, SS_PATH, SS_SERVICE).await.map_err(other)?;
        let mut attrs = Self::attributes();
        attrs.insert(ATTR_ACCOUNT, id);
        let (unlocked, locked): (Vec<zbus::zvariant::OwnedObjectPath>, Vec<zbus::zvariant::OwnedObjectPath>) = service.call("SearchItems", &(attrs,)).await.map_err(other)?;
        Ok(unlocked.into_iter().chain(locked).collect())
    }
}

impl Store for SecretService {
    fn load(&mut self) -> Result<Vec<Account>, StoreError> {
        let (unlocked, locked) = self.search()?;
        crate::runtime::block_on(async {
            self.unlock(&locked).await?;
            let all: Vec<zbus::zvariant::OwnedObjectPath> = unlocked.into_iter().chain(locked).collect();
            if all.is_empty() {
                return Ok(Vec::new());
            }
            let service = crate::actor::proxy(&self.conn, SS_DEST, SS_PATH, SS_SERVICE).await.map_err(other)?;
            let secrets: HashMap<zbus::zvariant::OwnedObjectPath, SecretStruct> = service.call("GetSecrets", &(&all, &self.session)).await.map_err(other)?;
            let mut accounts = Vec::new();
            for (path, (_, _, value, _)) in secrets {
                let uri = String::from_utf8_lossy(&value).into_owned();
                match parse_uri(&uri) {
                    Ok(mut account) => {
                        account.id = self.item_id(&path).await.unwrap_or_else(new_id);
                        accounts.push(account);
                    }
                    Err(e) => tracing::warn!("skipping unreadable keyring item {}: {e}", path.as_str()),
                }
            }
            accounts.sort_by(|a, b| a.issuer.to_lowercase().cmp(&b.issuer.to_lowercase()).then_with(|| a.label.cmp(&b.label)));
            Ok(accounts)
        })
    }

    fn save(&mut self, account: &Account) -> Result<(), StoreError> {
        crate::runtime::block_on(async {
            let service = crate::actor::proxy(&self.conn, SS_DEST, SS_PATH, SS_SERVICE).await.map_err(other)?;
            let collection: zbus::zvariant::OwnedObjectPath = service.call("ReadAlias", &("default",)).await.map_err(other)?;
            if collection.as_str() == NO_PROMPT {
                return Err(StoreError::Other(t("There is no default keyring; create one in your keyring app").into()));
            }
            self.unlock(std::slice::from_ref(&collection)).await?;
            let coll = crate::actor::proxy(&self.conn, SS_DEST, collection.as_str(), SS_COLLECTION).await.map_err(other)?;
            let attrs: HashMap<&str, &str> = HashMap::from([(ATTR_APPLICATION, APPLICATION), (ATTR_ACCOUNT, account.id.as_str()), (ATTR_SCHEMA, SCHEMA)]);
            let label = format!("Mandara Authenticator: {}", account.title());
            let mut props: HashMap<&str, zbus::zvariant::Value> = HashMap::new();
            props.insert("org.freedesktop.Secret.Item.Label", zbus::zvariant::Value::from(label));
            props.insert("org.freedesktop.Secret.Item.Attributes", zbus::zvariant::Value::from(attrs));
            let secret = (self.session.clone(), Vec::<u8>::new(), account.to_uri().into_bytes(), "text/plain".to_string());
            let (_, prompt): (zbus::zvariant::OwnedObjectPath, zbus::zvariant::OwnedObjectPath) = coll.call("CreateItem", &(props, secret, true)).await.map_err(other)?;
            self.prompt(&prompt).await
        })
    }

    fn delete(&mut self, id: &str) -> Result<(), StoreError> {
        crate::runtime::block_on(async {
            let paths = self.find_paths(id).await?;
            if paths.is_empty() {
                return Err(StoreError::Other(tf("No account {id}", &[("id", &id)])));
            }
            for path in paths {
                let item = crate::actor::proxy(&self.conn, SS_DEST, path.as_str(), SS_ITEM).await.map_err(other)?;
                let prompt: zbus::zvariant::OwnedObjectPath = item.call("Delete", &()).await.map_err(other)?;
                self.prompt(&prompt).await?;
            }
            Ok(())
        })
    }

    fn name(&self) -> &'static str {
        "Secret Service"
    }
}

// ── Actor ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "detail", rename_all = "kebab-case")]
pub enum AuthStatus {
    /// The keyring hasn't been opened yet.
    #[default]
    Idle,
    Loading,
    Ready,
    Locked,
    Unavailable(String),
    Error(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthState {
    pub status: AuthStatus,
    pub accounts: Vec<Account>,
    /// Showing in-memory demo accounts instead of the keyring.
    pub demo: bool,
    /// Backend name.
    pub backend: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AuthCommand {
    /// Open the keyring and read accounts (prompts to unlock if needed).
    Load,
    Add(Account),
    Remove(String),
    /// Advance an HOTP counter (after its code was used).
    NextCounter(String),
    /// Switch to / from in-memory demo data.
    Demo(bool),
    /// Current codes of accounts matching a query (JSON reply).
    Codes(String),
}

pub type AuthService = ServiceHandle<AuthState, AuthCommand>;

/// Applies one command to `store` / `state` (separate from the actor loop
/// so it is testable against a [`MemoryStore`]).
pub fn handle(store: &mut dyn Store, state: &mut AuthState, command: AuthCommand) -> Result<String, String> {
    match command {
        AuthCommand::Load => match store.load() {
            Ok(accounts) => {
                state.accounts = accounts;
                state.status = AuthStatus::Ready;
                serde_json::to_string(&state.accounts).map_err(|e| e.to_string())
            }
            Err(e) => {
                state.status = match &e {
                    StoreError::Locked => AuthStatus::Locked,
                    StoreError::Unavailable(m) => AuthStatus::Unavailable(m.clone()),
                    StoreError::Other(m) => AuthStatus::Error(m.clone()),
                };
                Err(e.to_string())
            }
        },
        AuthCommand::Add(account) => {
            if state.accounts.iter().any(|a| a.secret == account.secret && a.issuer == account.issuer && a.label == account.label) {
                return Err(tf("{account} is already added", &[("account", &account.title())]));
            }
            store.save(&account).map_err(|e| e.to_string())?;
            let title = account.title();
            state.accounts.push(account);
            state.accounts.sort_by(|a, b| a.issuer.to_lowercase().cmp(&b.issuer.to_lowercase()).then_with(|| a.label.cmp(&b.label)));
            Ok(tf("Added {account}", &[("account", &title)]))
        }
        AuthCommand::Remove(id) => {
            store.delete(&id).map_err(|e| e.to_string())?;
            state.accounts.retain(|a| a.id != id);
            Ok(t("Removed").into())
        }
        AuthCommand::NextCounter(id) => {
            let account = state.accounts.iter_mut().find(|a| a.id == id).ok_or_else(|| tf("No account {id}", &[("id", &id)]))?;
            let OtpKind::Hotp { counter } = account.kind else { return Err(t("Only HOTP accounts have a counter").into()) };
            let mut next = account.clone();
            next.kind = OtpKind::Hotp { counter: counter + 1 };
            store.save(&next).map_err(|e| e.to_string())?;
            *account = next;
            Ok(account.code(0))
        }
        AuthCommand::Codes(query) => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            let codes: Vec<serde_json::Value> = state
                .accounts
                .iter()
                .filter(|a| a.matches(&query) || a.id == query)
                .map(|a| {
                    let left = match a.kind {
                        OtpKind::Totp { period } => Some(seconds_left(now.as_millis() as u64, period)),
                        OtpKind::Hotp { .. } => None,
                    };
                    serde_json::json!({ "id": a.id, "issuer": a.issuer, "label": a.label, "code": a.code(now.as_secs()), "seconds_left": left })
                })
                .collect();
            if codes.is_empty() {
                return Err(tf("No account matches {query}", &[("query", &query)]));
            }
            serde_json::to_string(&codes).map_err(|e| e.to_string())
        }
        AuthCommand::Demo(_) => Ok(String::new()),
    }
}

/// Starts the authenticator actor. It stays idle (no D-Bus connection)
/// until the first command.
pub fn spawn_authenticator_service() -> AuthService {
    let (tx, updates) = async_channel::bounded(8);
    let (commands, rx) = async_channel::unbounded::<Envelope<AuthCommand>>();
    let _ = std::thread::Builder::new().name("mandara-authenticator".into()).spawn(move || {
        let mut state = AuthState { backend: "Secret Service", ..Default::default() };
        let mut keyring: Option<SecretService> = None;
        let mut demo = MemoryStore::default();
        while let Ok(Envelope { command, reply: r }) = rx.recv_blocking() {
            if let AuthCommand::Demo(on) = command {
                state.demo = on;
                if on {
                    demo.accounts = demo_accounts();
                }
                state.accounts.clear();
                state.status = AuthStatus::Idle;
                state.backend = if on { demo.name() } else { "Secret Service" };
                let result = if on { handle(&mut demo, &mut state, AuthCommand::Load) } else { Ok(String::new()) };
                if tx.send_blocking(state.clone()).is_err() {
                    return;
                }
                reply(&r, result);
                continue;
            }
            if command == AuthCommand::Load && !state.demo {
                state.status = AuthStatus::Loading;
                let _ = tx.send_blocking(state.clone());
            }
            let result = if state.demo {
                handle(&mut demo, &mut state, command)
            } else {
                if keyring.is_none() {
                    match SecretService::connect() {
                        Ok(k) => keyring = Some(k),
                        Err(e) => {
                            state.status = AuthStatus::Unavailable(e.to_string());
                            if tx.send_blocking(state.clone()).is_err() {
                                return;
                            }
                            reply(&r, Err(e.to_string()));
                            continue;
                        }
                    }
                }
                let store = keyring.as_mut().expect("connected above");
                // Adding before the first load still needs the existing list.
                if state.status != AuthStatus::Ready && !matches!(command, AuthCommand::Load) {
                    let _ = handle(store, &mut state, AuthCommand::Load);
                }
                let result = handle(store, &mut state, command);
                // A broken connection is re-opened next time.
                if matches!(state.status, AuthStatus::Unavailable(_)) {
                    keyring = None;
                }
                result
            };
            // State first, so a caller awaiting the reply sees it applied.
            if tx.send_blocking(state.clone()).is_err() {
                return;
            }
            reply(&r, result);
        }
    });
    ServiceHandle::new(updates, commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_SHA1: &[u8] = b"12345678901234567890";
    const RFC_SHA256: &[u8] = b"12345678901234567890123456789012";
    const RFC_SHA512: &[u8] = b"1234567890123456789012345678901234567890123456789012345678901234";

    #[test]
    fn rfc4226_hotp_vectors() {
        let expected = ["755224", "287082", "359152", "969429", "338314", "254676", "287922", "162583", "399871", "520489"];
        for (counter, code) in expected.iter().enumerate() {
            assert_eq!(hotp(RFC_SHA1, counter as u64, 6, Algorithm::Sha1), *code, "counter {counter}");
        }
    }

    #[test]
    fn rfc6238_totp_vectors() {
        let table: [(u64, &str, &str, &str); 6] = [
            (59, "94287082", "46119246", "90693936"),
            (1_111_111_109, "07081804", "68084774", "25091201"),
            (1_111_111_111, "14050471", "67062674", "99943326"),
            (1_234_567_890, "89005924", "91819424", "93441116"),
            (2_000_000_000, "69279037", "90698825", "38618901"),
            (20_000_000_000, "65353130", "77737706", "47863826"),
        ];
        for (time, sha1, sha256, sha512) in table {
            assert_eq!(totp(RFC_SHA1, time, 30, 8, Algorithm::Sha1), sha1, "SHA1 @ {time}");
            assert_eq!(totp(RFC_SHA256, time, 30, 8, Algorithm::Sha256), sha256, "SHA256 @ {time}");
            assert_eq!(totp(RFC_SHA512, time, 30, 8, Algorithm::Sha512), sha512, "SHA512 @ {time}");
        }
    }

    #[test]
    fn base32_round_trips() {
        assert_eq!(base32_decode("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").unwrap(), RFC_SHA1);
        assert_eq!(base32_encode(RFC_SHA1), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(base32_decode("gezd gnbv-gy3t qojq====").unwrap(), b"1234567890");
        assert_eq!(base32_decode("MZXW6YQ").unwrap(), b"foob");
        assert_eq!(base32_encode(b"foob"), "MZXW6YQ");
        assert!(base32_decode("ABC1").is_err());
    }

    #[test]
    fn parses_otpauth_uris() {
        let a = parse_uri("otpauth://totp/ACME%20Co:john.doe@email.com?secret=HXDMVJECJJWSRB3HWIZR4IFUGFTMXBOZ&issuer=ACME%20Co&algorithm=SHA1&digits=6&period=30").unwrap();
        assert_eq!((a.issuer.as_str(), a.label.as_str()), ("ACME Co", "john.doe@email.com"));
        assert_eq!(a.kind, OtpKind::Totp { period: 30 });
        assert_eq!(a.digits, 6);
        assert_eq!(a.initials(), "AC");
        let mut demo = a.clone();
        demo.issuer = "Demo · GitHub".into();
        assert_eq!(demo.initials(), "G");
        let h = parse_uri("otpauth://hotp/alice?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&counter=3&algorithm=sha256&digits=8").unwrap();
        assert_eq!(h.kind, OtpKind::Hotp { counter: 3 });
        assert_eq!(h.algorithm, Algorithm::Sha256);
        assert!(h.issuer.is_empty());
        assert_eq!(h.title(), "alice");
        // Round trip through our own URI (what the keyring stores).
        let back = parse_uri(&a.to_uri()).unwrap();
        assert_eq!((back.issuer.clone(), back.label.clone(), back.secret.clone(), back.kind.clone()), (a.issuer.clone(), a.label.clone(), a.secret.clone(), a.kind.clone()));
        for bad in ["https://x", "otpauth://totp/x", "otpauth://totp/x?secret=AAAA", "otpauth://totp/x?secret=GEZDGNBVGY3TQOJQ&digits=3", "otpauth://hotp/x?secret=GEZDGNBVGY3TQOJQ", "otpauth://totp/x?secret=GEZDGNBVGY3TQOJQ&algorithm=MD5"] {
            assert!(parse_uri(bad).is_err(), "{bad}");
        }
        assert!(!format!("{a:?}").contains("HXDM"), "Debug never prints the secret");
    }

    #[test]
    fn grouping_and_countdown() {
        assert_eq!(group_code("123456"), "123 456");
        assert_eq!(group_code("12345678"), "1234 5678");
        assert_eq!(group_code("1234567"), "1 234 567");
        assert_eq!(seconds_left(59_000, 30), 1.0);
        assert_eq!(seconds_left(60_000, 30), 30.0);
        assert_eq!(seconds_left(61_500, 30), 28.5);
    }

    #[test]
    fn memory_store_actor_logic() {
        let mut store = MemoryStore::default();
        let mut state = AuthState::default();
        assert!(handle(&mut store, &mut state, AuthCommand::Load).is_ok());
        assert_eq!(state.status, AuthStatus::Ready);
        let account = parse_uri("otpauth://hotp/Bank:me?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&counter=0").unwrap();
        let id = account.id.clone();
        handle(&mut store, &mut state, AuthCommand::Add(account.clone())).unwrap();
        assert!(handle(&mut store, &mut state, AuthCommand::Add(account)).is_err(), "duplicates are refused");
        assert_eq!(handle(&mut store, &mut state, AuthCommand::NextCounter(id.clone())).unwrap(), "287082");
        assert_eq!(store.accounts[0].kind, OtpKind::Hotp { counter: 1 }, "the counter is persisted");
        let codes: serde_json::Value = serde_json::from_str(&handle(&mut store, &mut state, AuthCommand::Codes("bank".into())).unwrap()).unwrap();
        assert_eq!(codes[0]["code"], "287082");
        assert!(handle(&mut store, &mut state, AuthCommand::Codes("nope".into())).is_err());
        let listed: serde_json::Value = serde_json::from_str(&handle(&mut store, &mut state, AuthCommand::Load).unwrap()).unwrap();
        assert!(listed[0].get("secret").is_none(), "listings never include secrets");
        handle(&mut store, &mut state, AuthCommand::Remove(id.clone())).unwrap();
        assert!(state.accounts.is_empty() && store.accounts.is_empty());
        assert!(handle(&mut store, &mut state, AuthCommand::Remove(id)).is_err());
    }

    #[test]
    fn demo_accounts_are_labelled() {
        let demo = demo_accounts();
        assert!(demo.iter().all(|a| a.issuer.starts_with("Demo")));
        assert_eq!(demo[0].code(59), "287082", "6-digit truncation of the RFC SHA1 vector");
    }

    #[test]
    fn decodes_no_qr_from_blank_image() {
        let blank = vec![255u8; 64 * 64 * 4];
        assert!(decode_qr(64, 64, &blank).is_empty());
        assert!(accounts_from_image(64, 64, &blank).is_err());
        assert!(decode_qr(64, 64, &blank[..10]).is_empty());
    }

    /// A QR code (from `qrencode -l L`, `#` = dark module) rendered at 4 px
    /// per module decodes back to its otpauth link.
    #[test]
    fn decodes_a_rendered_qr_code() {
        const MODULES: &[&str] = &[
        ".....................................",
        ".....................................",
        "..#######.#..##.###..####...#######..",
        "..#.....#.###.###..#.#.#..#.#.....#..",
        "..#.###.#...###.###...##.#..#.###.#..",
        "..#.###.#...#...#....#.##...#.###.#..",
        "..#.###.#.####.####.#.##....#.###.#..",
        "..#.....#.#..#...#.#..##..#.#.....#..",
        "..#######.#.#.#.#.#.#.#.#.#.#######..",
        "..........#.###.##.......#...........",
        "..###..##.###..######.#..#.####..##..",
        "..#####..#.##..###....#..##..#.#.##..",
        "..#.....##...#....#..#.#.#.##...#.#..",
        "...#.....###...#.##.....####.###.#...",
        "..#...######.###.######.....##...##..",
        "...#.#.#.##...#..#..##..##.....#..#..",
        "..###.####....###...##.#.#.######.#..",
        ".....###..#..##.#...###..####..#.....",
        "..########.##..####.#.#.#.#.#..#.##..",
        "....#.##..#.#..###..##.####..#..###..",
        "..##...####..#....##.#.#.#...####.#..",
        "...#####.#.....#...#.##.#.##.#.......",
        "...###..#.#.####.##.###.#.#.#.....#..",
        "...####..##.#.#..#.##....##..#..###..",
        "..##.#####.##.###.####.###..#..#..#..",
        ".....###.#..###.#####.##..##.##..##..",
        "..##....#...#..##...#..#########.....",
        "..........##...##..##.....#...###.#..",
        "..#######...##...#.##...#.#.#.#...#..",
        "..#.....#.#.#..#...###.#.##...#......",
        "..#.###.#..#.###....#.##.#######.##..",
        "..#.###.#..##.#..#.##..##.....###.#..",
        "..#.###.#.###.###.#.#.#...#...#..##..",
        "..#.....#.##.##.##.#.###..#.#..#.....",
        "..#######.###..###..####..##..#...#..",
        ".....................................",
        ".....................................",
        ];
        const SCALE: usize = 4;
        let side = MODULES.len() * SCALE;
        let mut rgba = vec![255u8; side * side * 4];
        for (y, row) in MODULES.iter().enumerate() {
            for (x, cell) in row.chars().enumerate() {
                if cell != '#' {
                    continue;
                }
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let i = ((y * SCALE + dy) * side + x * SCALE + dx) * 4;
                        rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
                    }
                }
            }
        }
        let accounts = accounts_from_image(side as u32, side as u32, &rgba).expect("QR decodes");
        assert_eq!(accounts.len(), 1);
        assert_eq!((accounts[0].issuer.as_str(), accounts[0].label.as_str()), ("ACME", "demo"));
        assert_eq!(accounts[0].secret, b"1234567890");
    }

    /// Read-only probe of the live Secret Service: searches for our items
    /// without unlocking or writing anything. Skips without a session bus.
    #[test]
    fn secret_service_search_is_read_only() {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
            return;
        }
        let Ok(service) = SecretService::connect() else { return };
        let (unlocked, locked) = service.search().expect("SearchItems works");
        assert!(unlocked.len() + locked.len() < 10_000);
    }
}
