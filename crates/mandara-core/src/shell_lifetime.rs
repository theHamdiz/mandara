//! Distinguishes an explicit shell quit from a stopped display event loop.

use std::{cell::Cell, rc::Rc};

/// Shared only by the shell's foreground thread and its application runner.
#[derive(Clone, Default)]
pub struct ShellLifetime(Rc<Cell<bool>>);

impl gpui::Global for ShellLifetime {}

impl ShellLifetime {
    /// An unexpected event-loop return is a failure so the supervisor restarts it.
    pub fn finished(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.0.get(),
            "the shell's display event loop stopped without a quit request"
        );
        Ok(())
    }

    /// Marks a deliberate quit before stopping GPUI's event loop.
    pub fn quit(cx: &mut gpui::App) {
        if let Some(lifetime) = cx.try_global::<Self>() {
            lifetime.0.set(true);
        }
        cx.quit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stopped_event_loop_is_not_an_explicit_quit() {
        let lifetime = ShellLifetime::default();
        let running = lifetime.clone();
        gpui_platform::application()
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_global(running);
                cx.spawn(async |cx| cx.update(|cx| cx.quit())).detach();
            });
        assert!(lifetime.finished().is_err());
    }

    #[test]
    fn an_explicit_quit_allows_the_supervisor_to_stop() {
        let lifetime = ShellLifetime::default();
        let running = lifetime.clone();
        gpui_platform::application()
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_global(running);
                cx.spawn(async |cx| cx.update(ShellLifetime::quit)).detach();
            });
        assert!(lifetime.finished().is_ok());
    }
}
