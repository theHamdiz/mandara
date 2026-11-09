//! Small resolved settings shared by views of one plugin.

use std::rc::Rc;
use std::sync::{Arc, Weak};

use mandara_config::ShellConfig;
use mandara_i18n::Lang;

/// Resolves immutable configuration plus bounded inputs such as the UTC offset.
/// Live service data needs its own revision key and must not be cached by config alone.
pub(super) trait ConfigSettings: Sized {
    type Scope: Copy + Eq;

    /// Resolves the settings from `config` in `language` for `scope`; runs
    /// only on a cache miss.
    fn resolve(config: &ShellConfig, language: Lang, scope: Self::Scope) -> Self;
}

/// The one cached value with every key it depends on.
struct Entry<T: ConfigSettings> {
    config: Weak<ShellConfig>,
    language: Lang,
    scope: T::Scope,
    value: Rc<T>,
}

/// Holds no strong config reference. After the last strong owner drops, config
/// fields are destroyed, but its inline Arc allocation remains until this weak
/// key is replaced or cleared. Cache values should contain only resolved settings.
pub(super) struct SettingsCache<T: ConfigSettings> {
    entry: Option<Entry<T>>,
}

impl<T: ConfigSettings> Default for SettingsCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: ConfigSettings> SettingsCache<T> {
    /// An empty cache (a `const`, for statics and thread-locals).
    pub const fn new() -> Self {
        Self { entry: None }
    }

    /// Reads a warm value without parsing TOML or copying its owned text and lists.
    pub fn get(&mut self, config: &Arc<ShellConfig>, language: Lang, scope: T::Scope) -> Rc<T> {
        if let Some(entry) = &self.entry
            && entry.config.as_ptr() == Arc::as_ptr(config)
            && entry.language == language
            && entry.scope == scope
        {
            return entry.value.clone();
        }
        let value = Rc::new(T::resolve(config, language, scope));
        self.entry = Some(Entry {
            config: Arc::downgrade(config),
            language,
            scope,
            value: value.clone(),
        });
        value
    }

    /// Also releases the weak key's inline config allocation when a plugin stops.
    pub fn clear(&mut self) {
        self.entry = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Probe {
        name: String,
        language: Lang,
        offset: u64,
    }

    impl ConfigSettings for Probe {
        type Scope = u64;
        fn resolve(config: &ShellConfig, language: Lang, offset: u64) -> Self {
            Self {
                name: config.location.name.clone(),
                language,
                offset,
            }
        }
    }

    /// Snapshot, locale and non-config scope changes each rebuild once.
    #[test]
    fn settings_follow_snapshot_language_scope_and_clear() {
        let config = Arc::new(ShellConfig::default());
        let mut cache = SettingsCache::<Probe>::new();
        let initial = cache.get(&config, Lang::En, 2);
        assert!(Rc::ptr_eq(&initial, &cache.get(&config, Lang::En, 2)));
        let localized = cache.get(&config, Lang::Ar, 2);
        assert!(!Rc::ptr_eq(&initial, &localized));
        assert_eq!(localized.language, Lang::Ar);
        let summer = cache.get(&config, Lang::Ar, 3);
        assert!(!Rc::ptr_eq(&localized, &summer));
        assert_eq!(summer.offset, 3);
        let mut changed = (*config).clone();
        changed.location.name = "Demo place".into();
        let changed = Arc::new(changed);
        let current = cache.get(&changed, Lang::Ar, 3);
        assert_eq!(current.name, "Demo place");
        assert_eq!(Arc::strong_count(&changed), 1);
        let weak = Arc::downgrade(&changed);
        drop(changed);
        assert!(
            weak.upgrade().is_none(),
            "settings cannot keep config fields alive"
        );
        cache.clear();
        assert!(cache.entry.is_none());
        assert!(!Rc::ptr_eq(&initial, &cache.get(&config, Lang::En, 2)));
        eprintln!(
            "settings cache: ShellConfig inline={} B, cache={} B, Entry<Probe>={} B; resolved Probe={} B",
            size_of::<ShellConfig>(),
            size_of::<SettingsCache<Probe>>(),
            size_of::<Entry<Probe>>(),
            size_of::<Probe>()
        );
    }
}
