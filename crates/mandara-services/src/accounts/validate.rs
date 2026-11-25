//! Form logic for user accounts: username rules and suggestions, password
//! strength, e-mail sanity and the languages a user may pick.

use mandara_i18n::t;

/// `useradd`'s portable limit (`^[a-z_][a-z0-9_-]{0,31}$`).
pub const MAX_USERNAME_LEN: usize = 32;
/// Suggestions stop numbering here and fall back to a plain base.
const MAX_SUFFIX: u32 = 99;
/// Base name when a full name has no Latin letters to borrow.
const FALLBACK_BASE: &str = "user";

/// Names that are never offered or accepted for new accounts, on top of
/// whatever `/etc/passwd` and `/etc/group` already hold.
pub const RESERVED: &[&str] = &[
    "root", "daemon", "bin", "sys", "sync", "games", "man", "lp", "mail", "news", "uucp", "proxy", "www-data", "backup", "list", "irc",
    "gnats", "nobody", "nogroup", "admin", "administrator", "adm", "wheel", "sudo", "staff", "users", "operator", "shutdown", "halt",
    "ftp", "http", "dbus", "polkitd", "systemd", "avahi", "gdm", "sddm", "lightdm", "greeter", "greetd", "colord", "rtkit", "git",
    "messagebus", "kvm", "video", "audio", "input", "tty", "disk", "storage", "network", "guest",
];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UsernameProblem {
    Empty,
    TooLong,
    /// Must start with a lowercase letter or `_`.
    BadStart,
    /// Only lowercase letters, digits, `_` and `-`.
    BadChars,
    Taken,
    Reserved,
}

impl UsernameProblem {
    pub fn message(self) -> &'static str {
        match self {
            UsernameProblem::Empty => t("Choose a username"),
            UsernameProblem::TooLong => t("Usernames can be at most 32 characters"),
            UsernameProblem::BadStart => t("Start with a lowercase letter or an underscore"),
            UsernameProblem::BadChars => t("Use only lowercase letters, digits, - and _"),
            UsernameProblem::Taken => t("This username is already in use"),
            UsernameProblem::Reserved => t("This name is reserved for the system"),
        }
    }
}

fn shape_problem(name: &str) -> Option<UsernameProblem> {
    let mut chars = name.chars();
    let first = chars.next()?;
    if !(first.is_ascii_lowercase() || first == '_') {
        return Some(UsernameProblem::BadStart);
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
        return Some(UsernameProblem::BadChars);
    }
    (name.len() > MAX_USERNAME_LEN).then_some(UsernameProblem::TooLong)
}

/// Checks `name` against `^[a-z_][a-z0-9_-]{0,31}$`, the reserved list and
/// the names already `taken`.
pub fn validate_username(name: &str, taken: &[String]) -> Result<(), UsernameProblem> {
    if name.is_empty() {
        return Err(UsernameProblem::Empty);
    }
    if let Some(p) = shape_problem(name) {
        return Err(p);
    }
    if RESERVED.contains(&name) {
        return Err(UsernameProblem::Reserved);
    }
    if taken.iter().any(|t| t == name) {
        return Err(UsernameProblem::Taken);
    }
    Ok(())
}

/// Folds common accented Latin letters to ASCII.
fn fold(c: char) -> Option<char> {
    let c = c.to_lowercase().next().unwrap_or(c);
    Some(match c {
        'a'..='z' | '0'..='9' => c,
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'ē' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ı' => 'i',
        'ñ' | 'ń' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' => 'u',
        'ý' | 'ÿ' => 'y',
        'ś' | 'š' | 'ş' => 's',
        'ž' | 'ź' | 'ż' => 'z',
        'ğ' => 'g',
        'ß' => 's',
        _ => return None,
    })
}

fn ascii_words(full_name: &str) -> Vec<String> {
    full_name
        .split(|c: char| c.is_whitespace() || c == '-' || c == '.' || c == '\'')
        .map(|w| w.chars().filter_map(fold).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect()
}

fn usable(name: &str, taken: &[String]) -> bool {
    validate_username(name, taken).is_ok()
}

/// A valid, free username derived from a full name: "Nour El-Sayed" →
/// `nour`, then `nelsayed`, `nourelsayed`, `nour2`… Names without Latin
/// letters (e.g. Arabic) start from `user`.
pub fn suggest_username(full_name: &str, taken: &[String]) -> String {
    let words = ascii_words(full_name);
    let clip = |s: String| -> String {
        let s = s.trim_start_matches(|c: char| c.is_ascii_digit()).to_string();
        s.chars().take(MAX_USERNAME_LEN).collect()
    };
    let mut candidates: Vec<String> = Vec::new();
    if let Some(first) = words.first() {
        candidates.push(clip(first.clone()));
        if words.len() > 1 {
            let rest: String = words[1..].concat();
            candidates.push(clip(format!("{}{}", &first[..1], rest)));
            candidates.push(clip(words.concat()));
        }
    }
    candidates.retain(|c| !c.is_empty());
    if candidates.is_empty() {
        candidates.push(FALLBACK_BASE.to_string());
    }
    if let Some(found) = candidates.iter().find(|c| usable(c, taken)) {
        return found.clone();
    }
    let base = candidates.into_iter().next().unwrap_or_else(|| FALLBACK_BASE.to_string());
    (2..=MAX_SUFFIX)
        .map(|n| {
            let suffix = n.to_string();
            let keep = MAX_USERNAME_LEN - suffix.len();
            format!("{}{suffix}", base.chars().take(keep).collect::<String>())
        })
        .find(|c| usable(c, taken))
        .unwrap_or(base)
}

/// 0 = very weak … 4 = strong.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Strength(pub u8);

impl Strength {
    pub const MAX: u8 = 4;

    pub fn label(self) -> &'static str {
        match self.0 {
            0 => t("Too weak"),
            1 => t("Weak"),
            2 => t("Fair"),
            3 => t("Good"),
            _ => t("Strong"),
        }
    }
}

/// Approximate entropy bits that map to each score step (8 → 1, …).
const STRENGTH_BITS: [f32; 4] = [28.0, 40.0, 56.0, 72.0];
/// Below this, a password is "too weak" whatever its alphabet.
pub const MIN_PASSWORD_LEN: usize = 8;
const COMMON: &[&str] = &["password", "123456", "qwerty", "letmein", "welcome", "admin", "iloveyou", "monkey", "dragon", "abc123", "111111", "mandara"];

/// Estimates strength from length × alphabet size, penalizing common
/// passwords, repeats and passwords containing the user's own names.
pub fn password_strength(password: &str, personal: &[&str]) -> Strength {
    let len = password.chars().count();
    if len == 0 {
        return Strength(0);
    }
    let lower = password.to_lowercase();
    if COMMON.iter().any(|c| lower.contains(c)) || personal.iter().any(|p| p.chars().count() >= 3 && lower.contains(&p.to_lowercase())) {
        return Strength(0);
    }
    let mut alphabet = 0u32;
    if password.chars().any(|c| c.is_ascii_lowercase()) {
        alphabet += 26;
    }
    if password.chars().any(|c| c.is_ascii_uppercase()) {
        alphabet += 26;
    }
    if password.chars().any(|c| c.is_ascii_digit()) {
        alphabet += 10;
    }
    if password.chars().any(|c| c.is_ascii_punctuation() || c == ' ') {
        alphabet += 33;
    }
    if !password.is_ascii() {
        alphabet += 100;
    }
    let distinct = {
        let mut v: Vec<char> = password.chars().collect();
        v.sort_unstable();
        v.dedup();
        v.len()
    };
    // Repeats ("aaaaaaaa") count once per distinct character, plus a little.
    let effective = (distinct as f32 + (len - distinct) as f32 * 0.25).min(len as f32);
    let bits = effective * (alphabet.max(1) as f32).log2();
    let score = STRENGTH_BITS.iter().filter(|b| bits >= **b).count() as u8;
    if len < MIN_PASSWORD_LEN { Strength(score.min(1)) } else { Strength(score) }
}

/// Empty is fine (the field is optional); otherwise `local@domain.tld`.
pub fn valid_email(email: &str) -> bool {
    let email = email.trim();
    if email.is_empty() {
        return true;
    }
    match email.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty() && !domain.contains('@') && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && !email.contains(char::is_whitespace)
        }
        None => false,
    }
}

/// The ten languages Mandara ships, as AccountsService locales with their
/// native names. The empty locale means "system default".
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en_US.UTF-8", "English"),
    ("ar_EG.UTF-8", "العربية"),
    ("es_ES.UTF-8", "Español"),
    ("fr_FR.UTF-8", "Français"),
    ("de_DE.UTF-8", "Deutsch"),
    ("tr_TR.UTF-8", "Türkçe"),
    ("ru_RU.UTF-8", "Русский"),
    ("hi_IN.UTF-8", "हिन्दी"),
    ("zh_CN.UTF-8", "简体中文"),
    ("ja_JP.UTF-8", "日本語"),
];

/// The native name of `locale` (matching on the language code, so
/// `ar_SA.UTF-8` is still "العربية"); `None` for the system default or an
/// unknown language.
pub fn language_name(locale: &str) -> Option<&'static str> {
    let code = locale.split(['_', '.', '@']).next().unwrap_or("");
    if code.is_empty() {
        return None;
    }
    LANGUAGES.iter().find(|(l, _)| l.split('_').next() == Some(code)).map(|(_, n)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn validates_usernames() {
        let t = taken(&["nour", "root"]);
        assert_eq!(validate_username("", &t), Err(UsernameProblem::Empty));
        assert_eq!(validate_username("Nour", &t), Err(UsernameProblem::BadStart));
        assert_eq!(validate_username("1nour", &t), Err(UsernameProblem::BadStart));
        assert_eq!(validate_username("-x", &t), Err(UsernameProblem::BadStart));
        assert_eq!(validate_username("no ur", &t), Err(UsernameProblem::BadChars));
        assert_eq!(validate_username("nour.s", &t), Err(UsernameProblem::BadChars));
        assert_eq!(validate_username("nourÉ", &t), Err(UsernameProblem::BadChars));
        assert_eq!(validate_username(&"a".repeat(33), &t), Err(UsernameProblem::TooLong));
        assert_eq!(validate_username(&"a".repeat(32), &t), Ok(()));
        assert_eq!(validate_username("nour", &t), Err(UsernameProblem::Taken));
        assert_eq!(validate_username("daemon", &t), Err(UsernameProblem::Reserved));
        assert_eq!(validate_username("_svc-1", &t), Ok(()));
        assert_eq!(validate_username("youssef", &t), Ok(()));
    }

    #[test]
    fn suggests_free_usernames() {
        assert_eq!(suggest_username("Youssef Mansour", &[]), "youssef");
        assert_eq!(suggest_username("Youssef Mansour", &taken(&["youssef"])), "ymansour");
        assert_eq!(suggest_username("Youssef Mansour", &taken(&["youssef", "ymansour"])), "youssefmansour");
        assert_eq!(suggest_username("Youssef Mansour", &taken(&["youssef", "ymansour", "youssefmansour"])), "youssef2");
        assert_eq!(suggest_username("Zoë Ábalos", &[]), "zoe");
        assert_eq!(suggest_username("Nour El-Sayed", &taken(&["nour"])), "nelsayed");
        assert_eq!(suggest_username("مريم فهمي", &[]), "user");
        assert_eq!(suggest_username("مريم", &taken(&["user"])), "user2");
        assert_eq!(suggest_username("", &[]), "user");
        assert_eq!(suggest_username("Root", &[]), "root2", "reserved names get a suffix");
        let long = suggest_username(&"a".repeat(40), &taken(&[&"a".repeat(32)]));
        assert!(long.len() <= MAX_USERNAME_LEN && long.ends_with('2'));
        for s in ["Ahmed 3rd", "007 Bond", "J. R. R. Tolkien"] {
            assert!(validate_username(&suggest_username(s, &[]), &[]).is_ok(), "{s}");
        }
    }

    #[test]
    fn rates_passwords() {
        assert_eq!(password_strength("", &[]).0, 0);
        assert_eq!(password_strength("password1", &[]).0, 0, "common");
        assert_eq!(password_strength("nour2026!", &["nour"]).0, 0, "contains the username");
        assert!(password_strength("aaaaaaaaaaaa", &[]).0 <= 1, "repeats");
        assert!(password_strength("Ab3$", &[]).0 <= 1, "short");
        assert!(password_strength("kitten-lantern", &[]) >= Strength(2));
        assert_eq!(password_strength("Corniche-Lighthouse-1952!", &[]).0, Strength::MAX);
        assert!(password_strength("Corniche-Lighthouse-1952!", &[]) > password_strength("corniche", &[]));
    }

    #[test]
    fn checks_emails() {
        assert!(valid_email(""));
        assert!(valid_email("nour@example.org"));
        assert!(!valid_email("nour@"));
        assert!(!valid_email("nour.example.org"));
        assert!(!valid_email("a@b@c.d"));
        assert!(!valid_email("a b@c.d"));
    }

    #[test]
    fn names_languages() {
        assert_eq!(LANGUAGES.len(), 10);
        assert_eq!(language_name("ar_SA.UTF-8"), Some("العربية"));
        assert_eq!(language_name("ja_JP.UTF-8"), Some("日本語"));
        assert_eq!(language_name(""), None);
        assert_eq!(language_name("sw_KE.UTF-8"), None);
    }
}
