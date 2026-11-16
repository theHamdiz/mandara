//! One file scan in flight; a newer request supersedes its result.

use gpui::{App, Global};

#[derive(Default)]
pub struct Reload {
    running: bool,
    dirty: bool,
}

impl Global for Reload {}

impl Reload {
    pub fn is_running(&self) -> bool {
        self.running
    }
    fn request(&mut self) -> bool {
        self.dirty = true;
        if self.running {
            return false;
        }
        self.running = true;
        true
    }

    fn finish(&mut self) -> bool {
        if self.dirty {
            return false;
        }
        self.running = false;
        true
    }
}

pub(super) fn request(cx: &mut App) {
    if !cx.has_global::<Reload>() {
        cx.set_global(Reload::default());
    }
    if !cx.global_mut::<Reload>().request() {
        return;
    }
    cx.spawn(async move |cx| {
        loop {
            cx.update(|cx| cx.global_mut::<Reload>().dirty = false);
            let found = mandara_services::runtime::unblock(super::scan).await;
            let done = cx.update(|cx| {
                if !cx.global_mut::<Reload>().finish() {
                    return false;
                }
                super::apply_scan(found, cx);
                true
            });
            if done {
                break;
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bursts_run_one_scan_and_reject_superseded_results() {
        let mut reload = Reload::default();
        assert!(reload.request());
        reload.dirty = false;
        for _ in 0..100 {
            assert!(!reload.request());
        }
        assert!(!reload.finish(), "the older scan must not publish");
        assert!(reload.running);
        reload.dirty = false;
        assert!(reload.finish());
        assert!(!reload.running);
        assert!(reload.request(), "an idle refresh starts a fresh worker");
    }
}
