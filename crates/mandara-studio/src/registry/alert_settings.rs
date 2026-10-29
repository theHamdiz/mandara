//! Registry entries for Notifications, On-screen displays, Privacy and the
//! audio additions (volume step, over-amplification, media players, shell
//! sounds). The bespoke parts (notification rules, ignore lists, device
//! names, per-app outputs) live in `app/{notifications,osd,audio}_page.rs`.

use mandara_i18n::tk;

use super::{Control, Section, Setting, s};

const POSITIONS: &[(&str, &str)] = &[
    ("top-left", tk("Top left")),
    ("top-center", tk("Top center")),
    ("top-right", tk("Top right")),
    ("bottom-left", tk("Bottom left")),
    ("bottom-center", tk("Bottom center")),
    ("bottom-right", tk("Bottom right")),
];
const URGENCIES: &[(&str, &str)] = &[("low", tk("Everything")), ("normal", tk("Normal and critical")), ("critical", tk("Critical only"))];
pub const OSD_POSITIONS: &[(&str, &str)] = &[("bottom", tk("Bottom")), ("top", tk("Top")), ("center", tk("Center")), ("left", tk("Left (vertical)")), ("right", tk("Right (vertical)"))];

/// Group names (the bespoke pages pick rows by them).
pub const POPUPS: &str = tk("Popups");
pub const TIMEOUTS: &str = tk("How long popups stay");
pub const DND: &str = tk("Do not disturb");
pub const HISTORY: &str = tk("History");
pub const SERVER: &str = tk("Notification server");
pub const TRY: &str = tk("Try it");
pub const OSD_LOOK: &str = tk("Display");
pub const OSD_KINDS: &str = tk("Show for");
pub const INDICATORS: &str = tk("Indicators");
pub const VOLUME: &str = tk("Volume");
pub const PLAYERS: &str = tk("Media players");
pub const SOUNDS: &str = tk("Sounds");
pub const SOUND_EVENTS: &str = tk("Play a sound for");

pub fn entries() -> Vec<Setting> {
    use Control::*;
    use Section::*;
    let slider = |min: f64, max: f64, step: f64, unit: &'static str| Slider { min, max, step, unit };
    vec![
        // Notifications
        s(Notifications, POPUPS, "/notifications/position", tk("Position"), tk("Where popups stack; they always stay clear of the bar"), Choice(POSITIONS)),
        s(Notifications, POPUPS, "/notifications/max_visible", tk("Popups at once"), tk("More wait their turn; all of them are in the history"), slider(1.0, 8.0, 1.0, "")),
        s(Notifications, POPUPS, "/notifications/width", tk("Width"), tk("Popup width"), slider(300.0, 560.0, 4.0, " px")),
        s(Notifications, POPUPS, "/notifications/scale", tk("Size"), tk("Scales popup text, icons and spacing"), slider(0.75, 1.5, 0.05, "×")),
        s(Notifications, POPUPS, "/notifications/opacity", tk("Background opacity"), tk("Lower lets windows show through popups"), slider(0.3, 1.0, 0.05, "")),
        s(Notifications, POPUPS, "/notifications/over_fullscreen", tk("Show over fullscreen apps"), tk("Popups appear above games and videos in fullscreen"), Toggle),
        s(Notifications, POPUPS, "/notifications/show_app_name", tk("App name"), tk("The sending app's name above the title"), Toggle),
        s(Notifications, POPUPS, "/notifications/show_actions", tk("Action buttons"), tk("Buttons apps attach, like Reply or Open"), Toggle),
        s(Notifications, POPUPS, "/notifications/click_action", tk("Click opens the app"), tk("Clicking a popup runs its default action"), Toggle),
        s(Notifications, POPUPS, "/notifications/compact", tk("Compact"), tk("One line of body text in popups"), Toggle),
        s(Notifications, POPUPS, "/notifications/markup", tk("Rich text"), tk("Bold, italic and links in notification text"), Toggle),
        s(Notifications, POPUPS, "/notifications/privacy", tk("Hide message text in popups"), tk("Popups show the app and title only; open the notification center to read"), Toggle),
        s(Notifications, POPUPS, "/notifications/progress_bar", tk("Countdown line"), tk("A thin line along a popup's edge shows the time left"), Toggle),
        s(Notifications, POPUPS, "/notifications/pause_on_hover", tk("Pause on hover"), tk("A popup under the pointer doesn't time out"), Toggle),
        s(Notifications, TIMEOUTS, "/notifications/timeout_low", tk("Low urgency"), tk("Seconds on screen; 0 keeps it until you close it"), slider(0.0, 30.0, 0.5, " s")),
        s(Notifications, TIMEOUTS, "/notifications/timeout_normal", tk("Normal"), tk("Seconds on screen; 0 keeps it until you close it"), slider(0.0, 30.0, 0.5, " s")),
        s(Notifications, TIMEOUTS, "/notifications/timeout_critical", tk("Critical"), tk("Seconds on screen; 0 keeps it until you close it"), slider(0.0, 60.0, 1.0, " s")),
        s(Notifications, TIMEOUTS, "/notifications/respect_app_timeout", tk("Let apps choose"), tk("Use the time an app asks for instead of the values above"), Toggle),
        s(Notifications, DND, "/notifications/dnd_allow_critical", tk("Critical alerts break through"), tk("Critical notifications still pop up while do not disturb is on"), Toggle),
        s(Notifications, DND, "/notifications/dnd_screen_share", tk("While sharing the screen"), tk("Quiet while an app shares or records your screen (screen-cast portal and Mandara's recorder)"), Toggle),
        s(Notifications, DND, "/notifications/dnd_fullscreen", tk("While a fullscreen app is focused"), tk("Quiet during games, videos and presentations"), Toggle),
        s(Notifications, DND, "/notifications/quiet_hours", tk("Quiet hours"), tk("Do not disturb every day between the times below"), Toggle),
        s(Notifications, DND, "/notifications/quiet_from", tk("From"), tk("Quiet hours start"), Time),
        s(Notifications, DND, "/notifications/quiet_to", tk("Until"), tk("Quiet hours end"), Time),
        s(Notifications, HISTORY, "/notifications/history", tk("Keep a history"), tk("Closed popups stay in the notification center"), Toggle),
        s(Notifications, HISTORY, "/notifications/history_max", tk("Entries kept"), tk("The oldest go first"), slider(10.0, 1000.0, 10.0, "")),
        s(Notifications, HISTORY, "/notifications/history_max_age_hours", tk("Forget after"), tk("Hours; 0 keeps them until you clear them"), slider(0.0, 720.0, 1.0, " h")),
        s(Notifications, HISTORY, "/notifications/history_low", tk("Keep low-urgency ones"), tk("Off: low-urgency notifications vanish when their popup closes"), Toggle),
        s(Notifications, HISTORY, "/notifications/keep_dismissed", tk("Closing a popup keeps it"), tk("The × on a popup only hides it; it stays in the history"), Toggle),
        s(Notifications, HISTORY, "/notifications/dedupe", tk("Fold repeats"), tk("The same message from the same app replaces the earlier one with a ×2 badge"), Toggle),
        s(Notifications, HISTORY, "/notifications/min_urgency", tk("Pop up for"), tk("Below this, notifications go straight to the history"), Choice(URGENCIES)),
        s(Notifications, SERVER, "/notifications/daemon", tk("Mandara handles notifications"), tk("Off lets another daemon (mako, swaync, dunst) take over; Mandara releases the name at once"), Toggle),
        s(Notifications, TRY, "/notifications/timeout_low", tk("Low urgency"), tk("Send a sample notification"), Command { button: tk("Send"), args: &["notifications", "send", "--low", "Mandara", "A quiet note"] }),
        s(Notifications, TRY, "/notifications/timeout_normal", tk("Normal"), tk("Send a sample notification"), Command { button: tk("Send"), args: &["notifications", "send", "Mandara", "Something happened"] }),
        s(Notifications, TRY, "/notifications/timeout_critical", tk("Critical"), tk("Send a sample notification"), Command { button: tk("Send"), args: &["notifications", "send", "--critical", "Mandara", "This needs you"] }),
        // On-screen displays
        s(Osd, OSD_LOOK, "/general/osd", tk("Show on-screen displays"), tk("A brief pill when volume, brightness, keyboard layout and more change"), Toggle),
        s(Osd, OSD_LOOK, "/general/osd_position", tk("Position"), tk("Always clear of the bar; left and right draw a vertical pill"), Choice(OSD_POSITIONS)),
        s(Osd, OSD_LOOK, "/osd/hold_ms", tk("Stays for"), tk("After the last change"), slider(500.0, 5000.0, 100.0, " ms")),
        s(Osd, OSD_LOOK, "/osd/scale", tk("Size"), tk("Scales the pill"), slider(0.6, 1.8, 0.05, "×")),
        s(Osd, OSD_LOOK, "/osd/opacity", tk("Background opacity"), tk("Lower lets windows show through"), slider(0.3, 1.0, 0.05, "")),
        s(Osd, OSD_LOOK, "/osd/show_value", tk("Show the value"), tk("The percentage next to level bars"), Toggle),
        s(Osd, OSD_LOOK, "/osd/offset_x", tk("Horizontal offset"), tk("Extra distance from the left / right edge"), slider(0.0, 200.0, 2.0, " px")),
        s(Osd, OSD_LOOK, "/osd/offset_y", tk("Vertical offset"), tk("Extra distance from the top / bottom edge (centre: moves it down)"), slider(-200.0, 200.0, 2.0, " px")),
        s(Osd, OSD_KINDS, "/osd/kinds/volume", tk("Volume"), tk("Output volume and mute"), Toggle),
        s(Osd, OSD_KINDS, "/osd/kinds/microphone", tk("Microphone"), tk("Microphone mute"), Toggle),
        s(Osd, OSD_KINDS, "/osd/kinds/output_device", tk("Sound output"), tk("Switching speakers / headphones"), Toggle),
        s(Osd, OSD_KINDS, "/osd/kinds/brightness", tk("Brightness"), tk("Screen backlight"), Toggle),
        s(Osd, OSD_KINDS, "/osd/kinds/keyboard_backlight", tk("Keyboard backlight"), tk("Laptops with a lit keyboard"), Toggle),
        s(Osd, OSD_KINDS, "/osd/kinds/keyboard_layout", tk("Keyboard layout"), tk("Switching input languages"), Toggle),
        s(Osd, OSD_KINDS, "/osd/kinds/lock_keys", tk("Lock keys"), tk("Caps Lock, Num Lock and Scroll Lock"), Toggle),
        s(Osd, OSD_KINDS, "/osd/kinds/media", tk("Now playing"), tk("The new track when music changes"), Toggle),
        s(Osd, OSD_KINDS, "/osd/kinds/power_profile", tk("Power mode"), tk("Power saver / balanced / performance"), Toggle),
        s(Osd, OSD_KINDS, "/osd/kinds/do_not_disturb", tk("Do not disturb"), tk("Turning on and off, including automatically"), Toggle),
        s(Osd, OSD_KINDS, "/osd/kinds/night_light", tk("Night light"), tk("Turning on and off"), Toggle),
        s(Osd, OSD_KINDS, "/osd/kinds/privacy", tk("Privacy"), tk("An app starts or stops using the microphone, camera or screen"), Toggle),
        s(Osd, OSD_KINDS, "/osd/kinds/custom", tk("Scripts and plugins"), tk("`mandara msg osd show …`"), Toggle),
        s(Osd, TRY, "/osd/kinds/volume", tk("Volume"), tk("Show the volume display"), Command { button: tk("Show"), args: &["osd", "volume"] }),
        s(Osd, TRY, "/osd/kinds/custom", tk("Custom"), tk("What scripts can show"), Command { button: tk("Show"), args: &["osd", "show", "Mandara", "--icon", "check", "Backup finished"] }),
        // Privacy
        s(Privacy, INDICATORS, "/privacy/indicators", tk("Privacy indicators"), tk("A bar chip (the Privacy plugin) and a pill while apps use your microphone, camera or screen"), Toggle),
        s(Privacy, INDICATORS, "/privacy/microphone", tk("Microphone"), tk("Apps recording sound (meters and visualizers don't count)"), Toggle),
        s(Privacy, INDICATORS, "/privacy/camera", tk("Camera"), tk("Apps with a camera open"), Toggle),
        s(Privacy, INDICATORS, "/privacy/screen_share", tk("Screen sharing"), tk("Screen casts through the desktop portal (tools that capture directly can't be seen)"), Toggle),
        s(Privacy, INDICATORS, "/privacy/camera_poll_s", tk("Camera check interval"), tk("Only used when the system can't report camera use instantly"), slider(1.0, 30.0, 1.0, " s")),
        // Audio & media additions
        s(Audio, VOLUME, "/audio/volume_step", tk("Volume step"), tk("Volume keys, the bar's scroll and `audio volume up / down`"), slider(1.0, 20.0, 1.0, " %")),
        s(Audio, VOLUME, "/audio/overamplify", tk("Allow above 100 %"), tk("Up to 150 %; can distort"), Toggle),
        s(Audio, PLAYERS, "/media/follow_playing", tk("Follow the player that starts playing"), tk("Off: stay with the current player until it closes"), Toggle),
        s(Audio, SOUNDS, "/sounds/enabled", tk("Shell sounds"), tk("Short sounds from your sound theme for the events below"), Toggle),
        s(Audio, SOUNDS, "/sounds/theme", tk("Sound theme"), tk("A folder in /usr/share/sounds; empty follows your GTK setting"), Text { placeholder: "freedesktop" }),
        s(Audio, SOUNDS, "/sounds/volume", tk("Volume"), tk("Of shell sounds only"), slider(0.0, 1.0, 0.05, "")),
        s(Audio, SOUNDS, "/sounds/mute_while_media", tk("Quiet while media plays"), tk("No shell sounds over your music or video"), Toggle),
        s(Audio, SOUND_EVENTS, "/sounds/volume_change", tk("Volume changes"), tk("A soft tick as the volume moves"), Toggle),
        s(Audio, SOUND_EVENTS, "/sounds/notification_low", tk("Low-urgency notifications"), tk("Usually silent"), Toggle),
        s(Audio, SOUND_EVENTS, "/sounds/notification", tk("Notifications"), tk("Apps can ask for their own sound"), Toggle),
        s(Audio, SOUND_EVENTS, "/sounds/notification_critical", tk("Critical notifications"), tk("A distinct alert sound"), Toggle),
        s(Audio, SOUND_EVENTS, "/sounds/charger", tk("Charger and battery"), tk("Plugging in, unplugging and low-battery alerts"), Toggle),
        s(Audio, SOUND_EVENTS, "/sounds/screenshot", tk("Screenshots"), tk("A shutter when a capture is taken"), Toggle),
        s(Audio, SOUND_EVENTS, "/sounds/login", tk("Logging in"), tk("Once per session"), Toggle),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::ShellConfig;

    #[test]
    fn alert_settings_round_trip_and_choices_are_valid() {
        let cfg = ShellConfig::default();
        for setting in entries() {
            let v = super::super::get(&cfg, &setting);
            let back = super::super::set(&cfg, &setting, v.clone()).unwrap_or_else(|e| panic!("{}: {e}", setting.path));
            assert_eq!(super::super::get(&back, &setting), v, "{}", setting.path);
            if let Control::Choice(choices) = &setting.control {
                for (raw, _) in choices.iter() {
                    assert!(super::super::set(&cfg, &setting, super::super::choice_value(raw)).is_ok(), "{} = {raw}", setting.path);
                }
            }
        }
    }
}
