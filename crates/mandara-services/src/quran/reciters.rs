//! The reciters shared by settings, playback and the reader.

use mandara_i18n::{t, tk};
use serde::Serialize;

/// A verified audio catalog; every offered recording uses Hafs from Asim.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Provider {
    GlobalAyah,
    EveryAyah { directory: &'static str },
    TimedChapter { read: u16, directory: &'static str },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reciter {
    pub id: &'static str,
    pub name: &'static str,
    pub arabic: &'static str,
    #[serde(skip)]
    pub(super) provider: Provider,
}

impl Reciter {
    pub fn localized_name(&self) -> &'static str {
        t(self.name)
    }
}

pub const RECITERS: [Reciter; 10] = [
    Reciter { id: "ar.yasseraldossari", name: tk("Yasser Al-Dossary"), arabic: "ياسر الدوسري", provider: Provider::EveryAyah { directory: "Yasser_Ad-Dussary_128kbps" } },
    Reciter { id: "mp3quran.258", name: tk("Abdulrashid Sufi"), arabic: "عبد الرشيد صوفي", provider: Provider::TimedChapter { read: 258, directory: "https://cdn.mp3quran.net/audio/abdulrasheed-sufi/r3/" } },
    Reciter { id: "ar.abdulsamad", name: tk("Abdul Basit Abdul Samad"), arabic: "عبد الباسط عبد الصمد", provider: Provider::EveryAyah { directory: "Abdul_Basit_Murattal_192kbps" } },
    Reciter { id: "ar.husary", name: tk("Mahmoud Khalil Al-Husary"), arabic: "محمود خليل الحصري", provider: Provider::GlobalAyah },
    Reciter { id: "ar.husarymujawwad", name: tk("Mahmoud Khalil Al-Husary (Mujawwad)"), arabic: "محمود خليل الحصري (مجوّد)", provider: Provider::GlobalAyah },
    Reciter { id: "ar.minshawi", name: tk("Mohamed Siddiq Al-Minshawi"), arabic: "محمد صديق المنشاوي", provider: Provider::GlobalAyah },
    Reciter { id: "ar.hudhaify", name: tk("Ali Al-Hudhaify"), arabic: "علي بن عبد الرحمن الحذيفي", provider: Provider::GlobalAyah },
    Reciter { id: "ar.mahermuaiqly", name: tk("Maher Al-Muaiqly"), arabic: "ماهر المعيقلي", provider: Provider::GlobalAyah },
    Reciter { id: "ar.shaatree", name: tk("Abu Bakr Ash-Shatri"), arabic: "أبو بكر الشاطري", provider: Provider::GlobalAyah },
    Reciter { id: "ar.muhammadayyoub", name: tk("Muhammad Ayyub"), arabic: "محمد أيوب", provider: Provider::GlobalAyah },
];

pub const DEFAULT_RECITER: &str = RECITERS[0].id;

/// Studio's static choice control uses the same catalog as playback.
pub const RECITER_CHOICES: [(&str, &str); RECITERS.len()] = choices();

const fn choices() -> [(&'static str, &'static str); RECITERS.len()] {
    let mut rows = [("", ""); RECITERS.len()];
    let mut i = 0;
    while i < RECITERS.len() {
        rows[i] = (RECITERS[i].id, RECITERS[i].name);
        i += 1;
    }
    rows
}

pub fn reciter(id: &str) -> Option<&'static Reciter> {
    RECITERS.iter().find(|r| r.id == id)
}

/// Saved choices that are no longer offered resolve without changing config.
pub fn reciter_or_default(id: &str) -> &'static Reciter {
    reciter(id).unwrap_or(&RECITERS[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_and_playback_offer_the_same_supported_reciters() {
        assert_eq!(RECITERS.len(), 10);
        for (i, r) in RECITERS.iter().enumerate() {
            assert_eq!(RECITER_CHOICES[i], (r.id, r.name));
            assert!(!r.name.is_empty() && !r.arabic.is_empty());
            assert_eq!(super::super::store::sanitize(r.id), r.id);
            assert!(!RECITERS[..i].iter().any(|old| old.id == r.id));
            assert_eq!(reciter_or_default(r.id), r);
        }
        assert!(matches!(reciter("ar.yasseraldossari").unwrap().provider, Provider::EveryAyah { directory: "Yasser_Ad-Dussary_128kbps" }));
        assert!(matches!(reciter("mp3quran.258").unwrap().provider, Provider::TimedChapter { read: 258, directory: "https://cdn.mp3quran.net/audio/abdulrasheed-sufi/r3/" }));
        assert!(matches!(reciter("ar.abdulsamad").unwrap().provider, Provider::EveryAyah { directory: "Abdul_Basit_Murattal_192kbps" }));
    }

    #[test]
    fn retired_and_unknown_saved_choices_use_the_supported_default() {
        for id in ["ar.alafasy", "ar.ahmedajamy", "unknown", ""] {
            assert!(reciter(id).is_none());
            assert_eq!(reciter_or_default(id).id, DEFAULT_RECITER);
        }
        assert_eq!(DEFAULT_RECITER, "ar.yasseraldossari");
    }
}
