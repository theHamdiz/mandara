//! Spoken five-minute time phrases and the hour’s grammatical case.

use mandara_i18n::{fill, tk};
use super::super::lingo::Lingo;

/// Case endings for twelve, one … eleven; written words stay in the catalog.
const TURKISH_PAST: [&str; 12] = ["yi", "i", "yi", "ü", "ü", "i", "yı", "yi", "i", "u", "u", "i"];
/// Dative endings in the same clock order, including the vowel buffer y.
const TURKISH_TO: [&str; 12] = ["ye", "e", "ye", "e", "e", "e", "ya", "ye", "e", "a", "a", "e"];
const HOURS: [&str; 12] = [tk("Twelve"), tk("One"), tk("Two"), tk("Three"), tk("Four"), tk("Five"), tk("Six"), tk("Seven"), tk("Eight"), tk("Nine"), tk("Ten"), tk("Eleven")];
const PHRASES: [&str; 12] = [tk("{hour} o'clock"), tk("Five past {hour}"), tk("Ten past {hour}"), tk("Quarter past {hour}"), tk("Twenty past {hour}"), tk("Twenty-five past {hour}"), tk("Half past {hour}"), tk("Twenty-five to {hour}"), tk("Twenty to {hour}"), tk("Quarter to {hour}"), tk("Ten to {hour}"), tk("Five to {hour}")];

/// The nearest five-minute phrase; the face also prints the exact time.
pub(super) fn phrase(hour: u32, minute: u32, l: &Lingo) -> String {
    let step = (minute.min(59) + 2) / 5;
    let next = u32::from(step > 6);
    let index = ((hour + next) % 12) as usize;
    let mut hour = l.word(HOURS[index]);
    if l.lang.unwrap_or_else(mandara_i18n::language) == mandara_i18n::Lang::Tr && matches!(step, 1..=5 | 7..=11) {
        // Turkish uses an accusative hour before “past”, a dative hour
        // before “to”, vowel buffers, and dörd- when four takes a suffix.
        if index == 4 { hour.pop(); hour.push('d'); }
        hour.push_str(if step < 6 { TURKISH_PAST[index] } else { TURKISH_TO[index] });
    }
    fill(&l.word(PHRASES[(step % 12) as usize]), &[("hour", &hour)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_to_phrases_keep_the_upcoming_hour_in_the_nominative() {
        let l = Lingo::of(&mandara_config::desktop_widgets::TextOptions { language: "ru".into(), ..Default::default() }, false, None);
        for (minute, expected) in [(35, "Без двадцати пяти Два"), (40, "Без двадцати Два"), (45, "Без четверти Два"), (50, "Без десяти Два"), (55, "Без пяти Два")] {
            assert_eq!(phrase(1, minute, &l), expected);
        }
    }

    #[test]
    fn turkish_hours_use_natural_past_half_and_to_forms() {
        let l = Lingo::of(&mandara_config::desktop_widgets::TextOptions { language: "tr".into(), ..Default::default() }, false, None);
        assert_eq!(phrase(9, 5, &l), "Dokuzu beş geçiyor");
        assert_eq!(phrase(9, 30, &l), "Dokuz buçuk");
        assert_eq!(phrase(3, 45, &l), "Dörde çeyrek var");
        assert_eq!(phrase(23, 59, &l), "Tam On iki");
        let past = ["On ikiyi", "Biri", "İkiyi", "Üçü", "Dördü", "Beşi", "Altıyı", "Yediyi", "Sekizi", "Dokuzu", "Onu", "On biri"];
        let to = ["On ikiye", "Bire", "İkiye", "Üçe", "Dörde", "Beşe", "Altıya", "Yediye", "Sekize", "Dokuza", "Ona", "On bire"];
        for hour in 0..12 {
            assert_eq!(phrase(hour, 5, &l), format!("{} beş geçiyor", past[hour as usize]));
            assert_eq!(phrase((hour + 11) % 12, 55, &l), format!("{} beş var", to[hour as usize]));
        }
    }
}
