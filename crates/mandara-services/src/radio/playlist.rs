//! Playlist files that point at a stream (`.pls`, `.m3u`), and telling
//! them apart from HLS (`.m3u8` with `#EXT-X-` tags), which isn't played.

/// Largest playlist body read (bytes).
pub const MAX_PLAYLIST_BYTES: usize = 64 * 1024;
/// Entries kept from one playlist (the rest are ignored).
pub const MAX_ENTRIES: usize = 8;

/// What a playlist body holds.
#[derive(Debug, PartialEq, Eq)]
pub enum Playlist {
    /// Stream addresses, in the playlist's order.
    Streams(Vec<String>),
    /// An HLS media or master playlist.
    Hls,
}

/// Whether a response looks like a playlist rather than audio, from its
/// content type or (when the server says nothing useful) its path.
pub fn is_playlist(content_type: Option<&str>, path: &str) -> bool {
    match content_type {
        Some("audio/x-scpls" | "audio/scpls" | "application/pls+xml" | "audio/x-mpegurl" | "audio/mpegurl" | "application/x-mpegurl" | "application/vnd.apple.mpegurl" | "application/mpegurl") => true,
        Some(ct) if ct.starts_with("audio/") || ct == "application/ogg" => false,
        _ => {
            let path = path.split(['?', '#']).next().unwrap_or(path);
            let ext = path.rsplit('.').next().unwrap_or("");
            ["pls", "m3u", "m3u8"].iter().any(|e| ext.eq_ignore_ascii_case(e))
        }
    }
}

/// Parses a `.pls` or `.m3u` / `.m3u8` body. Relative entries resolve
/// against `base`; only http(s) entries are kept.
pub fn parse(body: &str, base: &str) -> Playlist {
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    if body.lines().any(|l| l.trim_start().starts_with("#EXT-X-")) {
        return Playlist::Hls;
    }
    let pls = body.lines().map(str::trim).find(|l| !l.is_empty()).is_some_and(|l| l.eq_ignore_ascii_case("[playlist]"));
    let mut out = Vec::new();
    for line in body.lines().map(str::trim) {
        if out.len() >= MAX_ENTRIES {
            break;
        }
        let entry = if pls {
            // FileN=<url>
            match line.split_once('=').map(|(k, v)| (k.trim(), v)) {
                Some((key, value)) if key.len() > 4 && key[..4].eq_ignore_ascii_case("file") && key[4..].bytes().all(|b| b.is_ascii_digit()) => value.trim(),
                _ => continue,
            }
        } else {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            line
        };
        if let Some(url) = resolve(base, entry)
            && !out.contains(&url)
        {
            out.push(url);
        }
    }
    Playlist::Streams(out)
}

/// `entry` as an absolute http(s) address (relative to `base`).
pub fn resolve(base: &str, entry: &str) -> Option<String> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    let lower = entry.get(..8).unwrap_or(entry).to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(entry.to_string());
    }
    if entry.contains("://") {
        return None;
    }
    let scheme_end = base.find("://")? + 3;
    let host_end = base[scheme_end..].find('/').map_or(base.len(), |i| scheme_end + i);
    if let Some(rest) = entry.strip_prefix("//") {
        return Some(format!("{}{}", &base[..scheme_end], rest));
    }
    if entry.starts_with('/') {
        return Some(format!("{}{}", &base[..host_end], entry));
    }
    let path = base[host_end..].split(['?', '#']).next().unwrap_or("");
    let dir = path.rfind('/').map_or("/", |i| &path[..=i]);
    Some(format!("{}{}{}", &base[..host_end], if dir.is_empty() { "/" } else { dir }, entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pls_files() {
        let body = "[playlist]\nNumberOfEntries=2\nFile1=https://a.example/stream\nTitle1=A\nLength1=-1\nfile2=http://b.example:8000/live\nVersion=2\n";
        assert_eq!(parse(body, "https://x.example/list.pls"), Playlist::Streams(vec!["https://a.example/stream".into(), "http://b.example:8000/live".into()]));
        // Windows line endings, a byte-order mark, odd spacing.
        let crlf = "\u{feff}[Playlist]\r\nFile1 = https://a.example/s\r\n";
        assert_eq!(parse(crlf, "https://x.example/"), Playlist::Streams(vec!["https://a.example/s".into()]));
        // Keys that only start with "File" are not entries.
        assert_eq!(parse("[playlist]\nFileName=https://no.example\n", "https://x.example/"), Playlist::Streams(vec![]));
    }

    #[test]
    fn m3u_files() {
        let body = "#EXTM3U\n#EXTINF:-1,Station\nhttps://a.example/stream.mp3\n\n# comment\nrelative/stream.aac\n/abs/path\nhttps://a.example/stream.mp3\nftp://nope.example/x\n";
        assert_eq!(
            parse(body, "https://x.example/lists/radio.m3u?token=1"),
            Playlist::Streams(vec!["https://a.example/stream.mp3".into(), "https://x.example/lists/relative/stream.aac".into(), "https://x.example/abs/path".into()])
        );
        assert_eq!(parse("http://only.example/one\n", "https://x.example/a.m3u"), Playlist::Streams(vec!["http://only.example/one".into()]));
    }

    #[test]
    fn hls_is_recognised_not_played() {
        assert_eq!(parse("#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:10\nseg1.aac\n", "https://x.example/a.m3u8"), Playlist::Hls);
        assert_eq!(parse("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=64000\nchunks.m3u8\n", "https://x.example/master.m3u8"), Playlist::Hls);
    }

    #[test]
    fn playlists_are_bounded() {
        let body: String = (0..100).map(|i| format!("https://a.example/{i}\n")).collect();
        let Playlist::Streams(entries) = parse(&body, "https://x.example/") else { panic!() };
        assert_eq!(entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn telling_playlists_from_audio() {
        assert!(is_playlist(Some("audio/x-scpls"), "/listen"));
        assert!(is_playlist(Some("application/vnd.apple.mpegurl"), "/x"));
        assert!(!is_playlist(Some("audio/mpeg"), "/radio.pls"), "the content type wins");
        assert!(!is_playlist(Some("application/ogg"), "/radio.m3u"));
        assert!(is_playlist(Some("text/plain"), "/radio.PLS?x=1"));
        assert!(is_playlist(None, "/a/b/listen.m3u"));
        assert!(!is_playlist(None, "/stream"));
        assert!(!is_playlist(Some("application/octet-stream"), "/live"));
    }

    #[test]
    fn relative_addresses() {
        assert_eq!(resolve("https://h.example/a/b.pls", "c.mp3").as_deref(), Some("https://h.example/a/c.mp3"));
        assert_eq!(resolve("https://h.example", "c.mp3").as_deref(), Some("https://h.example/c.mp3"));
        assert_eq!(resolve("https://h.example/a/b.pls", "//cdn.example/s").as_deref(), Some("https://cdn.example/s"));
        assert_eq!(resolve("http://h.example:8000/a/", "/s").as_deref(), Some("http://h.example:8000/s"));
        assert_eq!(resolve("https://h.example/", "rtsp://x/y"), None);
        assert_eq!(resolve("https://h.example/", "   "), None);
    }
}
