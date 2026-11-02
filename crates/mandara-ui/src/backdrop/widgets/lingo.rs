//! The language-dependent text of a desktop widget: which digits it
//! writes, which calendar its dates are in, and the language of its day
//! and month names and words ([`mandara_config::desktop_widgets::TextOptions`]).
//!
//! A widget with no language of its own follows the interface (and
//! Studio's date language); one with a language writes its names and words
//! in it and reads in its direction. Digits follow the language unless
//! the widget chooses them.

use chrono::NaiveDate;
use mandara_config::desktop_widgets::{Calendar, TextOptions};
use mandara_i18n::Lang;

use crate::widgets::clock::{arabic_numerals, localized};

/// Today's Hijri date `(year, month 1–12, day)`; the Islamic day begins at
/// Maghrib, so after sunset it is already tomorrow's.
pub type HijriDate = (i64, usize, i64);

/// A widget's resolved language settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Lingo {
    /// `None`: the interface language (and the date language override).
    pub lang: Option<Lang>,
    pub rtl: bool,
    /// Eastern Arabic digits.
    pub eastern: bool,
    pub calendar: Calendar,
    pub hijri_in_arabic: bool,
}

impl Lingo {
    /// `opts` resolved for an interface reading right to left when
    /// `ui_rtl`; `eastern` overrides the digits (a clock face's own rule).
    pub fn of(opts: &TextOptions, ui_rtl: bool, eastern: Option<bool>) -> Self {
        let lang = Lang::from_code(opts.language.trim()).filter(|_| !opts.language.trim().is_empty());
        let rtl = lang.map_or(ui_rtl, Lang::is_rtl);
        let arabic = lang.map_or(mandara_i18n::language() == Lang::Ar, |l| l == Lang::Ar);
        Self { lang, rtl, eastern: eastern.unwrap_or_else(|| opts.numerals.eastern(arabic)), calendar: opts.calendar, hijri_in_arabic: opts.hijri_in_arabic }
    }

    /// [`Lingo::of`] for a plugin's widget: the widget's own language, else
    /// the plugin's (`plugin`), else the interface's.
    pub fn of_plugin(opts: &TextOptions, ui_rtl: bool, plugin: Option<Lang>) -> Self {
        match plugin.filter(|_| opts.language.trim().is_empty()) {
            Some(l) => Self::of(&TextOptions { language: l.code().into(), ..opts.clone() }, ui_rtl, None),
            None => Self::of(opts, ui_rtl, None),
        }
    }

    /// Whether the widget's language is Arabic.
    pub fn arabic(&self) -> bool {
        self.lang.map_or(mandara_i18n::language() == Lang::Ar, |l| l == Lang::Ar)
    }

    /// `s` with its digits in the widget's numerals.
    pub fn digits(&self, s: impl Into<String>) -> String {
        let s = s.into();
        if self.eastern { arabic_numerals(&s) } else { s }
    }

    /// `key` (an English catalog key) in the widget's language.
    pub fn word(&self, key: &str) -> String {
        match self.lang {
            None => mandara_i18n::tr(key),
            Some(Lang::En) => key.to_string(),
            Some(l) => mandara_i18n::catalog(l).get(key).unwrap_or(key).to_string(),
        }
    }

    /// `date` formatted with a strftime `pattern` (a catalog key, so the
    /// order of the parts follows the language), names in the widget's
    /// language, digits in its numerals.
    pub fn date(&self, date: &NaiveDate, pattern: &str) -> String {
        let text = match self.lang {
            None => localized(date, &mandara_i18n::tr(pattern)),
            Some(l) => date.format_localized(&self.word(pattern), chrono::Locale::try_from(l.posix()).unwrap_or(chrono::Locale::en_US)).to_string(),
        };
        self.digits(text)
    }

    /// The Hijri month's name.
    pub fn hijri_month(&self, month: usize) -> String {
        let (latin, arabic) = crate::plugins::prayer::HIJRI_MONTHS[month.clamp(1, crate::plugins::prayer::HIJRI_MONTHS.len()) - 1];
        if self.hijri_in_arabic || self.arabic() { arabic.to_string() } else { self.word(latin) }
    }

    /// Hijri digits: Eastern when the date is written in Arabic.
    pub fn hijri_digits(&self, n: i64) -> String {
        if self.hijri_in_arabic { arabic_numerals(&n.to_string()) } else { self.digits(n.to_string()) }
    }

    /// `5 Jumada al-Ula` (Arabic reads day, month).
    pub fn hijri_day_month(&self, (_, m, d): HijriDate) -> String {
        format!("{} {}", self.hijri_digits(d), self.hijri_month(m))
    }

    /// `5 Jumada al-Ula 1448`.
    pub fn hijri_full(&self, h: HijriDate) -> String {
        format!("{} {}", self.hijri_day_month(h), self.hijri_digits(h.0))
    }
}

/// The Gregorian date's `pattern` parts and the Hijri date, as the
/// calendar asks, joined by `sep`; empty for [`Calendar::None`].
pub fn dated(l: &Lingo, date: &NaiveDate, pattern: &str, hijri: Option<HijriDate>, sep: &str) -> String {
    let g = l.calendar.gregorian().then(|| l.date(date, pattern));
    let h = if l.calendar.hijri() { Some(hijri.map(|h| l.hijri_day_month(h)).unwrap_or_default()) } else { None };
    [g, h].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(sep)
}

/// The Hijri date of `date` (no sunset rule: for previews and tests).
pub fn hijri_of(date: NaiveDate) -> HijriDate {
    crate::plugins::prayer::hijri(date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::desktop_widgets::Numerals;

    fn opts(numerals: Numerals, calendar: Calendar, language: &str) -> TextOptions {
        TextOptions { numerals, calendar, language: language.into(), ..TextOptions::default() }
    }

    #[test]
    fn digits_follow_the_language_unless_chosen() {
        assert!(Lingo::of(&opts(Numerals::Auto, Calendar::Gregorian, "ar"), false, None).eastern);
        assert!(!Lingo::of(&opts(Numerals::Auto, Calendar::Gregorian, "en"), true, None).eastern);
        assert!(!Lingo::of(&opts(Numerals::Western, Calendar::Gregorian, "ar"), false, None).eastern);
        assert!(Lingo::of(&opts(Numerals::EasternArabic, Calendar::Gregorian, "de"), false, None).eastern);
        let ar = Lingo::of(&opts(Numerals::Auto, Calendar::Gregorian, "ar"), false, None);
        assert!(ar.rtl && ar.digits("12:05") == "١٢:٠٥");
    }

    #[test]
    fn plugin_languages_stand_in_for_the_widgets() {
        let own = Lingo::of_plugin(&opts(Numerals::Auto, Calendar::Gregorian, ""), false, Some(Lang::Ar));
        assert!(own.rtl && own.arabic() && own.lang == Some(Lang::Ar));
        // The widget's own language wins; no plugin language follows the interface.
        assert_eq!(Lingo::of_plugin(&opts(Numerals::Auto, Calendar::Gregorian, "de"), true, Some(Lang::Ar)).lang, Some(Lang::De));
        assert_eq!(Lingo::of_plugin(&opts(Numerals::Auto, Calendar::Gregorian, ""), true, None).lang, None);
    }

    #[test]
    fn calendars_pick_the_dates() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 1).expect("date");
        let h = hijri_of(d);
        let en = |c| Lingo { hijri_in_arabic: false, ..Lingo::of(&opts(Numerals::Western, c, "en"), false, None) };
        assert_eq!(dated(&en(Calendar::Gregorian), &d, "%-d %B", Some(h), " · "), "1 November");
        assert!(dated(&en(Calendar::Hijri), &d, "%-d %B", Some(h), " · ").contains(&h.2.to_string()));
        assert_eq!(dated(&en(Calendar::Both), &d, "%-d %B", Some(h), " · ").matches(" · ").count(), 1);
        assert!(dated(&en(Calendar::None), &d, "%-d %B", Some(h), " · ").is_empty());
        // The Hijri date in Arabic keeps its own digits and names.
        let ar = Lingo::of(&opts(Numerals::Western, Calendar::Hijri, "en"), false, None);
        assert!(ar.hijri_day_month(h).chars().any(|c| ('\u{0660}'..='\u{0669}').contains(&c)));
    }
}
