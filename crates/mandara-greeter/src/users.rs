//! Login users from `/etc/passwd` and their avatars.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::RenderImage;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub name: String,
    /// GECOS full name, or the login name.
    pub display: String,
    pub uid: u32,
    pub home: PathBuf,
    pub shell: String,
}

impl User {
    /// One or two initials for the monogram avatar.
    pub fn initials(&self) -> String {
        let words: Vec<&str> = self.display.split_whitespace().collect();
        let mut out: String = match words.as_slice() {
            [] => String::new(),
            [one] => one.chars().take(1).collect(),
            [first, .., last] => first.chars().take(1).chain(last.chars().take(1)).collect(),
        };
        if out.is_empty() {
            out = self.name.chars().take(1).collect();
        }
        out.to_uppercase()
    }
}

/// `UID_MIN` / `UID_MAX` from `/etc/login.defs` (defaults 1000 / 60000).
pub fn uid_range(login_defs: &str) -> (u32, u32) {
    let mut range = (1000, 60000);
    for line in login_defs.lines() {
        let mut it = line.split_whitespace();
        match (it.next(), it.next().and_then(|v| v.parse().ok())) {
            (Some("UID_MIN"), Some(v)) => range.0 = v,
            (Some("UID_MAX"), Some(v)) => range.1 = v,
            _ => {}
        }
    }
    range
}

/// Whether `shell` lets the user log in.
fn login_shell(shell: &str, shells: Option<&str>) -> bool {
    let shell = shell.trim();
    if shell.is_empty() {
        // An empty shell field means /bin/sh.
        return true;
    }
    let base = shell.rsplit('/').next().unwrap_or(shell);
    if matches!(base, "nologin" | "false" | "true" | "sync" | "halt" | "shutdown") {
        return false;
    }
    match shells {
        Some(list) => list.lines().map(str::trim).any(|l| l == shell) || !list.lines().any(|l| l.trim().starts_with('/')),
        None => true,
    }
}

/// Parses passwd text into login users (uid in range, real login shell),
/// sorted by uid.
pub fn parse_passwd(passwd: &str, range: (u32, u32), shells: Option<&str>) -> Vec<User> {
    let mut users: Vec<User> = passwd
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|line| {
            let f: Vec<&str> = line.split(':').collect();
            if f.len() < 7 {
                return None;
            }
            let uid: u32 = f[2].parse().ok()?;
            if uid < range.0 || uid > range.1 || uid == 65534 {
                return None;
            }
            if !login_shell(f[6], shells) {
                return None;
            }
            let gecos = f[4].split(',').next().unwrap_or("").trim();
            Some(User {
                name: f[0].to_string(),
                display: if gecos.is_empty() { f[0].to_string() } else { gecos.to_string() },
                uid,
                home: PathBuf::from(f[5]),
                shell: f[6].to_string(),
            })
        })
        .collect();
    users.sort_by_key(|u| u.uid);
    users.dedup_by(|a, b| a.name == b.name);
    users
}

/// Login users on this machine.
pub fn list_users() -> Vec<User> {
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let range = uid_range(&std::fs::read_to_string("/etc/login.defs").unwrap_or_default());
    let shells = std::fs::read_to_string("/etc/shells").ok();
    parse_passwd(&passwd, range, shells.as_deref())
}

/// The current user (demo mode), from the passwd entry of our uid.
pub fn current_user() -> Option<User> {
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    parse_passwd(&passwd, (uid, uid), None).into_iter().next().or_else(|| {
        let name = std::env::var("USER").ok()?;
        Some(User { display: name.clone(), name, uid, home: dirs::home_dir().unwrap_or_default(), shell: String::new() })
    })
}

/// Avatar candidates: AccountsService's `IconFile` (what Studio → Users
/// sets), its icon directory (world-readable on most systems, so it works
/// for the greeter user), then `~/.face`.
pub fn avatar_paths(user: &User) -> Vec<PathBuf> {
    mandara_services::accounts::avatar_candidates(&user.name, &user.home)
}

/// Loads, center-crops and downsizes the first readable avatar into a
/// square BGRA image of `size` pixels.
pub fn load_avatar(user: &User, size: u32) -> Option<Arc<RenderImage>> {
    avatar_paths(user).into_iter().find_map(|p| {
        let bytes = std::fs::read(&p).ok()?;
        let img = image::load_from_memory(&bytes).ok()?;
        let side = img.width().min(img.height());
        let cropped = img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side);
        let mut rgba = cropped.resize_exact(size, size, image::imageops::FilterType::Lanczos3).to_rgba8();
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)])))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "\
root:x:0:0:root:/root:/bin/bash
bin:x:1:1::/:/usr/bin/nologin
greeter:x:964:964:greetd greeter user:/var/lib/greetd:/bin/bash
layla:x:1000:1000:Layla Nour,,,:/home/layla:/usr/bin/fish
guest:x:1001:1001::/home/guest:/bin/bash
svc:x:1002:1002:Service:/srv:/usr/sbin/nologin
locked:x:1003:1003::/home/locked:/bin/false
nobody:x:65534:65534:Nobody:/:/usr/bin/nologin
# comment:x:1004:1004::/:/bin/bash
broken line
";

    #[test]
    fn lists_real_login_users_only() {
        let users = parse_passwd(PASSWD, (1000, 60000), None);
        let names: Vec<&str> = users.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, vec!["layla", "guest"]);
        assert_eq!(users[0].display, "Layla Nour");
        assert_eq!(users[1].display, "guest");
        assert_eq!(users[0].home, PathBuf::from("/home/layla"));
    }

    #[test]
    fn respects_etc_shells_and_login_defs() {
        let shells = "# /etc/shells\n/bin/bash\n/usr/bin/bash\n";
        let users = parse_passwd(PASSWD, (1000, 60000), Some(shells));
        assert_eq!(users.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), vec!["guest"]);
        assert_eq!(uid_range("# defs\nUID_MIN\t\t\t 500\nUID_MAX 2000\n"), (500, 2000));
        assert_eq!(uid_range(""), (1000, 60000));
    }

    #[test]
    fn initials() {
        let u = |display: &str| User { name: "x".into(), display: display.into(), uid: 1000, home: PathBuf::new(), shell: String::new() };
        assert_eq!(u("Layla Nour").initials(), "LN");
        assert_eq!(u("layla").initials(), "L");
        assert_eq!(u("Ada King Lovelace").initials(), "AL");
        assert_eq!(u("").initials(), "X");
        assert_eq!(u("ليلى نور").initials(), "لن");
    }
}
