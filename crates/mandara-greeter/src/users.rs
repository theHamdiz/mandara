//! Login users from `/etc/passwd` and their avatars.

use std::path::{Path, PathBuf};
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

/// The largest avatar file read. A user chooses what `~/.face` is, and the
/// login screen (a different user) reads it, so it is bounded: pictures this
/// large are not avatars.
const MAX_AVATAR_BYTES: u64 = 8 * 1024 * 1024;
/// The widest and tallest avatar picture decoded (pixels), so a small file
/// can't expand into an enormous image.
const MAX_AVATAR_SIDE: u32 = 4096;
/// The most memory one avatar decode may allocate: a full
/// [`MAX_AVATAR_SIDE`] square of 4-byte pixels, doubled for the decoder's
/// working copies.
const MAX_AVATAR_DECODE_BYTES: u64 = 2 * 4 * (MAX_AVATAR_SIDE as u64) * (MAX_AVATAR_SIDE as u64);

/// Loads, center-crops and downsizes the first readable avatar into a
/// square BGRA image of `size` pixels. Reads and decodes, which can take a
/// while on a large picture: call it off the UI thread.
pub fn load_avatar(user: &User, size: u32) -> Option<Arc<RenderImage>> {
    avatar_paths(user).into_iter().find_map(|p| decode_avatar(&p, size))
}

/// The bytes of the avatar file at `path`: a regular file of at most
/// [`MAX_AVATAR_BYTES`]. Opened without waiting (a FIFO named `~/.face`
/// would block the open forever) and checked after opening, so a link to
/// `/dev/zero` or a device is refused without being read.
fn read_avatar(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_AVATAR_BYTES {
        return None;
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    // One byte more than allowed: a file that grew after the check is refused.
    file.take(MAX_AVATAR_BYTES + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_AVATAR_BYTES).then_some(bytes)
}

/// Decodes `bytes` within the avatar limits, crops it to a centred square
/// and resizes it to `size` pixels (BGRA).
fn decode_avatar_bytes(bytes: &[u8], size: u32) -> Option<Arc<RenderImage>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_AVATAR_SIDE);
    limits.max_image_height = Some(MAX_AVATAR_SIDE);
    limits.max_alloc = Some(MAX_AVATAR_DECODE_BYTES);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let side = img.width().min(img.height());
    let cropped = img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side);
    let mut rgba = cropped.resize_exact(size, size, image::imageops::FilterType::Lanczos3).to_rgba8();
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)])))
}

fn decode_avatar(path: &Path, size: u32) -> Option<Arc<RenderImage>> {
    decode_avatar_bytes(&read_avatar(path)?, size)
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

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::new_rgba8(width, height).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn an_ordinary_avatar_is_decoded_and_squared() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".face");
        std::fs::write(&file, png(300, 200)).unwrap();
        let img = decode_avatar(&file, 64).expect("a normal picture loads");
        assert_eq!(img.size(0), gpui::size(gpui::DevicePixels(64), gpui::DevicePixels(64)));
    }

    #[test]
    fn avatars_that_are_not_small_regular_pictures_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        // Too many pixels for a tiny file (a decompression bomb's shape).
        let wide = dir.path().join("wide.png");
        std::fs::write(&wide, png(MAX_AVATAR_SIDE + 1, 1)).unwrap();
        assert!(decode_avatar(&wide, 64).is_none(), "wider than the limit");
        // Too large a file.
        let big = dir.path().join("big");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(MAX_AVATAR_BYTES + 1).unwrap();
        assert!(read_avatar(&big).is_none(), "over the size cap");
        // Not a picture, a device, a FIFO (opening it must not block).
        let text = dir.path().join("text");
        std::fs::write(&text, "not an image").unwrap();
        assert!(decode_avatar(&text, 64).is_none());
        assert!(read_avatar(Path::new("/dev/zero")).is_none(), "a device");
        let fifo = dir.path().join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: `c` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(read_avatar(&fifo).is_none(), "a FIFO");
        // A link to a regular file is how `~/.face` usually points at a picture.
        let target = dir.path().join("pic.png");
        std::fs::write(&target, png(8, 8)).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(decode_avatar(&link, 16).is_some());
        std::os::unix::fs::symlink("/dev/zero", dir.path().join("zero")).unwrap();
        assert!(read_avatar(&dir.path().join("zero")).is_none(), "a link to a device");
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
