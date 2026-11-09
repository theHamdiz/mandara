//! Stations: the curated, categorised list that ships with Mandara and the
//! stations people add themselves, plus the address rules both follow.
//!
//! Every curated entry was checked by connecting to it and decoding audio
//! (`cargo test -p mandara-services radio_curated_stations_play -- --ignored`
//! repeats the check); [`VERIFIED`] is the day of the last full pass.

use mandara_i18n::tk;
use serde::{Deserialize, Serialize};

/// The day every curated station last connected and decoded audio.
pub const VERIFIED: &str = "2026-11-09";

/// Longest station address accepted (bytes).
pub const MAX_URL_LEN: usize = 2048;
/// Longest station name kept (characters).
pub const MAX_NAME_CHARS: usize = 80;

/// A station's genre shelf.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    News,
    Talk,
    Pop,
    Rock,
    Jazz,
    Classical,
    Electronic,
    Chill,
    Ambient,
    World,
    Arabic,
    Quran,
    Kids,
    /// Stations added by address or from the directory that fit no shelf.
    Other,
}

/// Number of [`Category`] values.
pub const CATEGORIES: usize = 14;

impl Category {
    pub const ALL: [Category; CATEGORIES] = [
        Category::News,
        Category::Talk,
        Category::Pop,
        Category::Rock,
        Category::Jazz,
        Category::Classical,
        Category::Electronic,
        Category::Chill,
        Category::Ambient,
        Category::World,
        Category::Arabic,
        Category::Quran,
        Category::Kids,
        Category::Other,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Stable key (settings, commands, the state file).
    pub fn key(self) -> &'static str {
        match self {
            Category::News => "news",
            Category::Talk => "talk",
            Category::Pop => "pop",
            Category::Rock => "rock",
            Category::Jazz => "jazz",
            Category::Classical => "classical",
            Category::Electronic => "electronic",
            Category::Chill => "chill",
            Category::Ambient => "ambient",
            Category::World => "world",
            Category::Arabic => "arabic",
            Category::Quran => "quran",
            Category::Kids => "kids",
            Category::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        Category::ALL.into_iter().find(|c| c.key().eq_ignore_ascii_case(s))
    }

    /// English label (`tk`-marked; display with `tr`).
    pub fn label(self) -> &'static str {
        match self {
            Category::News => tk("News"),
            Category::Talk => tk("Talk"),
            Category::Pop => tk("Pop"),
            Category::Rock => tk("Rock"),
            Category::Jazz => tk("Jazz"),
            Category::Classical => tk("Classical"),
            Category::Electronic => tk("Electronic"),
            Category::Chill => tk("Lo-fi & chill"),
            Category::Ambient => tk("Ambient"),
            Category::World => tk("World"),
            Category::Arabic => tk("Arabic"),
            Category::Quran => tk("Qur'an recitation"),
            Category::Kids => tk("Kids"),
            Category::Other => tk("Other"),
        }
    }

    /// The bundled icon the shelf chips use.
    pub fn icon(self) -> &'static str {
        match self {
            Category::News => "globe",
            Category::Talk => "quote",
            Category::Pop => "music",
            Category::Rock => "zap",
            Category::Jazz => "music",
            Category::Classical => "music",
            Category::Electronic => "waves",
            Category::Chill => "coffee",
            Category::Ambient => "moon",
            Category::World => "globe",
            Category::Arabic => "globe",
            Category::Quran => "book",
            Category::Kids => "sun",
            Category::Other => "radio",
        }
    }

    /// Hue (OKLCH degrees) of the shelf, the base of generated station art.
    pub fn hue(self) -> f32 {
        match self {
            Category::News => 250.0,
            Category::Talk => 215.0,
            Category::Pop => 340.0,
            Category::Rock => 25.0,
            Category::Jazz => 55.0,
            Category::Classical => 80.0,
            Category::Electronic => 290.0,
            Category::Chill => 190.0,
            Category::Ambient => 265.0,
            Category::World => 140.0,
            Category::Arabic => 45.0,
            Category::Quran => 160.0,
            Category::Kids => 100.0,
            Category::Other => 230.0,
        }
    }
}

/// The codec a station sends (as far as Mandara can tell).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    #[default]
    Unknown,
    Mp3,
    Aac,
    Vorbis,
    Opus,
}

impl Codec {
    pub fn label(self) -> &'static str {
        match self {
            Codec::Unknown => "",
            Codec::Mp3 => "MP3",
            Codec::Aac => "AAC",
            Codec::Vorbis => "Vorbis",
            Codec::Opus => "Opus",
        }
    }

    /// Whether Mandara can decode it.
    pub fn playable(self) -> bool {
        !matches!(self, Codec::Opus)
    }
}

/// One curated station (all static: nothing is allocated to list them).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Curated {
    /// Stable id (`[a-z0-9-]`).
    pub id: &'static str,
    pub name: &'static str,
    pub category: Category,
    /// The stream (https wherever the broadcaster offers it).
    pub url: &'static str,
    /// The broadcaster's own page.
    pub homepage: &'static str,
    /// ISO 3166 country code (`""` for an online-only station).
    pub country: &'static str,
    pub codec: Codec,
    /// Nominal bitrate in kbit/s.
    pub kbps: u16,
    /// Extra search words (language, city, alternative names).
    pub tags: &'static str,
}

macro_rules! station {
    ($id:literal, $name:literal, $cat:ident, $url:literal, $home:literal, $cc:literal, $codec:ident, $kbps:literal, $tags:literal) => {
        Curated { id: $id, name: $name, category: Category::$cat, url: $url, homepage: $home, country: $cc, codec: Codec::$codec, kbps: $kbps, tags: $tags }
    };
}

/// The stations Mandara ships, grouped by shelf. Only public, official or
/// publicly listed free streams; each one connected and decoded on
/// [`VERIFIED`].
pub const CURATED: &[Curated] = &[
    // News
    station!("bbc-world-service", "BBC World Service", News, "https://stream.live.vc.bbcmedia.co.uk/bbc_world_service", "https://www.bbc.co.uk/worldserviceradio", "GB", Mp3, 56, "english london international"),
    station!("npr", "NPR Program Stream", News, "https://npr-ice.streamguys1.com/live.mp3", "https://www.npr.org", "US", Mp3, 96, "english public radio usa"),
    station!("franceinfo", "franceinfo", News, "https://icecast.radiofrance.fr/franceinfo-midfi.mp3", "https://www.francetvinfo.fr", "FR", Mp3, 128, "french france info radio france"),
    station!("deutschlandfunk", "Deutschlandfunk", News, "https://st01.sslstream.dlf.de/dlf/01/128/mp3/stream.mp3", "https://www.deutschlandfunk.de", "DE", Mp3, 128, "german dlf nachrichten"),
    station!("rfi-monde", "RFI Monde", News, "https://rfimonde64k.ice.infomaniak.ch/rfimonde-64.mp3", "https://www.rfi.fr", "FR", Mp3, 64, "french radio france internationale"),
    station!("dw-english", "DW English", News, "https://dw.audiostream.io/dw/1027/mp3/64/dw08", "https://www.dw.com", "DE", Mp3, 64, "english deutsche welle"),
    // Talk
    station!("france-culture", "France Culture", Talk, "https://icecast.radiofrance.fr/franceculture-midfi.mp3", "https://www.radiofrance.fr/franceculture", "FR", Mp3, 128, "french culture podcasts radio france"),
    station!("france-inter", "France Inter", Talk, "https://icecast.radiofrance.fr/franceinter-midfi.mp3", "https://www.radiofrance.fr/franceinter", "FR", Mp3, 128, "french radio france"),
    station!("dlf-kultur", "Deutschlandfunk Kultur", Talk, "https://st02.sslstream.dlf.de/dlf/02/128/mp3/stream.mp3", "https://www.deutschlandfunkkultur.de", "DE", Mp3, 128, "german kultur"),
    station!("wnyc", "WNYC 93.9 FM", Talk, "https://fm939.wnyc.org/wnycfm", "https://www.wnyc.org", "US", Mp3, 96, "english new york public radio"),
    // Pop
    station!("kexp", "KEXP 90.3 Seattle", Pop, "https://kexp-mp3-128.streamguys1.com/kexp128.mp3", "https://www.kexp.org", "US", Mp3, 128, "english indie alternative seattle"),
    station!("fip-pop", "FIP Pop", Pop, "https://icecast.radiofrance.fr/fippop-midfi.mp3", "https://www.radiofrance.fr/fip", "FR", Mp3, 128, "radio france"),
    station!("1live", "1LIVE", Pop, "https://wdr-1live-live.icecastssl.wdr.de/wdr/1live/live/mp3/128/stream.mp3", "https://www1.wdr.de/radio/1live", "DE", Mp3, 128, "german wdr charts"),
    station!("soma-indie-pop", "SomaFM Indie Pop Rocks!", Pop, "https://ice1.somafm.com/indiepop-128-mp3", "https://somafm.com/indiepop/", "US", Mp3, 128, "indie somafm"),
    station!("soma-poptron", "SomaFM PopTron", Pop, "https://ice1.somafm.com/poptron-128-mp3", "https://somafm.com/poptron/", "US", Mp3, 128, "electropop synthpop somafm"),
    // Rock
    station!("radio-paradise", "Radio Paradise", Rock, "https://stream.radioparadise.com/mp3-192", "https://radioparadise.com", "US", Mp3, 192, "eclectic main mix"),
    station!("radio-paradise-rock", "Radio Paradise Rock Mix", Rock, "https://stream.radioparadise.com/rock-192", "https://radioparadise.com", "US", Mp3, 192, "classic rock"),
    station!("fip-rock", "FIP Rock", Rock, "https://icecast.radiofrance.fr/fiprock-midfi.mp3", "https://www.radiofrance.fr/fip", "FR", Mp3, 128, "radio france"),
    station!("radio-bob", "RADIO BOB!", Rock, "https://streams.radiobob.de/bob-live/mp3-192/mediaplayer", "https://www.radiobob.de", "DE", Mp3, 192, "german rock"),
    station!("soma-underground-80s", "SomaFM Underground 80s", Rock, "https://ice1.somafm.com/u80s-128-mp3", "https://somafm.com/u80s/", "US", Mp3, 128, "new wave 80s somafm"),
    station!("soma-left-coast-70s", "SomaFM Left Coast 70s", Rock, "https://ice1.somafm.com/seventies-128-mp3", "https://somafm.com/seventies/", "US", Mp3, 128, "70s mellow somafm"),
    station!("soma-metal-detector", "SomaFM Metal Detector", Rock, "https://ice1.somafm.com/metal-128-mp3", "https://somafm.com/metal/", "US", Mp3, 128, "metal heavy somafm"),
    // Jazz
    station!("jazz24", "Jazz24", Jazz, "https://knkx-live-a.edge.audiocdn.com/6285_128k", "https://www.jazz24.org", "US", Mp3, 128, "knkx seattle"),
    station!("tsf-jazz", "TSF Jazz", Jazz, "https://tsfjazz.ice.infomaniak.ch/tsfjazz-high.mp3", "https://www.tsfjazz.com", "FR", Mp3, 128, "paris"),
    station!("fip-jazz", "FIP Jazz", Jazz, "https://icecast.radiofrance.fr/fipjazz-midfi.mp3", "https://www.radiofrance.fr/fip", "FR", Mp3, 128, "radio france"),
    station!("soma-sonic-universe", "SomaFM Sonic Universe", Jazz, "https://ice1.somafm.com/sonicuniverse-128-mp3", "https://somafm.com/sonicuniverse/", "US", Mp3, 128, "nu jazz somafm"),
    station!("wbgo", "WBGO Jazz 88.3", Jazz, "https://ais-sa8.cdnstream1.com/3629_128.mp3", "https://www.wbgo.org", "US", Mp3, 128, "newark new york"),
    // Classical
    station!("france-musique", "France Musique", Classical, "https://icecast.radiofrance.fr/francemusique-midfi.mp3", "https://www.radiofrance.fr/francemusique", "FR", Mp3, 128, "radio france orchestra"),
    station!("wqxr", "WQXR 105.9", Classical, "https://stream.wqxr.org/wqxr", "https://www.wqxr.org", "US", Mp3, 128, "new york"),
    station!("venice-classic", "Venice Classic Radio", Classical, "https://uk2.streamingpulse.com/ssl/vcr1", "https://www.veniceclassicradio.eu", "IT", Mp3, 128, "italy baroque"),
    station!("klassik-radio", "Klassik Radio", Classical, "https://live.streams.klassikradio.de/klassikradio-deutschland", "https://www.klassikradio.de", "DE", Aac, 128, "german film music"),
    station!("kusc", "Classical KUSC", Classical, "https://playerservices.streamtheworld.com/api/livestream-redirect/KUSCMP256.mp3", "https://www.kusc.org", "US", Mp3, 256, "los angeles"),
    // Electronic
    station!("fip-electro", "FIP Electro", Electronic, "https://icecast.radiofrance.fr/fipelectro-midfi.mp3", "https://www.radiofrance.fr/fip", "FR", Mp3, 128, "radio france"),
    station!("soma-beat-blender", "SomaFM Beat Blender", Electronic, "https://ice1.somafm.com/beatblender-128-mp3", "https://somafm.com/beatblender/", "US", Mp3, 128, "deep house downtempo somafm"),
    station!("soma-cliqhop", "SomaFM cliqhop idm", Electronic, "https://ice1.somafm.com/cliqhop-128-mp3", "https://somafm.com/cliqhop/", "US", Mp3, 128, "idm glitch somafm"),
    station!("soma-def-con", "SomaFM DEF CON Radio", Electronic, "https://ice1.somafm.com/defcon-128-mp3", "https://somafm.com/defcon/", "US", Mp3, 128, "hacking coding somafm"),
    station!("soma-vaporwaves", "SomaFM Vaporwaves", Electronic, "https://ice1.somafm.com/vaporwaves-128-mp3", "https://somafm.com/vaporwaves/", "US", Mp3, 128, "vaporwave somafm"),
    // Lo-fi & chill
    station!("soma-groove-salad", "SomaFM Groove Salad", Chill, "https://ice1.somafm.com/groovesalad-128-aac", "https://somafm.com/groovesalad/", "US", Aac, 128, "downtempo chillout somafm"),
    station!("soma-lush", "SomaFM Lush", Chill, "https://ice1.somafm.com/lush-128-mp3", "https://somafm.com/lush/", "US", Mp3, 128, "vocals somafm"),
    station!("soma-fluid", "SomaFM Fluid", Chill, "https://ice1.somafm.com/fluid-128-mp3", "https://somafm.com/fluid/", "US", Mp3, 128, "instrumental hip hop lofi somafm"),
    station!("soma-secret-agent", "SomaFM Secret Agent", Chill, "https://ice1.somafm.com/secretagent-128-mp3", "https://somafm.com/secretagent/", "US", Mp3, 128, "lounge somafm"),
    station!("radio-paradise-mellow", "Radio Paradise Mellow Mix", Chill, "https://stream.radioparadise.com/mellow-192", "https://radioparadise.com", "US", Mp3, 192, "mellow"),
    station!("laut-lofi", "lofi (laut.fm)", Chill, "https://lofi.stream.laut.fm/lofi", "https://laut.fm/lofi", "DE", Mp3, 128, "lofi hip hop study beats"),
    // Ambient
    station!("soma-drone-zone", "SomaFM Drone Zone", Ambient, "https://ice1.somafm.com/dronezone-128-mp3", "https://somafm.com/dronezone/", "US", Mp3, 128, "drone space somafm"),
    station!("soma-deep-space-one", "SomaFM Deep Space One", Ambient, "https://ice1.somafm.com/deepspaceone-128-mp3", "https://somafm.com/deepspaceone/", "US", Mp3, 128, "space somafm"),
    station!("soma-space-station", "SomaFM Space Station Soma", Ambient, "https://ice1.somafm.com/spacestation-128-mp3", "https://somafm.com/spacestation/", "US", Mp3, 128, "space somafm"),
    station!("ambient-sleeping-pill", "Ambient Sleeping Pill", Ambient, "https://radio.stereoscenic.com/asp-h", "https://ambientsleepingpill.com", "US", Mp3, 256, "sleep calm"),
    // World
    station!("fip-monde", "FIP Monde", World, "https://icecast.radiofrance.fr/fipworld-midfi.mp3", "https://www.radiofrance.fr/fip", "FR", Mp3, 128, "world music radio france"),
    station!("fip-reggae", "FIP Reggae", World, "https://icecast.radiofrance.fr/fipreggae-midfi.mp3", "https://www.radiofrance.fr/fip", "FR", Mp3, 128, "reggae dub radio france"),
    station!("soma-suburbs-of-goa", "SomaFM Suburbs of Goa", World, "https://ice1.somafm.com/suburbsofgoa-128-mp3", "https://somafm.com/suburbsofgoa/", "US", Mp3, 128, "india asian somafm"),
    station!("soma-thistle", "SomaFM ThistleRadio", World, "https://ice1.somafm.com/thistle-128-mp3", "https://somafm.com/thistle/", "US", Mp3, 128, "celtic irish scottish somafm"),
    station!("radio-panik", "Radio Panik", World, "https://streaming.domainepublic.net/radiopanik.ogg", "https://www.radiopanik.org", "BE", Vorbis, 112, "brussels belgium community french"),
    station!("radio-paradise-global", "Radio Paradise Global Mix", World, "https://stream.radioparadise.com/global-192", "https://radioparadise.com", "US", Mp3, 192, "global"),
    // Arabic
    station!("monte-carlo-doualiya", "Monte Carlo Doualiya", Arabic, "https://montecarlodoualiya128k.ice.infomaniak.ch/mc-doualiya.mp3", "https://www.mc-doualiya.com", "FR", Mp3, 128, "مونت كارلو الدولية news arabic"),
    station!("bbc-arabic", "BBC Arabic Radio", Arabic, "https://stream.live.vc.bbcmedia.co.uk/bbc_arabic_radio", "https://www.bbc.com/arabic", "GB", Mp3, 56, "بي بي سي عربي news arabic"),
    station!("sky-news-arabia", "Sky News Arabia Radio", Arabic, "https://radio.skynewsarabia.com/stream/radio/skynewsarabia", "https://www.skynewsarabia.com", "AE", Mp3, 128, "سكاي نيوز عربية news arabic"),
    station!("nogoum-fm", "Nogoum FM", Arabic, "https://audio.nrpstream.com/listen/nogoumfm/radio.mp3", "https://nogoumfm.net", "EG", Mp3, 128, "نجوم إف إم egypt cairo music"),
    station!("nile-fm", "Nile FM", Arabic, "https://audio.nrpstream.com/listen/nile_fm/radio.mp3", "https://nilefm.com", "EG", Mp3, 128, "نايل إف إم egypt cairo english hits"),
    // Qur'an recitation (the publicly listed streams of mp3quran.net and
    // the Cairo Qur'an radio)
    station!("quran-cairo", "Qur'an Radio Cairo", Quran, "https://stream.radiojar.com/8s5u5tpdtwzuv", "https://www.maspero.eg", "EG", Mp3, 128, "إذاعة القرآن الكريم من القاهرة egypt"),
    station!("quran-alafasi", "Mishary Alafasi", Quran, "https://backup.qurango.net/radio/mishary_alafasi", "https://mp3quran.net", "", Mp3, 128, "مشاري العفاسي recitation"),
    station!("quran-husary", "Mahmoud Khalil Al-Hussary", Quran, "https://backup.qurango.net/radio/mahmoud_khalil_alhussary", "https://mp3quran.net", "", Mp3, 128, "محمود خليل الحصري recitation"),
    station!("quran-minshawi", "Mohammed Siddiq Al-Minshawi", Quran, "https://backup.qurango.net/radio/mohammed_siddiq_alminshawi", "https://mp3quran.net", "", Mp3, 128, "محمد صديق المنشاوي recitation"),
    station!("quran-abdulbasit", "Abdulbasit Abdulsamad", Quran, "https://backup.qurango.net/radio/abdulbasit_abdulsamad", "https://mp3quran.net", "", Mp3, 128, "عبد الباسط عبد الصمد recitation"),
    station!("quran-sudais", "Abdulrahman Al-Sudais", Quran, "https://backup.qurango.net/radio/abdulrahman_alsudaes", "https://mp3quran.net", "", Mp3, 128, "عبد الرحمن السديس recitation"),
    station!("quran-maher", "Maher Al-Muaiqly", Quran, "https://backup.qurango.net/radio/maher", "https://mp3quran.net", "", Mp3, 128, "ماهر المعيقلي recitation"),
    station!("quran-tafseer", "Qur'an Tafseer", Quran, "https://backup.qurango.net/radio/tafseer", "https://mp3quran.net", "", Mp3, 128, "تفسير القرآن explanation"),
    station!("quran-english", "Qur'an with English translation", Quran, "https://backup.qurango.net/radio/translation_quran_english_basit", "https://mp3quran.net", "", Mp3, 96, "english translation meanings"),
    // Kids
    station!("kinderradio", "Kinderradio (laut.fm)", Kids, "https://kinderradio.stream.laut.fm/kinderradio", "https://laut.fm/kinderradio", "DE", Mp3, 128, "german children songs"),
    station!("fun-kids", "Fun Kids", Kids, "https://listen-funkids.sharp-stream.com/funkids.mp3", "https://www.funkidslive.com", "GB", Mp3, 128, "english children"),
    station!("radio-teddy", "Radio TEDDY", Kids, "https://streamtdy.ir-media-tec.com/live/mp3-128/web/play.mp3", "https://www.radioteddy.de", "DE", Mp3, 128, "german children"),
];

/// The curated station `id`.
pub fn curated(id: &str) -> Option<&'static Curated> {
    CURATED.iter().find(|s| s.id == id)
}

/// Why an address cannot be a station.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UrlError {
    Empty,
    TooLong,
    /// Not http:// or https://.
    Scheme,
    /// Spaces, control characters or another malformed part.
    Malformed,
    NoHost,
    /// User names or passwords in the address.
    Credentials,
}

impl UrlError {
    /// English message (`tk`-marked; display with `tr`).
    pub fn message(self) -> &'static str {
        match self {
            UrlError::Empty => tk("Enter the stream's address"),
            UrlError::TooLong => tk("That address is too long"),
            UrlError::Scheme => tk("Stream addresses start with http:// or https://"),
            UrlError::Malformed => tk("That doesn't look like a web address"),
            UrlError::NoHost => tk("The address has no server name"),
            UrlError::Credentials => tk("Addresses with a user name or password aren't supported"),
        }
    }
}

/// Checks a station address before any connection is made.
pub fn validate_url(url: &str) -> Result<(), UrlError> {
    let url = url.trim();
    if url.is_empty() {
        return Err(UrlError::Empty);
    }
    if url.len() > MAX_URL_LEN {
        return Err(UrlError::TooLong);
    }
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(UrlError::Scheme);
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(UrlError::Malformed);
    }
    let uri: ureq::http::Uri = url.parse().map_err(|_| UrlError::Malformed)?;
    let authority = uri.authority().ok_or(UrlError::NoHost)?;
    if authority.as_str().contains('@') {
        return Err(UrlError::Credentials);
    }
    if authority.host().is_empty() {
        return Err(UrlError::NoHost);
    }
    Ok(())
}

/// Trims a user-given station name to [`MAX_NAME_CHARS`] characters.
pub fn clean_name(name: &str) -> String {
    let name = name.trim();
    match name.char_indices().nth(MAX_NAME_CHARS) {
        Some((end, _)) => name[..end].trim_end().to_string(),
        None => name.to_string(),
    }
}

/// A stable id for a station added by address: `custom-` and a short hash
/// of the address (the same address is the same station).
pub fn custom_id(url: &str) -> String {
    // FNV-1a: stable across releases (std's hasher is not).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.trim().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("custom-{:012x}", h & 0xffff_ffff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_curated_list_is_well_formed() {
        let mut ids = HashSet::new();
        let mut urls = HashSet::new();
        for s in CURATED {
            assert!(ids.insert(s.id), "duplicate id {}", s.id);
            assert!(urls.insert(s.url), "duplicate stream {}", s.url);
            assert!(s.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "id {}", s.id);
            assert!(!s.id.starts_with("custom-") && !s.id.starts_with("dir-"), "reserved prefix {}", s.id);
            assert_eq!(validate_url(s.url), Ok(()), "{}", s.id);
            assert!(s.url.starts_with("https://"), "{} should be https", s.id);
            assert!(s.homepage.starts_with("https://"), "{} homepage", s.id);
            assert!(!s.name.is_empty() && s.name.chars().count() <= MAX_NAME_CHARS, "{}", s.id);
            assert!(s.codec.playable() && s.codec != Codec::Unknown, "{}", s.id);
            assert!((32..=320).contains(&s.kbps), "{} bitrate", s.id);
            assert!(s.country.is_empty() || (s.country.len() == 2 && s.country.bytes().all(|b| b.is_ascii_uppercase())), "{}", s.id);
            assert_ne!(s.category, Category::Other, "{} needs a shelf", s.id);
        }
        // Every shelf but Other has at least two stations.
        for c in Category::ALL.into_iter().filter(|c| *c != Category::Other) {
            assert!(CURATED.iter().filter(|s| s.category == c).count() >= 2, "{c:?}");
        }
        // Grouped by shelf in shelf order (the panel lists them as written).
        let order: Vec<usize> = CURATED.iter().map(|s| s.category.index()).collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]), "curated stations are grouped by shelf");
        assert_eq!(VERIFIED.len(), 10);
    }

    #[test]
    fn categories_round_trip() {
        for c in Category::ALL {
            assert_eq!(Category::parse(c.key()), Some(c));
            assert_eq!(Category::ALL[c.index()], c);
            assert!(!c.label().is_empty() && !c.icon().is_empty());
            let toml = toml::to_string(&std::collections::BTreeMap::from([("c", c)])).unwrap();
            assert!(toml.contains(c.key()), "{toml}");
        }
        assert_eq!(Category::parse(" JAZZ "), Some(Category::Jazz));
        assert_eq!(Category::parse("polka"), None);
    }

    #[test]
    fn addresses_are_checked_before_connecting() {
        assert_eq!(validate_url("https://ice1.somafm.com/groovesalad-128-mp3"), Ok(()));
        assert_eq!(validate_url("  http://example.com:8000/stream  "), Ok(()));
        assert_eq!(validate_url("HTTPS://Example.com/a.pls"), Ok(()));
        assert_eq!(validate_url(""), Err(UrlError::Empty));
        assert_eq!(validate_url("   "), Err(UrlError::Empty));
        assert_eq!(validate_url("ftp://example.com/x"), Err(UrlError::Scheme));
        assert_eq!(validate_url("file:///etc/passwd"), Err(UrlError::Scheme));
        assert_eq!(validate_url("javascript:alert(1)"), Err(UrlError::Scheme));
        assert_eq!(validate_url("example.com/stream"), Err(UrlError::Scheme));
        assert_eq!(validate_url("http://exa mple.com/"), Err(UrlError::Malformed));
        assert_eq!(validate_url("http://example.com/\u{7}"), Err(UrlError::Malformed));
        assert_eq!(validate_url("http://"), Err(UrlError::Malformed));
        assert_eq!(validate_url("http://user:pw@example.com/"), Err(UrlError::Credentials));
        assert_eq!(validate_url(&format!("https://example.com/{}", "a".repeat(MAX_URL_LEN))), Err(UrlError::TooLong));
        for e in [UrlError::Empty, UrlError::TooLong, UrlError::Scheme, UrlError::Malformed, UrlError::NoHost, UrlError::Credentials] {
            assert!(!e.message().is_empty());
        }
    }

    #[test]
    fn names_and_custom_ids() {
        assert_eq!(clean_name("  Corniche FM  "), "Corniche FM");
        assert_eq!(clean_name(&"x".repeat(200)).chars().count(), MAX_NAME_CHARS);
        assert_eq!(clean_name(&"ع".repeat(100)).chars().count(), MAX_NAME_CHARS, "counts characters, not bytes");
        let a = custom_id("https://example.com/a");
        assert_eq!(a, custom_id(" https://example.com/a "));
        assert_ne!(a, custom_id("https://example.com/b"));
        assert!(a.starts_with("custom-") && a.len() == "custom-".len() + 12);
        // Stable across releases.
        assert_eq!(custom_id(""), "custom-9ce484222325");
    }
}
