//! Settings for integrations that load only when their panel is visible.

use mandara_i18n::tk;
use mandara_ui::plugins::feeds;
use mandara_ui::plugins::home_assistant as home;
use mandara_ui::plugins::phone_connect as phone;
use mandara_ui::plugins::reddit;
use serde_json::Value;

use super::{Control, Section, Setting, sd};

/// Declare config-backed choices; credential values belong in the keyring editor.
pub(super) fn entries() -> Vec<Setting> {
    vec![
        sd(Section::Plugins, tk("Translator"), "/plugins/settings/translator/provider", tk("Provider"), tk("Text is sent only when you press Translate"), Control::Choice(const { &[("deepl-free", tk("DeepL Free")), ("deepl-pro", tk("DeepL Pro")), ("libretranslate", tk("LibreTranslate"))] }), Value::from("deepl-free")),
        sd(Section::Plugins, tk("Translator"), "/plugins/settings/translator/server", tk("Server address"), tk("Your LibreTranslate server, including a reverse proxy path if needed"), Control::Text { placeholder: "http://localhost:5000" }, Value::from("")),
        sd(Section::Plugins, tk("Translator"), "/plugins/settings/translator/use_api_key", tk("Use an API key"), tk("Enable this when your LibreTranslate server requires a key; save it in the keyring below"), Control::Toggle, Value::from(false)),
        sd(Section::Plugins, tk("Translator"), "/plugins/settings/translator/target", tk("Default target language"), tk("Initial target language code; choose from the provider's language list in the panel"), Control::Text { placeholder: "EN" }, Value::from(mandara_ui::plugins::translator::DEFAULT_TARGET)),
        sd(Section::Plugins, tk("Feeds · Reading"), "/plugins/settings/feeds/open_in", tk("Open articles in"), tk("The reader shows a clean copy of the article, without scripts, frames or trackers"), Control::Choice(const { &[("reader", tk("The reader")), ("browser", tk("Your web browser"))] }), Value::from("reader")),
        sd(Section::Plugins, tk("Feeds · Reading"), "/plugins/settings/feeds/mark_read_on_open", tk("Mark read when opened"), tk("Opening an article marks it read; you can always mark it unread again"), Control::Toggle, Value::from(true)),
        sd(Section::Plugins, tk("Feeds · Reading"), "/plugins/settings/feeds/images", tk("Pictures"), tk("Feed icons and thumbnails come with the feed. Pictures inside articles load from their websites only when you allow them"), Control::Choice(const { &[("off", tk("None")), ("thumbnails", tk("Icons and thumbnails")), ("all", tk("Also inside articles"))] }), Value::from("thumbnails")),
        sd(Section::Plugins, tk("Feeds · Refreshing"), "/plugins/settings/feeds/refresh_seconds", tk("Refresh while open"), tk("Feeds older than this refresh when you open the panel; the Refresh button fetches all of them at once"), Control::Slider { min: feeds::MIN_REFRESH_SECS as f64, max: feeds::MAX_REFRESH_SECS as f64, step: feeds::MIN_REFRESH_SECS as f64, unit: " s" }, Value::from(feeds::DEFAULT_REFRESH_SECS)),
        sd(Section::Plugins, tk("Feeds · Refreshing"), "/plugins/settings/feeds/background_refresh", tk("Refresh in the background"), tk("Keep the unread count current while the panel is closed, at a low interval; off, nothing downloads until you open Feeds"), Control::Toggle, Value::from(false)),
        sd(Section::Plugins, tk("Feeds · Refreshing"), "/plugins/settings/feeds/background_minutes", tk("Background interval"), tk("How often the background refresh runs; feeds that failed wait longer before trying again"), Control::Slider { min: feeds::MIN_BACKGROUND_MIN as f64, max: feeds::MAX_BACKGROUND_MIN as f64, step: feeds::MIN_BACKGROUND_MIN as f64, unit: " min" }, Value::from(feeds::DEFAULT_BACKGROUND_MIN)),
        sd(Section::Plugins, tk("Feeds · Refreshing"), "/plugins/settings/feeds/notify", tk("Notify about new articles"), tk("One notification for what is new, at most every 30 minutes, and never while the panel is open"), Control::Toggle, Value::from(false)),
        sd(Section::Plugins, tk("Feeds · History"), "/plugins/settings/feeds/items_per_feed", tk("Articles per feed"), tk("The newest articles kept for each feed; saved articles are kept on top of these"), Control::Slider { min: feeds::MIN_ITEMS as f64, max: feeds::MAX_ITEMS as f64, step: feeds::MIN_ITEMS as f64, unit: "" }, Value::from(feeds::DEFAULT_ITEMS)),
        sd(Section::Plugins, tk("Feeds · History"), "/plugins/settings/feeds/retention_days", tk("Keep old articles for"), tk("Days an article stays after it has left its feed; saved articles stay until you remove them"), Control::Slider { min: feeds::MIN_RETENTION_DAYS as f64, max: feeds::MAX_RETENTION_DAYS as f64, step: 1.0, unit: " d" }, Value::from(feeds::DEFAULT_RETENTION_DAYS)),
        sd(Section::Plugins, tk("Feeds · Bar"), "/plugins/settings/feeds/bar_style", tk("Unread count"), tk("How the bar item shows unread articles"), Control::Choice(const { &[("badge", tk("Badge")), ("count", tk("Number")), ("icon", tk("Dot"))] }), Value::from("badge")),
        sd(Section::Plugins, tk("Feeds · Sidebar card"), "/plugins/settings/feeds/card_folder", tk("Card folder"), tk("The sidebar card lists the newest unread articles of this folder; empty lists every feed"), Control::Text { placeholder: "News" }, Value::from("")),
        sd(Section::Plugins, tk("Reddit"), "/plugins/settings/reddit/client_id", tk("Client id"), tk("Optional. Without it Mandara reads Reddit's public feeds in limited mode (one request a minute, no scores or reply threads). For full mode, create an installed app at reddit.com/prefs/apps and paste the id shown under its name; the panel shows the steps"), Control::Text { placeholder: "a1B2c3D4e5F6g7" }, Value::from("")),
        sd(Section::Plugins, tk("Reddit"), "/plugins/settings/reddit/username", tk("Your Reddit user name"), tk("Optional. Reddit asks apps to name their developer in requests; Mandara never signs in as you"), Control::Text { placeholder: "u/name" }, Value::from("")),
        sd(Section::Plugins, tk("Reddit"), "/plugins/settings/reddit/subreddits", tk("Communities"), tk("Your communities in order, separated by spaces; add, remove and reorder them in the panel's Discover page too"), Control::Text { placeholder: reddit::DEFAULT_SUBSCRIPTIONS }, Value::from(reddit::DEFAULT_SUBSCRIPTIONS)),
        sd(Section::Plugins, tk("Reddit"), "/plugins/settings/reddit/card_subreddit", tk("Sidebar card community"), tk("The community the sidebar card shows; empty follows your first community"), Control::Text { placeholder: "r/linux" }, Value::from("")),
        sd(Section::Plugins, tk("Reddit"), "/plugins/settings/reddit/sort", tk("Sort posts by"), tk("How a community opens; change it any time in the panel"), Control::Choice(const { &[("hot", tk("Hot")), ("new", tk("New")), ("top", tk("Top rated")), ("rising", tk("Rising"))] }), Value::from("hot")),
        sd(Section::Plugins, tk("Reddit"), "/plugins/settings/reddit/span", tk("Top posts from"), tk("The time range of top-rated posts and searches"), Control::Choice(const { &[("hour", tk("Hour")), ("day", tk("Day")), ("week", tk("Week")), ("month", tk("Month")), ("year", tk("Year")), ("all", tk("All time"))] }), Value::from("day")),
        sd(Section::Plugins, tk("Reddit"), "/plugins/settings/reddit/layout", tk("Layout"), tk("Cards show bigger thumbnails and flair, compact fits more posts and the grid puts pictures first; the panel's layout button switches too"), Control::Choice(const { &[("cards", tk("Cards")), ("compact", tk("Compact")), ("grid", tk("Grid"))] }), Value::from("cards")),
        sd(Section::Plugins, tk("Reddit"), "/plugins/settings/reddit/images", tk("Load pictures"), tk("Thumbnails and post pictures, decoded at the size they're shown; off loads text only"), Control::Toggle, Value::from(true)),
        sd(Section::Plugins, tk("Reddit"), "/plugins/settings/reddit/nsfw", tk("NSFW posts"), tk("Hidden by default. Blurred shows them covered until you choose to see one"), Control::Choice(const { &[("hide", tk("Hide")), ("blur", tk("Blur")), ("show", tk("Show"))] }), Value::from("hide")),
        sd(Section::Plugins, tk("Reddit"), "/plugins/settings/reddit/spoilers", tk("Spoilers"), tk("Covered spoilers stay blurred, and spoiler text stays masked, until you reveal them"), Control::Choice(const { &[("blur", tk("Cover")), ("show", tk("Show"))] }), Value::from("blur")),
        sd(Section::Plugins, tk("Reddit"), "/plugins/settings/reddit/refresh_minutes", tk("Refresh an open list"), tk("Off by default. Only the list on screen refreshes, and only while the panel is open"), Control::Slider { min: 0.0, max: reddit::MAX_REFRESH_MINUTES as f64, step: reddit::REFRESH_STEP_MINUTES as f64, unit: " min" }, Value::from(0)),
        sd(Section::Plugins, tk("Home Assistant"), "/plugins/settings/home-assistant/server", tk("Server address"), tk("Your Home Assistant address, including a reverse proxy path if needed"), Control::Text { placeholder: "http://192.168.1.10:8123" }, Value::from("")),
        sd(Section::Plugins, tk("Home Assistant"), "/plugins/settings/home-assistant/entities", tk("Favourite entities"), tk("Separate up to 24 entity ids with spaces; empty shows the first available entities"), Control::Text { placeholder: "light.studio sensor.temperature scene.evening" }, Value::from("")),
        sd(Section::Plugins, tk("Home Assistant"), "/plugins/settings/home-assistant/refresh_seconds", tk("Refresh interval"), tk("Read your home while the panel is open; actions also refresh its state"), Control::Slider { min: home::MIN_REFRESH_SECS as f64, max: home::MAX_REFRESH_SECS as f64, step: home::MIN_REFRESH_SECS as f64, unit: " s" }, Value::from(home::DEFAULT_REFRESH_SECS)),
        sd(Section::Plugins, tk("Phone Connect"), "/plugins/settings/phone-connect/refresh_seconds", tk("Refresh interval"), tk("Check KDE Connect while the phone panel is open"), Control::Slider { min: phone::MIN_REFRESH_SECS as f64, max: phone::MAX_REFRESH_SECS as f64, step: phone::MIN_REFRESH_SECS as f64, unit: " s" }, Value::from(phone::DEFAULT_REFRESH_SECS)),
        sd(Section::Plugins, tk("Phone Connect"), "/plugins/settings/phone-connect/show_unpaired", tk("Show available devices"), tk("Include nearby devices that can be paired; pairing always needs your confirmation"), Control::Toggle, Value::from(true)),
    ]
}

/// Explain controls that do not apply to the selected integration provider.
pub(super) fn locked(path: &str, cfg: &mandara_config::ShellConfig) -> Option<&'static str> {
    if path == "/plugins/settings/feeds/background_minutes" && feeds::settings(cfg).background.is_none() {
        return Some(tk("Turn on Refresh in the background first"));
    }
    (matches!(path, "/plugins/settings/translator/server" | "/plugins/settings/translator/use_api_key")
        && mandara_ui::plugins::translator::settings(cfg).connection.provider != mandara_services::translator::Provider::LibreTranslate)
        .then_some(tk("DeepL uses its own server and requires an API key"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_specific_controls_explain_when_they_do_not_apply() {
        let mut cfg = mandara_config::ShellConfig::default();
        assert!(locked("/plugins/settings/translator/server", &cfg).is_some());
        assert!(locked("/plugins/settings/translator/provider", &cfg).is_none());
        cfg.plugins.settings.entry("translator".into()).or_default().insert("provider".into(), "libretranslate".into());
        assert!(locked("/plugins/settings/translator/server", &cfg).is_none());
        assert!(locked("/plugins/settings/translator/use_api_key", &cfg).is_none());
    }
}
