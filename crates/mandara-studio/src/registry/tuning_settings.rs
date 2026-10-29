//! Registry entries for values that used to be fixed in the shell: plugin
//! refresh intervals, list limits and alert thresholds (each in its
//! plugin's own `[plugins.settings.<id>]` table, shown in the plugin's
//! settings modal), the drop-down panels' close delay and action messages,
//! and the lock screen's low-battery mark.
//!
//! Defaults and bounds are the consumers' own named constants, so Studio's
//! sliders and the shell's clamps never disagree.

use mandara_config::desktop::defaults as desktop;
use mandara_config::surfaces::ranges;
use mandara_i18n::tk;
use mandara_ui::plugins::{authenticator, bluetooth, clock, containers, drives, net_speed, network, notes, processes, resources, services, todo};
use serde_json::Value;

use super::surface_settings::PANELS;
use super::{Control, Section, Setting, s, sd, span};

/// Plugin groups: the plugins' English names (matched against manifests).
const RESOURCES: &str = tk("Resources");
const PROCESSES: &str = tk("Process hunter");
const CONTAINERS: &str = tk("Containers");
const SERVICES: &str = tk("Services");
const DRIVES: &str = tk("Drives");
const NET_SPEED: &str = tk("Network speed");
const NETWORK: &str = tk("Network");
const CLOCK: &str = tk("Clock");
const TODO: &str = tk("To-do");
const NOTES: &str = tk("Notes");
const BLUETOOTH: &str = tk("Bluetooth");
const AUTHENTICATOR: &str = tk("Authenticator");

/// Shared labels.
const REFRESH: &str = tk("Refresh every");
const HOT_CPU: &str = tk("Hot CPU at");

/// Slider steps (bounds come from the consumers' ranges).
const MS_STEP: f64 = 250.0;
const S_STEP: f64 = 1.0;
const COARSE_S_STEP: f64 = 10.0;
const COUNT_STEP: f64 = 1.0;
const PERCENT_STEP: f64 = 1.0;
const HOT_CPU_STEP: f64 = 5.0;

/// A slider over an integer `(min, max)` range.
fn ints((min, max): (i64, i64), step: f64, unit: &'static str) -> Control {
    Control::Slider { min: min as f64, max: max as f64, step, unit }
}

/// A plugin-table setting with its default.
fn plugin(group: &'static str, path: &'static str, label: &'static str, help: &'static str, control: Control, default: i64) -> Setting {
    sd(Section::Plugins, group, path, label, help, control, Value::from(default))
}

pub fn entries() -> Vec<Setting> {
    use Section::*;
    vec![
        // Resources
        plugin(RESOURCES, "/plugins/settings/resources/refresh_ms", REFRESH, tk("How often CPU, memory, temperatures and the busiest apps are sampled (the bar rings too)"), ints(resources::REFRESH_MS_RANGE, MS_STEP, " ms"), resources::DEFAULT_REFRESH_MS),
        plugin(RESOURCES, "/plugins/settings/resources/disks_every_s", tk("Disk usage every"), tk("How often filesystem usage is read again while the panel is open"), ints(resources::DISKS_EVERY_S_RANGE, S_STEP, " s"), resources::DEFAULT_DISKS_EVERY_S),
        plugin(RESOURCES, "/plugins/settings/resources/top_apps", tk("Busiest apps"), tk("Apps listed under TOP APPS"), ints(resources::TOP_APPS_RANGE, COUNT_STEP, ""), resources::DEFAULT_TOP_APPS),
        plugin(RESOURCES, "/plugins/settings/resources/temps_shown", tk("Temperatures shown"), tk("Sensors listed before the list folds"), ints(resources::TEMPS_SHOWN_RANGE, COUNT_STEP, ""), resources::DEFAULT_TEMPS_SHOWN),
        plugin(RESOURCES, "/plugins/settings/resources/disk_warn_percent", tk("Disk warning at"), tk("A filesystem this full turns amber"), ints(resources::DISK_PERCENT_RANGE, PERCENT_STEP, " %"), resources::DEFAULT_DISK_WARN_PERCENT),
        plugin(RESOURCES, "/plugins/settings/resources/disk_full_percent", tk("Disk full at"), tk("A filesystem this full turns coral"), ints(resources::DISK_PERCENT_RANGE, PERCENT_STEP, " %"), resources::DEFAULT_DISK_FULL_PERCENT),
        // Process hunter
        plugin(PROCESSES, "/plugins/settings/processes/refresh_ms", REFRESH, tk("How often the process table is sampled while the panel is open"), ints(processes::REFRESH_MS_RANGE, MS_STEP, " ms"), processes::DEFAULT_REFRESH_MS),
        plugin(PROCESSES, "/plugins/settings/processes/hot_cpu_percent", HOT_CPU, tk("A process using this much CPU (percent of one core) is highlighted"), ints(processes::HOT_CPU_RANGE, HOT_CPU_STEP, " %"), processes::DEFAULT_HOT_CPU_PERCENT),
        // Containers
        plugin(CONTAINERS, "/plugins/settings/containers/hot_cpu_percent", HOT_CPU, tk("A container using this much CPU (percent of one core) is highlighted"), ints(containers::HOT_CPU_RANGE, HOT_CPU_STEP, " %"), containers::DEFAULT_HOT_CPU_PERCENT),
        // Services
        plugin(SERVICES, "/plugins/settings/services/list_refresh_s", REFRESH, tk("How often the unit list is reloaded while the panel is open"), ints(services::LIST_REFRESH_S_RANGE, S_STEP, " s"), services::DEFAULT_LIST_REFRESH_S),
        plugin(SERVICES, "/plugins/settings/services/failed_check_s", tk("Check for failed units every"), tk("How often failed units are counted for the chip's badge"), ints(services::FAILED_CHECK_S_RANGE, COARSE_S_STEP, " s"), services::DEFAULT_FAILED_CHECK_S),
        // Drives
        plugin(DRIVES, "/plugins/settings/drives/usage_every_s", REFRESH, tk("How often drive usage is read again while the panel is open"), ints(drives::USAGE_EVERY_S_RANGE, S_STEP, " s"), drives::DEFAULT_USAGE_EVERY_S),
        plugin(DRIVES, "/plugins/settings/drives/full_warn_percent", tk("Nearly full at"), tk("A volume this full turns its ring amber"), ints(drives::FULL_WARN_RANGE, PERCENT_STEP, " %"), drives::DEFAULT_FULL_WARN_PERCENT),
        // Network speed / Network
        plugin(NET_SPEED, "/plugins/settings/net-speed/interval_s", REFRESH, tk("How often transfer rates are measured"), ints(net_speed::INTERVAL_S_RANGE, S_STEP, " s"), net_speed::DEFAULT_INTERVAL_S),
        plugin(NETWORK, "/plugins/settings/network/networks_shown", tk("Networks shown"), tk("Other Wi-Fi networks listed in the panel"), ints(network::NETWORKS_SHOWN_RANGE, COUNT_STEP, ""), network::DEFAULT_NETWORKS_SHOWN),
        // Clock / To-do
        plugin(CLOCK, "/plugins/settings/clock/agenda_rows", tk("Agenda events"), tk("Upcoming events listed under the calendar"), ints(clock::AGENDA_ROWS_RANGE, COUNT_STEP, ""), clock::DEFAULT_AGENDA_ROWS),
        plugin(TODO, "/plugins/settings/todo/panel_rows", tk("Tasks in the panel"), tk("Tasks listed at most"), ints(todo::ROWS_RANGE, COUNT_STEP, ""), todo::DEFAULT_PANEL_ROWS),
        plugin(TODO, "/plugins/settings/todo/card_rows", tk("Tasks in the sidebar card"), tk("Tasks listed at most"), ints(todo::ROWS_RANGE, COUNT_STEP, ""), todo::DEFAULT_CARD_ROWS),
        sd(Plugins, TODO, "/plugins/settings/todo/default_list", tk("Default list"), tk("Where new tasks go while the card shows All (a list's name; empty: Inbox)"), Control::Text { placeholder: tk("Inbox") }, Value::from("")),
        sd(Plugins, TODO, "/plugins/settings/todo/show_completed", tk("Show completed"), tk("A folding Completed group under each list"), Control::Toggle, Value::from(todo::DEFAULT_SHOW_COMPLETED)),
        sd(Plugins, TODO, "/plugins/settings/todo/sort", tk("Sort tasks"), tk("Order inside each list (open tasks stay above done ones)"), Control::Choice(const { &[("newest", tk("Newest first")), ("oldest", tk("Oldest first")), ("alphabetical", tk("A to Z"))] }), Value::from("newest")),
        sd(Plugins, TODO, "/plugins/settings/todo/bar_count", tk("Bar count"), tk("Open tasks the bar item counts: every list, or one list's name"), Control::Text { placeholder: tk("All lists") }, Value::from("")),
        // Notes
        sd(Plugins, NOTES, "/plugins/settings/notes/default_topic", tk("Default topic"), tk("Where new notes go while the card shows All (a topic's name; empty: Notes)"), Control::Text { placeholder: tk("Notes") }, Value::from("")),
        sd(Plugins, NOTES, "/plugins/settings/notes/storage_folder", tk("Notes folder"), tk("Each note is a Markdown file, each topic a folder in here — point it at a synced folder or an Obsidian vault (empty: Mandara's data folder)"), Control::Text { placeholder: tk("~/.local/share/mandara/notes") }, Value::from("")),
        sd(Plugins, NOTES, "/plugins/settings/notes/preview_by_default", tk("Open notes as preview"), tk("Show the rendered Markdown first; the pencil switches to the editor"), Control::Toggle, Value::from(notes::DEFAULT_PREVIEW)),
        sd(Plugins, NOTES, "/plugins/settings/notes/editor_font", tk("Editor font"), tk("Typeface of the note editor"), Control::Choice(const { &[("sans", tk("Sans")), ("mono", tk("Monospace"))] }), Value::from("sans")),
        plugin(NOTES, "/plugins/settings/notes/editor_size", tk("Editor text size"), tk("Text size of the note editor"), ints(notes::FONT_SIZE_RANGE, COUNT_STEP, " px"), notes::DEFAULT_FONT_SIZE),
        // Bluetooth / Authenticator
        plugin(BLUETOOTH, "/plugins/settings/bluetooth/battery_warn_percent", tk("Battery warning at"), tk("A device battery this low turns amber; Power → peripheral alerts sets when it turns coral"), ints(bluetooth::BATTERY_WARN_RANGE, PERCENT_STEP, " %"), bluetooth::DEFAULT_BATTERY_WARN_PERCENT),
        plugin(AUTHENTICATOR, "/plugins/settings/authenticator/visible_rows", tk("Visible accounts"), tk("Rows shown before the list scrolls"), ints(authenticator::VISIBLE_ROWS_RANGE, COUNT_STEP, ""), authenticator::DEFAULT_VISIBLE_ROWS),
        // Bar → Drop-down panels
        s(Bar, PANELS, "/panels/leave_delay_ms", tk("Close delay"), tk("How long hover-opened panels, the launcher and the session menu wait after the pointer leaves"), span(ranges::LEAVE_DELAY_MS, " ms")),
        s(Bar, PANELS, "/panels/toast_ms", tk("Action messages"), tk("How long a panel's result message (stopped, copied, failed…) stays up"), span(ranges::TOAST_MS, " ms")),
        // Lock appearance
        s(LockScreen, tk("Show"), "/lock/low_battery_percent", tk("Low battery at"), tk("The battery reading turns coral at or below this charge"), ints((i64::from(desktop::LOCK_LOW_BATTERY_RANGE.0), i64::from(desktop::LOCK_LOW_BATTERY_RANGE.1)), PERCENT_STEP, " %")),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every default sits inside its slider and on its step grid.
    #[test]
    fn defaults_fit_their_sliders() {
        for e in entries() {
            let Control::Slider { min, max, step, .. } = e.control else { continue };
            let Some(v) = e.default.as_ref().and_then(Value::as_f64) else { continue };
            assert!((min..=max).contains(&v), "{}: {v} outside {min}..{max}", e.path);
            assert!(((v - min) / step).fract().abs() < 1e-9, "{}: {v} off the {step} grid from {min}", e.path);
        }
    }
}
