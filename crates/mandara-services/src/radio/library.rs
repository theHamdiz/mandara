//! Your stations: favourites in your order, stations you added (by address
//! or from the directory), per-station volume, recent stations and the
//! last one played. Kept in `~/.local/state/mandara/radio.toml`; every list
//! is bounded and cleaned on load.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::station::{CURATED, Category, Codec, Curated, MAX_NAME_CHARS, MAX_URL_LEN, clean_name, curated, custom_id, validate_url};

/// Stations you can add, at most.
pub const MAX_CUSTOM: usize = 200;
/// Favourites kept, at most.
pub const MAX_FAVOURITES: usize = 200;
/// Recent stations remembered.
pub const MAX_RECENT: usize = 8;

/// A station you added.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Custom {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default = "other")]
    pub category: Category,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub kbps: Option<u32>,
    #[serde(default)]
    pub codec: Codec,
}

fn other() -> Category {
    Category::Other
}

fn default_volume() -> f32 {
    super::player::DEFAULT_VOLUME
}

/// The saved library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    pub favourites: Vec<String>,
    #[serde(default)]
    pub custom: Vec<Custom>,
    /// Volume per station id, when it differs from the main volume.
    #[serde(default)]
    pub volumes: BTreeMap<String, f32>,
    #[serde(default)]
    pub recent: Vec<String>,
    #[serde(default)]
    pub last: Option<String>,
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub muted: bool,
}

impl Default for Library {
    fn default() -> Self {
        Self { favourites: Vec::new(), custom: Vec::new(), volumes: BTreeMap::new(), recent: Vec::new(), last: None, volume: default_volume(), muted: false }
    }
}

/// A station of either kind, borrowed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Station<'a> {
    Curated(&'static Curated),
    Custom(&'a Custom),
}

impl<'a> Station<'a> {
    pub fn id(&self) -> &'a str {
        match self {
            Station::Curated(c) => c.id,
            Station::Custom(c) => &c.id,
        }
    }
    pub fn name(&self) -> &'a str {
        match self {
            Station::Curated(c) => c.name,
            Station::Custom(c) => &c.name,
        }
    }
    pub fn url(&self) -> &'a str {
        match self {
            Station::Curated(c) => c.url,
            Station::Custom(c) => &c.url,
        }
    }
    pub fn category(&self) -> Category {
        match self {
            Station::Curated(c) => c.category,
            Station::Custom(c) => c.category,
        }
    }
    pub fn homepage(&self) -> &'a str {
        match self {
            Station::Curated(c) => c.homepage,
            Station::Custom(c) => &c.homepage,
        }
    }
    pub fn kbps(&self) -> Option<u32> {
        match self {
            Station::Curated(c) => Some(u32::from(c.kbps)),
            Station::Custom(c) => c.kbps,
        }
    }
    pub fn codec(&self) -> Codec {
        match self {
            Station::Curated(c) => c.codec,
            Station::Custom(c) => c.codec,
        }
    }
    pub fn country(&self) -> &'a str {
        match self {
            Station::Curated(c) => c.country,
            Station::Custom(_) => "",
        }
    }
    pub fn tags(&self) -> &'a str {
        match self {
            Station::Curated(c) => c.tags,
            Station::Custom(_) => "",
        }
    }
    pub fn is_custom(&self) -> bool {
        matches!(self, Station::Custom(_))
    }

    /// The player's tune for this station.
    pub fn tune(&self) -> super::player::Tune {
        super::player::Tune { id: self.id().to_string(), name: self.name().to_string(), url: self.url().to_string(), kbps: self.kbps() }
    }

    /// Whether every (lower-case) word appears in the station's name,
    /// tags, country or shelf (its key or `shelf_label`, the shelf's name
    /// in the current language).
    pub fn matches(&self, words: &[&str], shelf_label: &str) -> bool {
        words.iter().all(|w| {
            contains_ci(self.name(), w) || contains_ci(self.tags(), w) || self.country().eq_ignore_ascii_case(w) || contains_ci(self.category().key(), w) || contains_ci(shelf_label, w)
        })
    }
}

/// Case-insensitive substring test without allocating (`needle` is
/// lower-case).
pub fn contains_ci(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if hay.is_ascii() && needle.is_ascii() {
        return hay.as_bytes().windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle.as_bytes()));
    }
    for (i, _) in hay.char_indices() {
        let mut a = hay[i..].chars().flat_map(char::to_lowercase);
        if needle.chars().all(|n| a.next() == Some(n)) {
            return true;
        }
    }
    false
}

/// Why a station couldn't be added or changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    Url(super::station::UrlError),
    NoName,
    Full,
    Exists,
    Unknown,
}

impl EditError {
    pub fn message(&self) -> &'static str {
        use mandara_i18n::tk;
        match self {
            EditError::Url(e) => e.message(),
            EditError::NoName => tk("Give the station a name"),
            EditError::Full => tk("You have the most stations Mandara keeps; remove one first"),
            EditError::Exists => tk("You already have this station"),
            EditError::Unknown => tk("That station isn't in your list"),
        }
    }
}

impl Library {
    pub fn path() -> PathBuf {
        mandara_config::paths::state_dir().join("radio.toml")
    }

    /// Reads the library (an empty one when missing or unreadable).
    pub fn load(path: &Path) -> Self {
        let mut lib = std::fs::read_to_string(path).ok().and_then(|t| toml::from_str::<Library>(&t).ok()).unwrap_or_default();
        lib.clean();
        lib
    }

    /// Writes it atomically (temporary file, then rename).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    /// Bounds every list and drops entries that point nowhere.
    pub fn clean(&mut self) {
        self.custom.retain(|c| validate_url(&c.url).is_ok() && c.url.len() <= MAX_URL_LEN && !c.id.is_empty() && curated(&c.id).is_none());
        for c in &mut self.custom {
            c.name = clean_name(&c.name);
            if c.name.is_empty() {
                c.name = c.url.chars().take(MAX_NAME_CHARS).collect();
            }
        }
        let mut seen = std::collections::HashSet::new();
        self.custom.retain(|c| seen.insert(c.id.clone()));
        self.custom.truncate(MAX_CUSTOM);
        let known = |lib: &Library, id: &str| curated(id).is_some() || lib.custom.iter().any(|c| c.id == id);
        let favs = std::mem::take(&mut self.favourites);
        let mut seen = std::collections::HashSet::new();
        self.favourites = favs.into_iter().filter(|id| known(self, id) && seen.insert(id.clone())).take(MAX_FAVOURITES).collect();
        let recent = std::mem::take(&mut self.recent);
        let mut seen = std::collections::HashSet::new();
        self.recent = recent.into_iter().filter(|id| known(self, id) && seen.insert(id.clone())).take(MAX_RECENT).collect();
        let volumes = std::mem::take(&mut self.volumes);
        self.volumes = volumes.into_iter().filter(|(id, v)| known(self, id) && v.is_finite()).map(|(id, v)| (id, v.clamp(0.0, 1.0))).collect();
        if self.last.as_deref().is_some_and(|id| !known(self, id)) {
            self.last = None;
        }
        self.volume = if self.volume.is_finite() { self.volume.clamp(0.0, 1.0) } else { default_volume() };
    }

    /// The station `id` (curated or yours).
    pub fn station(&self, id: &str) -> Option<Station<'_>> {
        curated(id).map(Station::Curated).or_else(|| self.custom.iter().find(|c| c.id == id).map(Station::Custom))
    }

    /// Every station: the curated ones, then yours.
    pub fn all(&self) -> impl Iterator<Item = Station<'_>> {
        CURATED.iter().map(Station::Curated).chain(self.custom.iter().map(Station::Custom))
    }

    pub fn is_favourite(&self, id: &str) -> bool {
        self.favourites.iter().any(|f| f == id)
    }

    /// Adds or removes a favourite; whether it is one now.
    pub fn toggle_favourite(&mut self, id: &str) -> Result<bool, EditError> {
        if let Some(i) = self.favourites.iter().position(|f| f == id) {
            self.favourites.remove(i);
            return Ok(false);
        }
        if self.station(id).is_none() {
            return Err(EditError::Unknown);
        }
        if self.favourites.len() >= MAX_FAVOURITES {
            return Err(EditError::Full);
        }
        self.favourites.push(id.to_string());
        Ok(true)
    }

    /// Moves a favourite `delta` places (clamped); whether it moved.
    pub fn move_favourite(&mut self, id: &str, delta: isize) -> bool {
        let Some(from) = self.favourites.iter().position(|f| f == id) else { return false };
        let to = (from as isize + delta).clamp(0, self.favourites.len() as isize - 1) as usize;
        if to == from {
            return false;
        }
        let item = self.favourites.remove(from);
        self.favourites.insert(to, item);
        true
    }

    /// Adds a station by address (checked again here); its id.
    pub fn add_custom(&mut self, name: &str, url: &str, category: Category, kbps: Option<u32>, codec: Codec) -> Result<String, EditError> {
        validate_url(url).map_err(EditError::Url)?;
        let name = clean_name(name);
        if name.is_empty() {
            return Err(EditError::NoName);
        }
        let url = url.trim().to_string();
        if self.all().any(|s| s.url() == url) {
            return Err(EditError::Exists);
        }
        if self.custom.len() >= MAX_CUSTOM {
            return Err(EditError::Full);
        }
        let id = custom_id(&url);
        self.custom.push(Custom { id: id.clone(), name, url, category, homepage: String::new(), kbps, codec });
        Ok(id)
    }

    /// Changes one of your stations (its id stays).
    pub fn edit_custom(&mut self, id: &str, name: &str, url: &str, category: Category) -> Result<(), EditError> {
        validate_url(url).map_err(EditError::Url)?;
        let name = clean_name(name);
        if name.is_empty() {
            return Err(EditError::NoName);
        }
        let url = url.trim();
        if self.all().any(|s| s.url() == url && s.id() != id) {
            return Err(EditError::Exists);
        }
        let c = self.custom.iter_mut().find(|c| c.id == id).ok_or(EditError::Unknown)?;
        if c.url != url {
            c.kbps = None;
            c.codec = Codec::Unknown;
        }
        c.name = name;
        c.url = url.to_string();
        c.category = category;
        Ok(())
    }

    /// Removes one of your stations everywhere; whether it existed.
    pub fn remove_custom(&mut self, id: &str) -> bool {
        let before = self.custom.len();
        self.custom.retain(|c| c.id != id);
        if self.custom.len() == before {
            return false;
        }
        self.favourites.retain(|f| f != id);
        self.recent.retain(|f| f != id);
        self.volumes.remove(id);
        if self.last.as_deref() == Some(id) {
            self.last = None;
        }
        true
    }

    /// Remembers that `id` played.
    pub fn played(&mut self, id: &str) {
        self.last = Some(id.to_string());
        self.recent.retain(|r| r != id);
        self.recent.insert(0, id.to_string());
        self.recent.truncate(MAX_RECENT);
    }

    /// The volume `id` plays at.
    pub fn volume_of(&self, id: &str) -> f32 {
        self.volumes.get(id).copied().unwrap_or(self.volume)
    }

    /// Sets the volume (for `id` too when per-station volume is on).
    pub fn set_volume(&mut self, id: Option<&str>, v: f32, per_station: bool) {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { default_volume() };
        match id {
            Some(id) if per_station && self.station(id).is_some() => {
                self.volumes.insert(id.to_string(), v);
            }
            _ => self.volume = v,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_round_trips_and_cleans_itself() {
        let dir = std::env::temp_dir().join(format!("mandara-radio-lib-{}", std::process::id()));
        let path = dir.join("radio.toml");
        let mut lib = Library::default();
        let id = lib.add_custom("Corniche FM", "https://corniche.example/live", Category::Jazz, Some(128), Codec::Mp3).unwrap();
        assert_eq!(lib.toggle_favourite("soma-groove-salad"), Ok(true));
        assert_eq!(lib.toggle_favourite(&id), Ok(true));
        lib.set_volume(Some(&id), 0.4, true);
        lib.set_volume(None, 0.8, true);
        lib.played("soma-groove-salad");
        lib.played(&id);
        lib.muted = true;
        lib.save(&path).unwrap();
        let back = Library::load(&path);
        assert_eq!(back, lib);
        assert_eq!(back.volume_of(&id), 0.4);
        assert_eq!(back.volume_of("fip-jazz"), 0.8);
        assert_eq!(back.recent, [id.clone(), "soma-groove-salad".to_string()]);
        // A hand-edited file with junk is bounded and cleaned.
        let junk = "favourites = [\"nope\", \"fip-jazz\", \"fip-jazz\"]\nrecent = [\"x\"]\nvolume = 7.0\nlast = \"gone\"\n[volumes]\nfip-jazz = -3.0\nghost = 0.5\n[[custom]]\nid = \"custom-1\"\nname = \"\"\nurl = \"ftp://bad\"\n[[custom]]\nid = \"custom-2\"\nname = \"   \"\nurl = \"https://ok.example/s\"\n";
        std::fs::write(&path, junk).unwrap();
        let cleaned = Library::load(&path);
        assert_eq!(cleaned.favourites, ["fip-jazz"]);
        assert!(cleaned.recent.is_empty() && cleaned.last.is_none());
        assert_eq!(cleaned.volume, 1.0);
        assert_eq!(cleaned.volumes.get("fip-jazz"), Some(&0.0));
        assert!(!cleaned.volumes.contains_key("ghost"));
        assert_eq!(cleaned.custom.len(), 1);
        assert_eq!(cleaned.custom[0].name, "https://ok.example/s", "a nameless station is named by its address");
        assert_eq!(cleaned.custom[0].category, Category::Other);
        assert_eq!(Library::load(&dir.join("missing.toml")), Library::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn editing_and_removing_stations() {
        let mut lib = Library::default();
        assert_eq!(lib.add_custom("", "https://a.example/", Category::Pop, None, Codec::Unknown), Err(EditError::NoName));
        assert!(matches!(lib.add_custom("A", "nope", Category::Pop, None, Codec::Unknown), Err(EditError::Url(_))));
        assert_eq!(lib.add_custom("Dup", CURATED[0].url, Category::Pop, None, Codec::Unknown), Err(EditError::Exists));
        let a = lib.add_custom("A", "https://a.example/", Category::Pop, Some(64), Codec::Aac).unwrap();
        let b = lib.add_custom("B", "https://b.example/", Category::Rock, None, Codec::Unknown).unwrap();
        assert_eq!(lib.add_custom("A again", "https://a.example/", Category::Pop, None, Codec::Unknown), Err(EditError::Exists));
        assert_eq!(lib.edit_custom(&a, "A2", "https://b.example/", Category::Pop), Err(EditError::Exists));
        lib.edit_custom(&a, "A2", "https://a2.example/", Category::Jazz).unwrap();
        let s = lib.station(&a).unwrap();
        assert_eq!((s.name(), s.url(), s.category(), s.kbps(), s.codec()), ("A2", "https://a2.example/", Category::Jazz, None, Codec::Unknown), "a new address forgets what was measured");
        assert_eq!(s.id(), a, "the id stays");
        assert_eq!(lib.edit_custom("custom-nope", "X", "https://x.example/", Category::Pop), Err(EditError::Unknown));
        lib.toggle_favourite(&a).unwrap();
        lib.toggle_favourite(&b).unwrap();
        lib.toggle_favourite("fip-rock").unwrap();
        assert!(lib.move_favourite("fip-rock", -5));
        assert_eq!(lib.favourites, ["fip-rock".to_string(), a.clone(), b.clone()]);
        assert!(!lib.move_favourite("fip-rock", -1), "already first");
        assert!(lib.move_favourite(&a, 1));
        assert_eq!(lib.favourites, ["fip-rock".to_string(), b.clone(), a.clone()]);
        assert_eq!(lib.toggle_favourite("not-a-station"), Err(EditError::Unknown));
        lib.played(&a);
        lib.set_volume(Some(&a), 0.2, true);
        assert!(lib.remove_custom(&a));
        assert!(!lib.remove_custom(&a));
        assert!(!lib.is_favourite(&a) && lib.recent.is_empty() && lib.last.is_none() && !lib.volumes.contains_key(&a));
        assert!(!lib.remove_custom("fip-rock"), "curated stations can't be removed");
        for e in [EditError::NoName, EditError::Full, EditError::Exists, EditError::Unknown] {
            assert!(!e.message().is_empty());
        }
    }

    #[test]
    fn lists_are_bounded() {
        let mut lib = Library::default();
        for i in 0..MAX_CUSTOM {
            lib.add_custom(&format!("S{i}"), &format!("https://s{i}.example/"), Category::Other, None, Codec::Unknown).unwrap();
        }
        assert_eq!(lib.add_custom("One more", "https://more.example/", Category::Other, None, Codec::Unknown), Err(EditError::Full));
        for s in CURATED.iter().take(20) {
            lib.played(s.id);
        }
        assert_eq!(lib.recent.len(), MAX_RECENT);
        assert_eq!(lib.recent[0], CURATED[19].id);
    }

    #[test]
    fn searching_stations() {
        let lib = Library::default();
        let s = lib.station("monte-carlo-doualiya").unwrap();
        assert!(s.matches(&["monte", "carlo"], "Arabic"));
        assert!(s.matches(&["arabic"], ""));
        assert!(s.matches(&["عربي"], "عربي"), "the shelf's translated name");
        assert!(s.matches(&["مونت"], ""));
        assert!(s.matches(&["fr"], ""), "country code");
        assert!(!s.matches(&["jazz"], "Arabic"));
        assert!(lib.station("soma-groove-salad").unwrap().matches(&["somafm", "groove"], ""));
        assert!(contains_ci("Café del Mar", "café"));
        assert!(contains_ci("ÄRZTE", "ärzte"));
        assert!(!contains_ci("abc", "abcd"));
        assert!(contains_ci("x", ""));
        assert_eq!(lib.all().count(), CURATED.len());
    }
}
