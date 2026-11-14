//! A backup's presentation is prepared off the UI thread for the current language.

use gpui::SharedString;
use mandara_i18n::{Lang, t, tf};
use mandara_services::backups::Snapshot;

/// A card stays compact when a repository backs up many source folders.
pub(super) const VISIBLE_PATHS: usize = 3;
pub(super) const ROWS_PER_PAGE: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Format {
    locale: chrono::Locale,
    clock_24h: bool,
}
impl Format {
    pub fn new(config: &mandara_config::ShellConfig, language: Lang) -> Self {
        let locale = chrono::Locale::try_from(config.datetime.time_locale.as_str()).ok()
            .unwrap_or_else(|| chrono::Locale::try_from(language.posix()).unwrap_or(chrono::Locale::en_US));
        Self { locale, clock_24h: config.general.clock_24h }
    }

    fn date<T: chrono::TimeZone>(&self, date: &chrono::DateTime<T>) -> String
    where T::Offset: std::fmt::Display {
        let day = date.format_localized(t("%-d %B %Y"), self.locale);
        let time = date.format_localized(if self.clock_24h { "%H:%M" } else { "%-I:%M %p" }, self.locale);
        format!("{day} · {time}")
    }

    fn timestamp(&self, value: String) -> SharedString {
        chrono::DateTime::parse_from_rfc3339(&value).map(|date| self.date(&date.with_timezone(&chrono::Local)))
            .unwrap_or(value).into()
    }
}

pub(super) struct Row {
    pub identity: String,
    pub restore_id: SharedString,
    pub time: SharedString,
    pub detail: SharedString,
    pub size: SharedString,
    pub files: SharedString,
    pub paths: Vec<SharedString>,
    pub more: Option<SharedString>,
}
impl Row {
    pub fn new(snapshot: Snapshot, language: Lang, format: Format) -> Self {
        let more = snapshot.paths.len().saturating_sub(VISIBLE_PATHS);
        mandara_i18n::with_language(Some(language), || Self {
            restore_id: format!("restore-{}", snapshot.id).into(),
            time: format.timestamp(snapshot.time),
            detail: format!("{} · {}", &snapshot.id[..snapshot.id.len().min(8)], snapshot.hostname).into(),
            size: snapshot.summary.as_ref().and_then(|summary| summary.total_bytes_processed).map(crate::widgets::system::format_bytes).unwrap_or_else(|| t("Size unavailable").into()).into(),
            files: snapshot.summary.as_ref().and_then(|summary| summary.total_files_processed).map(|count| format!("{}: {count}", t("Files"))).unwrap_or_else(|| t("File count unavailable").into()).into(),
            paths: snapshot.paths.into_iter().take(VISIBLE_PATHS).map(Into::into).collect(),
            more: (more > 0).then(|| tf("{count} more folders", &[("count", &more)]).into()),
            identity: snapshot.id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_backup_source_lists_are_presented_compactly_without_losing_the_count() {
        let row = Row::new(Snapshot { id: "abcd".repeat(16), time: "2026-11-13".into(), hostname: "test".into(), paths: vec!["/source".into(); 32], summary: Default::default() }, Lang::En, Format::new(&mandara_config::ShellConfig::default(), Lang::En));
        assert_eq!(row.paths.len(), VISIBLE_PATHS);
        assert!(row.more.unwrap().contains("29"));
        assert_eq!(row.identity.len(), 64);
        assert_eq!(row.files.as_ref(), t("File count unavailable"));
        assert_eq!(row.size.as_ref(), t("Size unavailable"));
    }

    #[test]
    fn backup_dates_follow_the_date_locale_and_clock_format_without_raw_nanoseconds() {
        mandara_i18n::with_language(Some(Lang::En), || {
            let date = chrono::DateTime::parse_from_rfc3339("2026-11-13T23:32:54.814900406+02:00").unwrap();
            let mut config = mandara_config::ShellConfig::default();
            config.general.clock_24h = true;
            let format = Format::new(&config, Lang::En);
            assert_eq!(format.date(&date), "13 November 2026 · 23:32");
            config.general.clock_24h = false;
            assert_eq!(Format::new(&config, Lang::En).date(&date), "13 November 2026 · 11:32 PM");
            config.datetime.time_locale = "fr_FR".into();
            let translated = Format::new(&config, Lang::En);
            assert_ne!(translated.locale, format.locale);
            assert!(translated.date(&date).contains("novembre"));
            assert_eq!(translated.timestamp("unknown date".into()).as_ref(), "unknown date");
        });
    }
}
