//! Live renderer pause rules, independent of the wallpaper painting backend.

use crate::backdrop::video;
use gpui::{App, Global, Subscription, Task};
use mandara_config::desktop::WallpaperVideo;

struct Watch {
    _subscriptions: Vec<Subscription>,
    _locks: Task<()>,
}
impl Global for Watch {}

pub(crate) fn conditions(cx: &App) -> video::Conditions {
    video::Conditions {
        locked: crate::lock::is_locked(),
        screens_off: crate::powerctl::idle::manager(cx).is_some_and(|m| m.read(cx).screens_off()),
        on_battery: crate::powerctl::battery::on_battery(cx),
        ..Default::default()
    }
}

pub(crate) fn may_play(
    rules: &WallpaperVideo,
    output: &str,
    base: video::Conditions,
    cx: &App,
) -> bool {
    video::may_play(
        rules,
        &video::Conditions {
            covered: !crate::wallpaper::desktop_visible(output, cx),
            fullscreen: crate::wallpaper::fullscreen_on(output, cx),
            ..base
        },
    )
}

pub(crate) fn refresh(cx: &mut App) {
    let mut outputs = crate::plugins::external::live::shown()
        .into_iter()
        .map(|(output, _)| output)
        .collect::<Vec<_>>();
    if outputs.is_empty() {
        if cx.has_global::<Watch>() {
            cx.remove_global::<Watch>();
        }
        return;
    }
    if !cx.has_global::<Watch>() {
        watch(cx);
    }
    let mut rules = crate::state::config(cx)
        .read(cx)
        .get()
        .wallpaper
        .video
        .clone();
    rules.still_frame = false;
    let conditions = conditions(cx);
    outputs.retain(|output| may_play(&rules, output, conditions, cx));
    crate::plugins::external::live::set_playing(&outputs);
}

fn watch(cx: &mut App) {
    let mut subscriptions = Vec::new();
    subscriptions.push(cx.observe(&crate::state::config(cx), |_, cx| refresh(cx)));
    if let Some(hub) = cx
        .try_global::<crate::hub::GlobalHub>()
        .map(|hub| hub.0.clone())
    {
        subscriptions.push(cx.observe(&hub, |_, cx| refresh(cx)));
    }
    subscriptions.push(cx.observe_global::<crate::overlays::Outputs>(refresh));
    if let Some(idle) = crate::powerctl::idle::manager(cx) {
        subscriptions.push(cx.observe(&idle, |_, cx| refresh(cx)));
    }
    if let Some(battery) = crate::powerctl::battery::policy(cx) {
        subscriptions.push(cx.observe(&battery, |_, cx| refresh(cx)));
    }
    let locks = crate::lock::subscribe();
    let locks = cx.spawn(async move |cx| {
        while locks.recv().await.is_ok() {
            cx.update(refresh);
        }
    });
    cx.set_global(Watch {
        _subscriptions: subscriptions,
        _locks: locks,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refreshing_without_live_scenes_needs_no_watch_or_shell_state() {
        gpui_platform::application()
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(|cx| {
                refresh(cx);
                refresh(cx);
                assert!(!cx.has_global::<Watch>());
                cx.spawn(async |cx| cx.update(|cx| cx.quit())).detach();
            });
    }
}
