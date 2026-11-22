//! Stderr follows RUST_LOG; crash diagnostics reach the journal independently.

use tracing::Subscriber;
use tracing_subscriber::{
    EnvFilter, Layer as _, filter::FilterExt as _, filter::LevelFilter, fmt::MakeWriter, layer::SubscriberExt as _,
};

/// Where routine (below WARN) log lines go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Filtered lines go to stderr; the journal only gets WARN and above.
    Stderr,
    /// Filtered lines go to the journal only; stderr stays silent.
    Journal,
}

/// A Linux virtual console (`/dev/ttyN`, `/dev/console`) is a screen the user
/// sees between the greeter and the desktop; a pty or pipe is not.
pub fn is_vt_name(name: &std::path::Path) -> bool {
    let Some(name) = name.to_str() else { return false };
    name == "/dev/console"
        || name
            .strip_prefix("/dev/tty")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Picks the log route from whether stderr is a VT and the journal is reachable.
pub fn route(stderr_is_vt: bool, journal_available: bool) -> Route {
    if stderr_is_vt && journal_available { Route::Journal } else { Route::Stderr }
}

/// Whether stderr (fd 2) is a Linux VT.
pub fn stderr_is_vt() -> bool {
    // SAFETY: isatty only inspects fd 2.
    if unsafe { libc::isatty(2) } != 1 {
        return false;
    }
    std::fs::read_link("/proc/self/fd/2").is_ok_and(|p| is_vt_name(&p))
}

pub fn subscriber<W>(
    stderr: W,
    journal: Option<crate::journal::Journal>,
    filter: EnvFilter,
    route: Route,
) -> impl Subscriber + Send + Sync
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    let to_journal = route == Route::Journal && journal.is_some();
    let (stderr_filter, journal_filter) = if to_journal {
        (EnvFilter::new("off"), filter)
    } else {
        (filter, EnvFilter::new("off"))
    };
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(stderr)
                .with_filter(stderr_filter),
        )
        // WARN and above always reach the journal, whatever the filter says.
        .with(journal.map(|journal| journal.with_filter(journal_filter.or(LevelFilter::WARN))))
}

#[cfg(test)]
mod tests {
    #[test]
    fn stderr_keeps_its_filter_when_the_journal_is_unavailable() {
        let stderr = tempfile::NamedTempFile::new().unwrap();
        let subscriber = super::subscriber(stderr.reopen().unwrap(), None, "info".into(), super::Route::Stderr);
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!("hidden detail");
            tracing::info!("routine message");
            tracing::warn!("warning");
        });
        let text = std::fs::read_to_string(stderr.path()).unwrap();
        assert!(!text.contains("hidden detail"));
        assert!(text.contains("routine message"));
        assert!(text.contains("warning"));
    }

    #[test]
    fn vt_names_are_told_from_ptys_and_files() {
        use std::path::Path;
        assert!(super::is_vt_name(Path::new("/dev/tty1")));
        assert!(super::is_vt_name(Path::new("/dev/tty12")));
        assert!(super::is_vt_name(Path::new("/dev/console")));
        assert!(!super::is_vt_name(Path::new("/dev/pts/0")));
        assert!(!super::is_vt_name(Path::new("/dev/tty")));
        assert!(!super::is_vt_name(Path::new("/dev/ttyS0")));
        assert!(!super::is_vt_name(Path::new("pipe:[123]")));
    }

    #[test]
    fn only_a_vt_with_a_journal_routes_to_the_journal() {
        use super::{Route, route};
        assert_eq!(route(true, true), Route::Journal);
        assert_eq!(route(true, false), Route::Stderr);
        assert_eq!(route(false, true), Route::Stderr);
        assert_eq!(route(false, false), Route::Stderr);
    }
}
