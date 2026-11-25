//! App metadata helpers: icons (kind and pixel size from the bytes),
//! AppStream fields, and desktop entries (reading, rewriting `Exec=` /
//! `Icon=` to the installed copies, synthesizing one when an archive has
//! none).

use std::path::Path;

/// `‰PNG\r\n\x1a\n`.
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
/// The IHDR width / height are big-endian u32s at these offsets.
const PNG_WIDTH_AT: usize = 16;
const PNG_HEIGHT_AT: usize = 20;
const XPM_MAGIC: &[u8] = b"/* XPM */";
const GZIP_MAGIC: &[u8] = b"\x1f\x8b";
/// Bytes searched for `<svg`.
const SVG_SNIFF_BYTES: usize = 1024;
/// hicolor sizes Mandara installs into (the nearest one at or above the
/// icon's size; anything larger goes to the biggest).
pub const HICOLOR_SIZES: &[u32] = &[16, 22, 24, 32, 48, 64, 96, 128, 192, 256, 512];
const DESKTOP_GROUP: &str = "[Desktop Entry]";
const ACTION_GROUP_PREFIX: &str = "[Desktop Action ";
/// Longest app id kept.
const MAX_ID_LEN: usize = 96;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IconKind {
    Png,
    Svg,
    Svgz,
    Xpm,
}

impl IconKind {
    pub fn extension(self) -> &'static str {
        match self {
            IconKind::Png => "png",
            IconKind::Svg => "svg",
            IconKind::Svgz => "svgz",
            IconKind::Xpm => "xpm",
        }
    }
}

/// An icon's bytes with its kind and (raster) size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconData {
    pub bytes: Vec<u8>,
    pub kind: IconKind,
    /// The larger side in pixels (raster only).
    pub size: Option<u32>,
}

impl IconData {
    /// Recognizes PNG / SVG / SVGZ / XPM bytes; `None` for anything else.
    pub fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        if bytes.starts_with(PNG_MAGIC) {
            let be = |at: usize| bytes.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_be_bytes);
            let size = be(PNG_WIDTH_AT).zip(be(PNG_HEIGHT_AT)).map(|(w, h)| w.max(h));
            return Some(Self { bytes, kind: IconKind::Png, size });
        }
        if bytes.starts_with(XPM_MAGIC) {
            return Some(Self { bytes, kind: IconKind::Xpm, size: None });
        }
        if bytes.starts_with(GZIP_MAGIC) {
            return Some(Self { bytes, kind: IconKind::Svgz, size: None });
        }
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(SVG_SNIFF_BYTES)]).to_ascii_lowercase();
        head.contains("<svg").then_some(Self { bytes, kind: IconKind::Svg, size: None })
    }

    /// `hicolor/<dir>/apps`: `scalable` for vectors, else the nearest size.
    pub fn hicolor_dir(&self) -> String {
        match (self.kind, self.size) {
            (IconKind::Svg | IconKind::Svgz, _) => "scalable".into(),
            (_, Some(px)) => {
                let s = HICOLOR_SIZES.iter().copied().find(|s| *s >= px).unwrap_or(HICOLOR_SIZES[HICOLOR_SIZES.len() - 1]);
                format!("{s}x{s}")
            }
            // Unknown raster size: the common 256 px.
            (_, None) => "256x256".into(),
        }
    }
}

/// AppStream fields Mandara shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppStream {
    pub id: String,
    pub name: String,
    pub summary: String,
    /// The newest `<release version=…>`.
    pub version: String,
    /// `<description>` as plain text: paragraphs and `• ` list items, one
    /// per line (markup stripped).
    pub description: String,
    /// `<developer><name>` (or the older `<developer_name>`).
    pub developer: String,
    /// `<categories><category>…`.
    pub categories: Vec<String>,
    /// `<screenshot><image>` values (URLs or paths), the default first.
    pub screenshots: Vec<String>,
}

/// Screenshots read from metadata (the sheet shows at most this many).
pub const MAX_SCREENSHOTS: usize = 3;
/// Longest description kept (characters).
const MAX_DESCRIPTION_CHARS: usize = 4000;

/// The raw inner text of the first `<tag>` or `<tag …>` (not `xml:lang`
/// translations), unparsed.
fn raw_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut from = 0;
    while let Some(at) = xml[from..].find(&open) {
        let start = from + at;
        let rest = &xml[start + open.len()..];
        let head_end = rest.find('>')?;
        let head = &rest[..head_end];
        from = start + open.len();
        // `<tagx>` is another element; translations are skipped.
        if !(head.is_empty() || head.starts_with(char::is_whitespace)) || head.contains("xml:lang") || head.ends_with('/') {
            continue;
        }
        let body = &rest[head_end + 1..];
        return Some(&body[..body.find(&close)?]);
    }
    None
}

/// Every top-level `<tag>` text inside `xml` (translations skipped).
fn all_tags(xml: &str, tag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(body) = raw_tag(rest, tag) {
        let text = unescape(body.trim());
        if !text.is_empty() {
            out.push(text);
        }
        let consumed = body.as_ptr() as usize - rest.as_ptr() as usize + body.len();
        rest = &rest[consumed..];
    }
    out
}

/// AppStream / HTML-ish markup as plain text: `<p>` paragraphs and `<li>`
/// items (`• `) on their own lines, other tags dropped, whitespace folded.
pub fn strip_markup(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    // Line breaks come from the markup only; source newlines are spaces.
    let push = |out: &mut String, text: &str| out.extend(text.chars().map(|c| if c == '\n' { ' ' } else { c }));
    while let Some(lt) = rest.find('<') {
        push(&mut out, &rest[..lt]);
        let Some(gt) = rest[lt..].find('>') else { break };
        let tag = rest[lt + 1..lt + gt].trim().to_ascii_lowercase();
        let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or_default().to_string();
        // Translations (`<p xml:lang="ar">…</p>`) are skipped whole.
        if tag.contains("xml:lang") && !tag.ends_with('/') {
            let close = format!("</{name}>");
            let after = &rest[lt + gt + 1..];
            rest = after.find(&close).map_or("", |e| &after[e + close.len()..]);
            continue;
        }
        let name = name.as_str();
        match (name, tag.starts_with('/')) {
            ("p" | "ul" | "ol" | "br", _) | ("li", true) => out.push('\n'),
            ("li", false) => out.push_str("\n• "),
            _ => {}
        }
        rest = &rest[lt + gt + 1..];
    }
    push(&mut out, rest);
    let text = unescape(&out);
    let lines: Vec<String> = text.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty() && l != "•").collect();
    let mut joined = lines.join("\n");
    if joined.chars().count() > MAX_DESCRIPTION_CHARS {
        joined = joined.chars().take(MAX_DESCRIPTION_CHARS).collect::<String>() + "…";
    }
    joined
}

/// The text of the first `<tag>` without attributes (`xml:lang` variants
/// skipped), unescaped.
fn plain_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    let text = unescape(xml[start..end].trim());
    (!text.is_empty()).then_some(text)
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Reads an AppStream component (a tiny tag scan; no XML library).
pub fn parse_appstream(xml: &str) -> Option<AppStream> {
    if !xml.contains("<component") {
        return None;
    }
    let id = plain_tag(xml, "id").map(|i| i.trim_end_matches(".desktop").to_string()).unwrap_or_default();
    let version = xml
        .find("<release ")
        .and_then(|at| {
            let rest = &xml[at..];
            let tag = &rest[..rest.find('>')?];
            let v = tag.find("version=\"")? + "version=\"".len();
            Some(tag[v..v + tag[v..].find('"')?].to_string())
        })
        .unwrap_or_default();
    // The component's own `<name>` comes before `<developer><name>`.
    let developer_block = raw_tag(xml, "developer");
    let without_developer = developer_block.map_or_else(|| xml.to_string(), |d| xml.replacen(d, "", 1));
    let developer = developer_block.and_then(|d| plain_tag(d, "name")).or_else(|| plain_tag(xml, "developer_name")).unwrap_or_default();
    let mut screenshots = Vec::new();
    if let Some(block) = raw_tag(xml, "screenshots") {
        // `type="default"` first, then the rest in order.
        let mut shots: Vec<(bool, &str)> = Vec::new();
        let mut rest = block;
        while let Some(at) = rest.find("<screenshot") {
            let after = &rest[at..];
            let end = after.find("</screenshot>").map_or(after.len(), |e| e + "</screenshot>".len());
            let one = &after[..end];
            shots.push((one[..one.find('>').unwrap_or(0)].contains("default"), one));
            rest = &after[end..];
        }
        shots.sort_by_key(|(default, _)| !default);
        for (_, one) in shots {
            if let Some(image) = all_tags(one, "image").into_iter().next() {
                screenshots.push(image);
            }
            if screenshots.len() == MAX_SCREENSHOTS {
                break;
            }
        }
    }
    Some(AppStream {
        id,
        name: plain_tag(&without_developer, "name").unwrap_or_default(),
        summary: plain_tag(xml, "summary").unwrap_or_default(),
        version,
        description: raw_tag(xml, "description").map(strip_markup).unwrap_or_default(),
        developer,
        categories: raw_tag(xml, "categories").map(|c| all_tags(c, "category")).unwrap_or_default(),
        screenshots,
    })
}

/// `key=` in the `[Desktop Entry]` group (no locale variants).
pub fn desktop_value(text: &str, key: &str) -> Option<String> {
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == DESKTOP_GROUP;
            continue;
        }
        if in_entry && let Some((k, v)) = line.split_once('=') && k.trim() == key {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// Splits an `Exec=` line into its program and the rest (quotes kept in
/// the rest; leading `env VAR=x` assignments skipped).
pub fn split_exec(exec: &str) -> (String, String) {
    let mut words: Vec<(String, usize)> = Vec::new();
    let (mut cur, mut quoted, mut in_word) = (String::new(), false, false);
    let chars: Vec<(usize, char)> = exec.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            '\\' if quoted && i + 1 < chars.len() => {
                cur.push(chars[i + 1].1);
                i += 1;
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    words.push((std::mem::take(&mut cur), at));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
        i += 1;
    }
    if in_word {
        words.push((cur, exec.len()));
    }
    let mut idx = 0;
    if words.first().is_some_and(|(w, _)| w == "env") {
        idx = 1;
        while words.get(idx).is_some_and(|(w, _)| w.contains('=') && !w.starts_with('-')) {
            idx += 1;
        }
    }
    let Some((program, end)) = words.get(idx).cloned() else { return (String::new(), String::new()) };
    (program, exec[end..].trim().to_string())
}

/// Quotes a path for `Exec=` (desktop entry spec: double quotes, with `"`,
/// `` ` ``, `$` and `\` escaped) when it needs it.
pub fn quote_exec_arg(arg: &str) -> String {
    const RESERVED: &[char] = &[' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`'];
    if !arg.contains(RESERVED) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    // A desktop-file string value escapes backslashes once more.
    out.replace('\\', "\\\\")
}

/// Rewrites a packaged desktop entry for the installed copy: every `Exec=`
/// (main entry and actions) runs `program`, `TryExec=` goes, `Icon=` names
/// the installed icon, and Mandara's markers are added. Other groups and
/// keys are kept as they are.
pub fn rewrite_desktop(text: &str, program: &Path, icon: Option<&str>, markers: &[(&str, &str)]) -> String {
    let quoted = quote_exec_arg(&program.to_string_lossy());
    let mut out = String::new();
    let mut group = String::new();
    let mut seen_entry = false;
    let mut keep_group = false;
    let flush_markers = |out: &mut String| {
        for (k, v) in markers {
            out.push_str(&format!("{k}={v}\n"));
        }
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if group == DESKTOP_GROUP {
                flush_markers(&mut out);
            }
            group = trimmed.to_string();
            keep_group = group == DESKTOP_GROUP || group.starts_with(ACTION_GROUP_PREFIX);
            if !keep_group {
                continue;
            }
            if group == DESKTOP_GROUP {
                seen_entry = true;
            }
            if !out.is_empty() && !out.ends_with("\n\n") {
                out.push('\n');
            }
            out.push_str(trimmed);
            out.push('\n');
            continue;
        }
        if !keep_group {
            continue;
        }
        let key = trimmed.split_once('=').map(|(k, _)| k.trim()).unwrap_or_default();
        match key {
            "Exec" => {
                let value = trimmed.split_once('=').map(|(_, v)| v.trim()).unwrap_or_default();
                let (_, rest) = split_exec(value);
                out.push_str(&format!("Exec={quoted}{}{rest}\n", if rest.is_empty() { "" } else { " " }));
            }
            "TryExec" | "DBusActivatable" => {}
            "Icon" if icon.is_some() && group == DESKTOP_GROUP => out.push_str(&format!("Icon={}\n", icon.unwrap_or_default())),
            k if markers.iter().any(|(m, _)| *m == k) => {}
            _ if trimmed.is_empty() => {}
            _ => {
                out.push_str(trimmed);
                out.push('\n');
            }
        }
    }
    if group == DESKTOP_GROUP {
        flush_markers(&mut out);
    }
    if !seen_entry {
        return String::new();
    }
    // An entry without an icon line gets one.
    if let Some(icon) = icon
        && desktop_value(&out, "Icon").is_none()
    {
        out = out.replacen(DESKTOP_GROUP, &format!("{DESKTOP_GROUP}\nIcon={icon}"), 1);
    }
    out
}

/// A desktop entry for an app that came without one.
pub fn synthesize_desktop(name: &str, program: &Path, icon: Option<&str>, comment: &str, markers: &[(&str, &str)]) -> String {
    let mut out = format!("{DESKTOP_GROUP}\nType=Application\nName={name}\nExec={}\n", quote_exec_arg(&program.to_string_lossy()));
    if !comment.is_empty() {
        out.push_str(&format!("Comment={comment}\n"));
    }
    if let Some(icon) = icon {
        out.push_str(&format!("Icon={icon}\n"));
    }
    out.push_str("Terminal=false\nCategories=Utility;\n");
    for (k, v) in markers {
        out.push_str(&format!("{k}={v}\n"));
    }
    out
}

/// A safe app id: letters, digits, `.`, `-`, `_` (others become `-`),
/// trimmed, at most [`MAX_ID_LEN`] chars; `None` when nothing is left.
pub fn sanitize_id(raw: &str) -> Option<String> {
    let mapped: String = raw.trim().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '-' }).collect();
    let trimmed = mapped.trim_matches(|c| matches!(c, '.' | '-' | '_')).chars().take(MAX_ID_LEN).collect::<String>();
    // Never `.` / `..` or a hidden name.
    (!trimmed.is_empty() && !trimmed.starts_with('.')).then_some(trimmed)
}

/// A command name for the PATH shim: lowercase id-safe characters.
pub fn shim_name(raw: &str) -> Option<String> {
    let base = raw.rsplit('/').next().unwrap_or(raw);
    let base = base.strip_suffix(".AppImage").or_else(|| base.strip_suffix(".appimage")).unwrap_or(base);
    let lower: String = base.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+') { c } else { '-' }).collect();
    sanitize_id(&lower)
}

/// Drops a trailing version and architecture from a file stem
/// (`Obsidian-1.6.7-x86_64` → `Obsidian`).
pub fn strip_version_suffix(stem: &str) -> String {
    let mut cut = stem.len();
    for (i, c) in stem.char_indices() {
        if (c == '-' || c == '_' || c == '.') && stem[i + 1..].chars().next().is_some_and(|n| n.is_ascii_digit() || n == 'v') && stem[..i].chars().any(|c| c.is_alphabetic()) {
            let next = &stem[i + 1..];
            let digit_after_v = next.strip_prefix('v').is_none_or(|r| r.starts_with(|c: char| c.is_ascii_digit()));
            if digit_after_v {
                cut = i;
                break;
            }
        }
    }
    stem[..cut].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_rewrites_keep_field_codes_and_quote_paths() {
        let text = "[Desktop Entry]\nName=Hello\nExec=env FOO=1 hello --flag %U\nTryExec=hello\nIcon=hello\n\n[Desktop Action new]\nName=New\nExec=hello --new\n\n[X-Other]\nExec=evil\n";
        let out = rewrite_desktop(text, Path::new("/home/me/My Apps/hello/Hello.AppImage"), Some("mandara-hello"), &[("X-Mandara-App-Id", "hello")]);
        assert!(out.contains("Exec=\"/home/me/My Apps/hello/Hello.AppImage\" --flag %U\n"), "{out}");
        assert!(out.contains("Exec=\"/home/me/My Apps/hello/Hello.AppImage\" --new\n"), "{out}");
        assert!(!out.contains("TryExec") && !out.contains("evil") && !out.contains("X-Other"));
        assert!(out.contains("Icon=mandara-hello\n") && out.contains("X-Mandara-App-Id=hello\n"));
    }

    #[test]
    fn splits_exec_lines() {
        assert_eq!(split_exec("\"/opt/My App/app\" --x %f"), ("/opt/My App/app".into(), "--x %f".into()));
        assert_eq!(split_exec("env A=1 B=2 app"), ("app".into(), String::new()));
    }

    #[test]
    fn ids_and_shims_are_safe() {
        assert_eq!(sanitize_id("org.Example.App"), Some("org.Example.App".into()));
        assert_eq!(sanitize_id("../../etc"), Some("etc".into()));
        assert_eq!(sanitize_id("..."), None);
        assert_eq!(shim_name("Obsidian.AppImage"), Some("obsidian".into()));
        assert_eq!(strip_version_suffix("Obsidian-1.6.7-x86_64"), "Obsidian");
        assert_eq!(strip_version_suffix("tool-v2.0"), "tool");
        assert_eq!(strip_version_suffix("vlc"), "vlc");
    }

    #[test]
    fn icon_sizes_map_to_hicolor() {
        let png = |w: u32| {
            let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
            b.extend_from_slice(&w.to_be_bytes());
            b.extend_from_slice(&w.to_be_bytes());
            IconData::from_bytes(b).unwrap()
        };
        assert_eq!(png(48).hicolor_dir(), "48x48");
        assert_eq!(png(100).hicolor_dir(), "128x128");
        assert_eq!(png(1024).hicolor_dir(), "512x512");
        assert_eq!(IconData::from_bytes(b"<?xml?><svg/>".to_vec()).unwrap().hicolor_dir(), "scalable");
        assert!(IconData::from_bytes(b"nope".to_vec()).is_none());
    }

    #[test]
    fn appstream_description_developer_categories_screenshots() {
        let xml = r#"<component type="desktop-application"><id>org.x.Y</id><name>Y</name>
<developer id="org.x"><name>X Labs</name></developer><summary>S</summary>
<description><p>Fast &amp; <em>light</em>
   editor.</p><p xml:lang="ar">ترجمة</p><ul><li>Tabs</li><li>Themes</li></ul></description>
<categories><category>Office</category><category>TextEditor</category></categories>
<screenshots><screenshot><image type="thumbnail">usr/share/y/thumb.png</image></screenshot>
<screenshot type="default"><caption>Main</caption><image>usr/share/y/main.png</image></screenshot></screenshots></component>"#;
        let a = parse_appstream(xml).unwrap();
        assert_eq!(a.name, "Y");
        assert_eq!(a.developer, "X Labs");
        assert_eq!(a.description, "Fast & light editor.\n• Tabs\n• Themes");
        assert_eq!(a.categories, vec!["Office", "TextEditor"]);
        assert_eq!(a.screenshots, vec!["usr/share/y/main.png", "usr/share/y/thumb.png"]);
        let old = parse_appstream("<component><developer_name>Old Co</developer_name><name>Z</name></component>").unwrap();
        assert_eq!((old.developer.as_str(), old.name.as_str()), ("Old Co", "Z"));
        assert_eq!(strip_markup("<p>a</p>\n\n<p>  b  c </p>"), "a\nb c");
    }

    #[test]
    fn appstream_fields() {
        let xml = "<component><id>org.x.Y.desktop</id><name xml:lang=\"ar\">ي</name><name>Y &amp; Co</name><summary>S</summary><releases><release version=\"2.0\"/><release version=\"1.0\"/></releases></component>";
        let a = parse_appstream(xml).unwrap();
        assert_eq!((a.id.as_str(), a.name.as_str(), a.version.as_str()), ("org.x.Y", "Y & Co", "2.0"));
    }
}
