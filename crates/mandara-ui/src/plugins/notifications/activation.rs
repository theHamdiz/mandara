//! Opens a notification's application without inventing sender actions.

use gpui::App;
use mandara_services::{Notification, NotificationCommand};
use mandara_services::toplevels::Toplevel;

use crate::apps::App as DesktopApp;

/// Click handlers keep identity, not a second copy of notification bodies.
#[derive(Clone)]
pub(super) struct Source {
    id: u32,
    app_name: String,
    desktop_entry: String,
    default_action: bool,
}

impl From<&Notification> for Source {
    fn from(notification: &Notification) -> Self {
        Self { id: notification.id, app_name: notification.app_name.clone(), desktop_entry: notification.desktop_entry.clone(), default_action: has_default(notification) }
    }
}

#[derive(Debug, PartialEq)]
enum Destination<'a> {
    Window(u64),
    Launch(&'a DesktopApp),
}

/// The sender owns its default action, including any conversation or document.
fn has_default(notification: &Notification) -> bool {
    notification.actions.iter().any(|(key, _)| key == "default")
}

/// Notifications without actions can still identify the app they came from.
pub(super) fn available(notification: &Notification) -> bool {
    has_default(notification) || !notification.desktop_entry.trim().is_empty() || !notification.app_name.trim().is_empty()
}

/// The notification hint names a desktop-file id, optionally with its suffix.
fn desktop_id(notification: &Source) -> &str {
    let id = notification.desktop_entry.trim();
    id.strip_suffix(".desktop").unwrap_or(id)
}

/// Resolve an installed entry only from an exact id or an unambiguous app name.
/// A supplied desktop id takes precedence over the display name.
fn desktop_app<'a>(notification: &Source, apps: &'a [DesktopApp]) -> Option<&'a DesktopApp> {
    let id = desktop_id(notification);
    if !id.is_empty() {
        return apps.iter().find(|app| app.id.eq_ignore_ascii_case(id));
    }
    let name = notification.app_name.trim();
    if name.is_empty() { return None; }
    let mut matches = apps.iter().filter(|app| app.id.eq_ignore_ascii_case(name) || app.name.eq_ignore_ascii_case(name));
    let found = matches.next()?;
    matches.next().is_none().then_some(found)
}

/// Prefer an existing MRU window. An advertised default action prevents a
/// second launch; the sender is responsible for opening its own target.
fn resolve<'a>(notification: &Source, apps: &'a [DesktopApp], windows: &[Toplevel]) -> Option<Destination<'a>> {
    let app = desktop_app(notification, apps);
    let identity = if desktop_id(notification).is_empty() { notification.app_name.trim() } else { desktop_id(notification) };
    let exact = |window: &&Toplevel| match app {
        Some(app) => app.id.eq_ignore_ascii_case(&window.app_id) || app.wm_class.as_deref().is_some_and(|class| class.eq_ignore_ascii_case(&window.app_id)),
        None => !identity.is_empty() && window.app_id.eq_ignore_ascii_case(identity),
    };
    if let Some(window) = windows.iter().find(exact) {
        return Some(Destination::Window(window.id));
    }
    let matching = |window: &&Toplevel| match app {
        Some(app) => crate::windows::same_app(&window.app_id, &app.id),
        None => !identity.is_empty() && crate::windows::same_app(&window.app_id, identity),
    };
    let mut matches = windows.iter().filter(matching);
    if let Some(first) = matches.next() {
        // A short id can match unrelated reverse-DNS ids. Keep aliases only
        // when every matching window belongs to the same application.
        return matches.all(|other| crate::windows::same_app(&first.app_id, &other.app_id)).then_some(Destination::Window(first.id));
    }
    if notification.default_action { None } else { app.map(Destination::Launch) }
}

/// Release the notification center before asking another app to take focus.
fn close_center(cx: &mut App) {
    crate::state::shell(cx).update(cx, |shell, cx| {
        if shell.panel.as_deref() == Some(super::ID) { shell.close_panel_now(cx); }
        shell.set_sidebar(false, cx);
    });
    crate::floating::close(super::ID, cx);
}

/// Apply a resolved target on the UI thread; desktop execution stays on a worker.
fn show(notification: &Source, destination: Destination<'_>, cx: &mut App) {
    close_center(cx);
    if !notification.default_action { super::send(cx, NotificationCommand::Dismiss(notification.id)); }
    match destination {
        Destination::Window(id) => cx.defer(move |cx| crate::windows::activate(id, cx)),
        Destination::Launch(app) => {
            let app = app.clone();
            let options = crate::state::config(cx).read(cx).snapshot().launch_options();
            cx.background_executor().spawn(async move {
                if let Err(error) = crate::apps::launch_with(&app, None, &options) {
                    tracing::warn!("notification application: {error}");
                }
            }).detach();
        }
    }
}

/// Preserve sender actions and raise a known window, or resolve a safe desktop
/// fallback off the UI thread when the sender did not supply a default action.
pub(super) fn open(notification: Source, cx: &mut App) {
    if notification.default_action {
        close_center(cx);
        super::send(cx, NotificationCommand::Invoke { id: notification.id, action: "default".into() });
    }
    let windows = crate::windows::windows(cx);
    if let Some(Destination::Window(id)) = windows.as_ref().and_then(|windows| resolve(&notification, &[], &windows.read(cx).state.toplevels)) {
        show(&notification, Destination::Window(id), cx);
        return;
    }
    let entries = cx.background_executor().spawn(async { crate::apps::discover() });
    cx.spawn(async move |cx| {
        let entries = entries.await;
        cx.update(|cx| {
            let windows = crate::windows::windows(cx);
            let destination = {
                let list = windows.as_ref().map(|windows| &windows.read(cx).state.toplevels[..]).unwrap_or_default();
                resolve(&notification, &entries, list)
            };
            if let Some(destination) = destination { show(&notification, destination, cx); }
        });
    }).detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(id: &str) -> Source {
        Source::from(&Notification { desktop_entry: id.into(), app_name: "Mail".into(), ..Default::default() })
    }

    fn app(id: &str, name: &str) -> DesktopApp {
        DesktopApp { id: id.into(), name: name.into(), exec: "/usr/bin/mail".into(), ..Default::default() }
    }

    fn window(id: u64, app_id: &str) -> Toplevel {
        Toplevel { id, app_id: app_id.into(), ..Default::default() }
    }

    #[test]
    fn source_only_records_an_advertised_default_action() {
        let mut note = Notification { actions: vec![("reply".into(), "Reply".into())], ..Default::default() };
        assert!(!Source::from(&note).default_action);
        note.actions.push(("default".into(), "Open conversation".into()));
        assert!(Source::from(&note).default_action);
    }

    #[test]
    fn desktop_hint_focuses_the_mru_window_even_without_an_action() {
        let n = notification("org.example.Mail.desktop");
        let windows = [window(8, "org.example.Mail"), window(7, "org.example.Mail"), window(2, "other")];
        assert_eq!(resolve(&n, &[], &windows), Some(Destination::Window(8)));
        assert!(available(&Notification { desktop_entry: "org.example.Mail".into(), ..Default::default() }));
    }

    #[test]
    fn startup_class_matches_and_a_default_handler_never_relaunches() {
        let mut n = notification("org.example.Mail");
        let mut app = app("org.example.Mail", "Mail");
        app.wm_class = Some("mail-client".into());
        let apps = [app];
        assert_eq!(resolve(&n, &apps, &[window(7, "mail-client")]), Some(Destination::Window(7)));
        assert_eq!(resolve(&n, &apps, &[]), Some(Destination::Launch(&apps[0])));
        n.default_action = true;
        assert_eq!(resolve(&n, &apps, &[]), None);
        assert_eq!(resolve(&n, &apps, &[window(7, "mail-client")]), Some(Destination::Window(7)));
    }

    #[test]
    fn a_unique_display_name_can_open_an_app_but_ambiguous_names_cannot() {
        let n = notification("");
        let apps = [app("org.example.Mail", "Mail")];
        assert_eq!(resolve(&n, &apps, &[]), Some(Destination::Launch(&apps[0])));
        let apps = [app("org.example.Mail", "Mail"), app("org.other.Post", "Mail")];
        assert_eq!(resolve(&n, &apps, &[]), None);
        assert_eq!(resolve(&n, &[], &[window(1, "org.example.Mail"), window(2, "org.other.Mail")]), None);
    }

    #[test]
    fn an_installed_identity_takes_precedence_over_a_shared_display_name() {
        let n = notification("");
        let apps = [app("org.example.Mail", "Mail")];
        let windows = [window(8, "com.other.Mail"), window(7, "org.example.Mail")];
        assert_eq!(resolve(&n, &apps, &windows), Some(Destination::Window(7)));
        assert_eq!(resolve(&n, &apps, &windows[..1]), Some(Destination::Launch(&apps[0])));
    }

    #[test]
    fn an_exact_window_id_wins_and_ambiguous_short_aliases_stay_untouched() {
        let n = notification("Mail");
        let apps = [app("Mail", "Mail")];
        let windows = [window(8, "com.other.Mail"), window(7, "org.example.Mail"), window(6, "Mail")];
        assert_eq!(resolve(&n, &apps, &windows), Some(Destination::Window(6)));
        assert_eq!(resolve(&n, &apps, &windows[..2]), None);
    }

    #[test]
    fn a_supplied_desktop_hint_does_not_fall_back_to_an_unrelated_display_name() {
        let n = notification("unknown.desktop");
        let apps = [app("org.example.Mail", "Mail")];
        assert_eq!(resolve(&n, &apps, &[window(1, "org.example.Mail")]), None);
        assert_eq!(resolve(&notification("../../bad.desktop"), &apps, &[]), None);
        assert!(!available(&Notification::default()));
        assert_eq!(resolve(&Source::from(&Notification::default()), &apps, &[window(2, "")]), None);
    }
}
