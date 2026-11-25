//! ICY (SHOUTcast / Icecast) in-band metadata: with `Icy-MetaData: 1` a
//! server interleaves a metadata block after every `icy-metaint` bytes of
//! audio. [`Demux`] separates the two without allocating; [`stream_title`]
//! reads the now-playing text out of a block.

/// A metadata block is at most 255 × 16 bytes.
pub const MAX_BLOCK: usize = 255 * 16;
/// Largest `icy-metaint` accepted (larger values are treated as broken).
pub const MAX_METAINT: usize = 1 << 20;

/// Where the demuxer is in the interleaving.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    /// Audio bytes left before the next length byte.
    Audio(usize),
    /// Waiting for the length byte.
    Length,
    /// Metadata bytes still to come (of `len`).
    Meta { len: usize, got: usize },
}

/// Splits a response body into audio and metadata blocks.
pub struct Demux {
    metaint: usize,
    phase: Phase,
    block: [u8; MAX_BLOCK],
}

impl Demux {
    /// `metaint`: the server's `icy-metaint` (`None` or 0: no metadata).
    pub fn new(metaint: Option<usize>) -> Self {
        let metaint = metaint.filter(|m| (1..=MAX_METAINT).contains(m)).unwrap_or(0);
        Self { metaint, phase: Phase::Audio(metaint), block: [0; MAX_BLOCK] }
    }

    pub fn has_metadata(&self) -> bool {
        self.metaint != 0
    }

    /// Feeds `input`; `audio` receives each audio run, `meta` each complete
    /// non-empty metadata block. Neither allocates.
    pub fn feed(&mut self, mut input: &[u8], audio: &mut impl FnMut(&[u8]), meta: &mut impl FnMut(&[u8])) {
        if self.metaint == 0 {
            if !input.is_empty() {
                audio(input);
            }
            return;
        }
        while !input.is_empty() {
            match self.phase {
                Phase::Audio(left) => {
                    let n = left.min(input.len());
                    audio(&input[..n]);
                    input = &input[n..];
                    self.phase = if n == left { Phase::Length } else { Phase::Audio(left - n) };
                }
                Phase::Length => {
                    let len = usize::from(input[0]) * 16;
                    input = &input[1..];
                    self.phase = if len == 0 { Phase::Audio(self.metaint) } else { Phase::Meta { len, got: 0 } };
                }
                Phase::Meta { len, got } => {
                    let n = (len - got).min(input.len());
                    self.block[got..got + n].copy_from_slice(&input[..n]);
                    input = &input[n..];
                    if got + n == len {
                        meta(&self.block[..len]);
                        self.phase = Phase::Audio(self.metaint);
                    } else {
                        self.phase = Phase::Meta { len, got: got + n };
                    }
                }
            }
        }
    }
}

/// The `StreamTitle` of a metadata block (`None` when absent or empty).
/// Titles may themselves contain `'`, so the value ends at `';`.
pub fn stream_title(block: &[u8]) -> Option<String> {
    let block = &block[..block.iter().position(|&b| b == 0).unwrap_or(block.len())];
    const KEY: &[u8] = b"StreamTitle='";
    let start = block.windows(KEY.len()).position(|w| w == KEY)? + KEY.len();
    let rest = &block[start..];
    let end = rest.windows(2).position(|w| w == b"';").unwrap_or_else(|| rest.iter().rposition(|&b| b == b'\'').unwrap_or(rest.len()));
    let raw = &rest[..end];
    let text = match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        // Older servers send Latin-1.
        Err(_) => raw.iter().map(|&b| char::from(b)).collect(),
    };
    let text: String = text.chars().filter(|c| !c.is_control()).collect();
    let text = text.trim();
    (!text.is_empty() && text != "-").then(|| text.to_string())
}

/// Splits `"Artist - Title"` (the usual ICY convention) into its parts.
pub fn artist_and_title(title: &str) -> (Option<&str>, &str) {
    match title.split_once(" - ") {
        Some((artist, song)) if !artist.trim().is_empty() && !song.trim().is_empty() => (Some(artist.trim()), song.trim()),
        _ => (None, title.trim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str) -> Vec<u8> {
        let mut b = text.as_bytes().to_vec();
        let padded = b.len().div_ceil(16) * 16;
        b.resize(padded, 0);
        let mut out = vec![(padded / 16) as u8];
        out.extend(b);
        out
    }

    /// A stream of `chunks` audio runs of `metaint` bytes, each followed by
    /// a metadata block (empty for `None`).
    fn stream(metaint: usize, metas: &[Option<&str>]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, m) in metas.iter().enumerate() {
            out.extend(std::iter::repeat_n(i as u8, metaint));
            match m {
                Some(t) => out.extend(block(t)),
                None => out.push(0),
            }
        }
        out
    }

    fn run(demux: &mut Demux, input: &[u8], piece: usize) -> (Vec<u8>, Vec<String>) {
        let (mut audio, mut titles) = (Vec::new(), Vec::new());
        for chunk in input.chunks(piece) {
            demux.feed(chunk, &mut |a| audio.extend_from_slice(a), &mut |m| titles.push(stream_title(m).unwrap_or_default()));
        }
        (audio, titles)
    }

    #[test]
    fn audio_and_metadata_separate_at_any_chunking() {
        let metas = [Some("StreamTitle='Harbor Lights - Low Tide';StreamUrl='';"), None, Some("StreamTitle='Second';")];
        let input = stream(100, &metas);
        let expected_audio: Vec<u8> = (0..3u8).flat_map(|i| std::iter::repeat_n(i, 100)).collect();
        for piece in [1, 7, 16, 99, 100, 101, 4096] {
            let (audio, titles) = run(&mut Demux::new(Some(100)), &input, piece);
            assert_eq!(audio, expected_audio, "piece {piece}");
            assert_eq!(titles, ["Harbor Lights - Low Tide", "Second"], "piece {piece}");
        }
    }

    #[test]
    fn no_metaint_passes_everything_through() {
        let input: Vec<u8> = (0..=255u8).collect();
        for m in [None, Some(0), Some(MAX_METAINT + 1)] {
            let mut d = Demux::new(m);
            assert!(!d.has_metadata());
            let (audio, titles) = run(&mut d, &input, 10);
            assert_eq!(audio, input);
            assert!(titles.is_empty());
        }
    }

    #[test]
    fn the_largest_block_fits() {
        let long = format!("StreamTitle='{}';", "x".repeat(MAX_BLOCK - 15));
        assert_eq!(long.len(), MAX_BLOCK);
        let mut input = vec![1u8; 8];
        input.push(255);
        input.extend(long.as_bytes());
        input.extend([2u8; 8]);
        let (audio, titles) = run(&mut Demux::new(Some(8)), &input, 333);
        assert_eq!(audio.len(), 16);
        assert_eq!(titles[0].len(), MAX_BLOCK - 15);
    }

    #[test]
    fn stream_titles() {
        assert_eq!(stream_title(b"StreamTitle='A - B';StreamUrl='http://x';\0\0\0").as_deref(), Some("A - B"));
        assert_eq!(stream_title(b"StreamTitle='Rock 'n' Roll - It's Me';").as_deref(), Some("Rock 'n' Roll - It's Me"));
        assert_eq!(stream_title(b"StreamTitle='No terminator'").as_deref(), Some("No terminator"));
        assert_eq!(stream_title("StreamTitle='فيروز - نسم علينا الهوى';".as_bytes()).as_deref(), Some("فيروز - نسم علينا الهوى"));
        // Latin-1 from older servers.
        assert_eq!(stream_title(b"StreamTitle='Caf\xe9 del Mar';").as_deref(), Some("Café del Mar"));
        assert_eq!(stream_title(b"StreamTitle='';"), None);
        assert_eq!(stream_title(b"StreamTitle=' - ';"), None);
        assert_eq!(stream_title(b"StreamUrl='x';"), None);
        assert_eq!(stream_title(b"StreamTitle='Line\nbreak';").as_deref(), Some("Linebreak"));
    }

    #[test]
    fn artist_title_split() {
        assert_eq!(artist_and_title("Harbor Lights - Low Tide"), (Some("Harbor Lights"), "Low Tide"));
        assert_eq!(artist_and_title("A - B - C"), (Some("A"), "B - C"));
        assert_eq!(artist_and_title("Just a title"), (None, "Just a title"));
        assert_eq!(artist_and_title(" - Lonely"), (None, "- Lonely"));
    }
}
