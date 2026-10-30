//! `mandara changelog`: what changed in each release, in plain words, from
//! the `CHANGELOG.md` compiled into this binary
//! (`mandara_version::changelog`).

use anyhow::{Result, bail};
use clap::Args;
use mandara_version::changelog::{self, Release};
use std::io::Write as _;

#[derive(Args)]
pub struct ChangelogArgs {
    /// A version (`0.1.1`, or `unreleased`); this build's version when omitted.
    version: Option<String>,
    /// Every release, newest first.
    #[arg(long, conflicts_with = "version")]
    all: bool,
    /// Every release newer than this one, up to this build (what an update brought).
    #[arg(long, value_name = "VERSION", conflicts_with_all = ["version", "all"])]
    since: Option<String>,
    /// Markdown, as written in CHANGELOG.md.
    #[arg(long, conflicts_with = "json")]
    markdown: bool,
    /// Machine-readable.
    #[arg(long)]
    json: bool,
}

/// Widest the text wraps to, even on a wide terminal (easy reading).
const MAX_WIDTH: usize = 88;
/// Used when stdout isn't a terminal.
const DEFAULT_WIDTH: usize = 80;
/// Narrower terminals still get this much.
const MIN_WIDTH: usize = 40;

/// The terminal's width in columns, capped for reading.
fn width() -> usize {
    // SAFETY: TIOCGWINSZ only writes a `winsize` into the struct we pass.
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let cols = if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0 { usize::from(ws.ws_col) } else { DEFAULT_WIDTH };
    cols.clamp(MIN_WIDTH, MAX_WIDTH)
}

fn json(r: &Release) -> serde_json::Value {
    serde_json::json!({
        "version": r.version,
        "date": r.date,
        "summary": r.summary,
        "sections": r.sections.iter().map(|s| serde_json::json!({ "title": s.title, "items": s.items })).collect::<Vec<_>>(),
    })
}

/// The releases the arguments ask for, newest first.
fn chosen<'a>(args: &ChangelogArgs, all: &'a [Release]) -> Result<Vec<&'a Release>> {
    let running = mandara_version::version();
    if args.all {
        return Ok(all.iter().collect());
    }
    if let Some(since) = &args.since {
        // A dev build also shows the notes waiting for the next release.
        let mut list: Vec<&Release> = all.iter().filter(|r| r.is_unreleased() && running.contains('-')).collect();
        list.extend(changelog::between(all, since, running));
        if list.is_empty() {
            bail!("nothing changed between {since} and {running}");
        }
        return Ok(list);
    }
    let found = match &args.version {
        Some(v) => changelog::release(all, v),
        None => changelog::current(all, running),
    };
    match found {
        Some(r) => Ok(vec![r]),
        None => bail!("no changelog entry for {}", args.version.as_deref().unwrap_or(running)),
    }
}

pub fn main(args: ChangelogArgs) -> Result<()> {
    let all = changelog::releases();
    let list = chosen(&args, &all)?;
    // Locked writes that stop quietly on a closed pipe (`| head`).
    let mut out = std::io::stdout().lock();
    if args.json {
        let _ = writeln!(out, "{}", serde_json::to_string_pretty(&list.iter().map(|r| json(r)).collect::<Vec<_>>())?);
        return Ok(());
    }
    let width = width();
    for (i, r) in list.iter().enumerate() {
        let text = if args.markdown {
            let heading = match &r.date {
                Some(date) => format!("## {} — {date}", r.version),
                None => format!("## {}", r.version),
            };
            format!("{heading}\n\n{}\n", r.to_markdown())
        } else {
            r.to_text(width)
        };
        if (i > 0 && writeln!(out).is_err()) || write!(out, "{text}").is_err() {
            return Ok(());
        }
    }
    Ok(())
}

/// A release's notes from GitHub (the update check) for the terminal, or
/// `None` when the release has none.
pub fn update_notes(version: &str, body: &str) -> Option<String> {
    let r = changelog::parse_notes(version, body);
    (!r.sections.is_empty()).then(|| r.to_text(width()))
}
