//! Stderr follows RUST_LOG; crash diagnostics reach the journal independently.

use tracing::Subscriber;
use tracing_subscriber::{
    EnvFilter, Layer as _, filter::LevelFilter, fmt::MakeWriter, layer::SubscriberExt as _,
};

pub fn subscriber<W>(
    stderr: W,
    journal: Option<crate::journal::Journal>,
    filter: EnvFilter,
) -> impl Subscriber + Send + Sync
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(stderr)
                .with_filter(filter),
        )
        .with(journal.map(|journal| journal.with_filter(LevelFilter::WARN)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn stderr_keeps_its_filter_when_the_journal_is_unavailable() {
        let stderr = tempfile::NamedTempFile::new().unwrap();
        let subscriber = super::subscriber(stderr.reopen().unwrap(), None, "info".into());
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
}
