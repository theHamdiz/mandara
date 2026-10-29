//! Login screen → Login manager: which login screen the system boots into
//! (Mandara Login, another greetd greeter, or a display manager such as
//! SDDM / GDM), and a "Use" button per installed choice.
//!
//! * Detection is [`mandara_greeter::manager::detect`], read on a
//!   background thread (plain file reads under `/etc`).
//! * "Use" first runs `mandara greeter use <id> --dry-run` and shows that
//!   plan in a confirm card; "Switch" then runs
//!   `pkexec mandara greeter use <id> --apply` (argv only, no shell).
//!   With `MANDARA_GREETER_DRY_RUN=1` the switch runs `--dry-run` instead
//!   (sandboxes, screenshots): nothing is ever changed.
//! * Nothing is started or stopped: the switch shows at the next boot.

use std::ffi::OsString;
use std::path::Path;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_greeter::manager::{self, Availability, Detection, Kind, LoginManager, Missing, SysRoot};
use mandara_i18n::{t, tf};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;

use super::GreeterPage;
use crate::tokens::*;

/// The privilege helper the switch runs through.
const PKEXEC: &str = "pkexec";
/// Height of the plan box in the confirm card before it scrolls.
const PLAN_MAX_H_PX: f32 = 260.0;

/// The plan waiting for the user's "Switch".
#[derive(Clone)]
pub(super) struct Confirm {
    id: &'static str,
    /// Install what the choice is missing first (`greeter use --install`).
    install: bool,
    plan: String,
    /// The dry run passed validation (otherwise "Switch" is disabled).
    runnable: bool,
}

#[derive(Default)]
pub(super) struct ManagerState {
    detection: Option<Detection>,
    detecting: bool,
    confirm: Option<Confirm>,
    /// Id whose plan or switch is running.
    busy: Option<&'static str>,
}

impl ManagerState {
    /// The state while the first detection (see [`spawn_detection`]) runs.
    pub(super) fn detecting() -> ManagerState {
        ManagerState { detecting: true, ..ManagerState::default() }
    }

    /// A plan or a pkexec switch is running (keeps the page alive).
    pub(super) fn busy(&self) -> bool {
        self.busy.is_some()
    }
}

/// argv for `greeter use <id>`: the dry run directly, the real switch
/// through pkexec; `install` first installs what the choice is missing
/// (the login manager or greeter, greetd, its kiosk compositor).
pub(super) fn use_command(exe: &Path, id: &str, dry_run: bool, install: bool) -> (OsString, Vec<OsString>) {
    let mut tail: Vec<OsString> = ["greeter", "use", id, if dry_run { "--dry-run" } else { "--apply" }].map(OsString::from).to_vec();
    if install {
        tail.push("--install".into());
    }
    if dry_run { (exe.as_os_str().to_owned(), tail) } else { (PKEXEC.into(), std::iter::once(exe.as_os_str().to_owned()).chain(tail).collect()) }
}

/// Runs argv; Ok(stdout) on success, Err(stderr or stdout) otherwise.
fn run(prog: OsString, args: Vec<OsString>) -> Result<String, String> {
    match std::process::Command::new(&prog).args(&args).stdin(std::process::Stdio::null()).output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        Ok(o) => {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            Err([out, err].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n"))
        }
        Err(e) => Err(format!("{}: {e}", prog.to_string_lossy())),
    }
}

/// Detects the login manager on a background thread and stores it in the
/// page (usable while the page is being created).
pub(super) fn spawn_detection(cx: &mut Context<GreeterPage>) {
    let task = cx.background_executor().spawn(async move { manager::detect(&SysRoot::system()) });
    cx.spawn(async move |this, cx| {
        let d = task.await;
        let _ = this.update(cx, |this, cx| {
            this.manager.detection = Some(d);
            this.manager.detecting = false;
            cx.notify();
        });
    })
    .detach();
}

fn kind_label(m: &LoginManager) -> &'static str {
    match m.kind {
        Kind::GreetdGreeter => t("greetd greeter"),
        Kind::DisplayManager => t("Display manager"),
    }
}

fn missing_label(m: &Missing) -> String {
    tf("{name} isn't installed — Install & use adds it (package {package})", &[("name", &m.what), ("package", &m.package)])
}

impl GreeterPage {
    /// Reads which login manager is active (background thread).
    pub(super) fn detect_managers(&mut self, cx: &mut Context<Self>) {
        if self.manager.detecting {
            return;
        }
        self.manager.detecting = true;
        spawn_detection(cx);
        cx.notify();
    }

    /// "Use" / "Install & use": computes the plan (`--dry-run`) and asks
    /// for confirmation.
    fn plan_switch(&mut self, id: &'static str, install: bool, cx: &mut Context<Self>) {
        if self.manager.busy.is_some() {
            return;
        }
        self.manager.busy = Some(id);
        self.manager.confirm = None;
        self.message = Some((true, t("Preparing the plan…").to_string()));
        let exe = std::env::current_exe().unwrap_or_else(|_| "mandara".into());
        let (prog, args) = use_command(&exe, id, true, install);
        let task = cx.background_executor().spawn(async move { run(prog, args) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.manager.busy = None;
                this.message = None;
                let (plan, runnable) = match result {
                    Ok(plan) => (plan, true),
                    Err(plan) => (plan, false),
                };
                this.manager.confirm = Some(Confirm { id, install, plan, runnable });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// "Switch": runs the confirmed plan through pkexec (or, with
    /// `MANDARA_GREETER_DRY_RUN`, as a dry run).
    fn run_switch(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.manager.confirm.take() else { return };
        if self.manager.busy.is_some() || !confirm.runnable {
            return;
        }
        let dry = manager::forced_dry_run();
        self.manager.busy = Some(confirm.id);
        self.message = Some((true, if dry { t("Preparing the plan…") } else { t("Waiting for authorization…") }.to_string()));
        let exe = std::env::current_exe().unwrap_or_else(|_| "mandara".into());
        let (prog, args) = use_command(&exe, confirm.id, dry, confirm.install);
        let name = manager::find(confirm.id).map_or(confirm.id, |m| m.name);
        let task = cx.background_executor().spawn(async move { run(prog, args) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.manager.busy = None;
                this.message = Some(match result {
                    Ok(_) if dry => (true, t("Dry run: nothing was changed.").to_string()),
                    Ok(_) => (true, format!("{} {}", tf("Switched to {name}.", &[("name", &name)]), t("Takes effect the next time you log in or restart. Your current session isn't touched."))),
                    Err(e) => (false, e),
                });
                this.detect_managers(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The Login manager tab.
    pub(super) fn manager_group(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let Some(d) = self.manager.detection.clone() else {
            out.push(Self::card(p, t("Active now").to_string(), vec![Self::row(p, t("Detecting login managers…").to_string(), div().into_any_element())]));
            return out;
        };
        if let Some(c) = self.manager.confirm.clone() {
            out.push(self.confirm_card(p, &c, cx));
        }
        // Active now.
        let active = d.active.and_then(manager::find);
        let (name, sub, ic) = match (active, d.display_manager.as_deref()) {
            (Some(m), _) => (m.name.to_string(), kind_label(m).to_string(), m.icon),
            (None, Some("greetd")) => (t("Custom greetd greeter").to_string(), d.greetd.command.clone().unwrap_or_default(), "terminal"),
            (None, Some(dm)) => (dm.to_string(), t("Display manager").to_string(), "monitor"),
            (None, None) => (t("No login manager is enabled").to_string(), String::new(), "alert"),
        };
        let refresh = Self::button(p, "lm-refresh", t("Refresh").to_string(), "refresh", false).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.detect_managers(cx)));
        out.push(Self::card(p, t("Active now").to_string(), vec![self.entry_row(p, ic, name, sub, None, refresh.into_any_element())]));
        // Every choice.
        let mut rows = Vec::new();
        for o in &d.options {
            rows.push(self.option_row(p, &d, o, cx));
        }
        out.push(Self::card(p, t("Available login screens").to_string(), rows));
        out.push(
            div()
                .px(px(SPACE_6_PX))
                .text_size(px(TEXT_12_PX))
                .text_color(p.on_surface_variant.hsla())
                .child(mandara_ui::bidi::paragraph(t("Takes effect the next time you log in or restart. Your current session isn't touched.")))
                .into_any_element(),
        );
        out
    }

    /// Icon, name, subtitle (+ badge) and a control on the right.
    fn entry_row(&self, p: &MandaraPalette, ic: &'static str, name: String, sub: String, badge: Option<String>, control: AnyElement) -> AnyElement {
        div()
            .min_h(px(DIM_52_PX))
            .px(px(SPACE_14_PX))
            .py(px(SPACE_8_PX))
            .row()
            .items_center()
            .gap(px(SPACE_12_PX))
            .child(div().size(px(DIM_32_PX)).flex_none().rounded(px(RADIUS_10_PX)).bg(p.surface_container_high.hsla()).row().items_center().justify_center().child(icon(ic, ICON_15_PX, p.primary)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_3_PX))
                    .child(
                        div()
                            .row()
                            .items_center()
                            .gap(px(SPACE_8_PX))
                            .child(div().font_weight(FontWeight::MEDIUM).text_color(p.on_surface.hsla()).child(name))
                            .when_some(badge, |el, b| {
                                el.child(div().px(px(SPACE_7_PX)).py(px(SPACE_3_PX)).rounded(px(RADIUS_8_PX)).text_size(px(TEXT_11_PX)).bg(p.primary.with_alpha(ALPHA_16).hsla()).text_color(p.primary.hsla()).child(b))
                            }),
                    )
                    .when(!sub.is_empty(), |el| el.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(sub))),
            )
            .child(control)
            .into_any_element()
    }

    fn option_row(&self, p: &MandaraPalette, d: &Detection, o: &Availability, cx: &mut Context<Self>) -> AnyElement {
        let m = o.manager();
        let id = m.id;
        let is_active = d.active == Some(id);
        let badge = m.is_mandara().then(|| t("Recommended · default").to_string());
        let mut sub = kind_label(m).to_string();
        if !o.available() {
            sub = o.missing.iter().map(missing_label).collect::<Vec<_>>().join(" · ");
        }
        let busy = self.manager.busy == Some(id);
        let control = if is_active {
            div()
                .px(px(SPACE_10_PX))
                .py(px(SPACE_4_PX))
                .rounded(px(RADIUS_10_PX))
                .text_size(px(TEXT_12_PX))
                .bg(p.success_emerald.with_alpha(ALPHA_16).hsla())
                .row()
                .items_center()
                .gap(px(SPACE_5_PX))
                .child(icon("check", ICON_14_PX, p.success_emerald))
                .child(t("Active"))
                .into_any_element()
        } else if o.available() {
            Self::button(p, id, if busy { t("Preparing the plan…").to_string() } else { t("Use").to_string() }, "arrow", m.is_mandara())
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.plan_switch(id, false, cx)))
                .into_any_element()
        } else {
            // Missing pieces are installed with the system's package manager
            // as part of the switch (the plan lists them before it runs).
            Self::button(p, id, if busy { t("Preparing the plan…").to_string() } else { t("Install & use").to_string() }, "download", m.is_mandara())
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.plan_switch(id, true, cx)))
                .into_any_element()
        };
        self.entry_row(p, m.icon, m.name.to_string(), sub, badge, control)
    }

    fn confirm_card(&self, p: &MandaraPalette, c: &Confirm, cx: &mut Context<Self>) -> AnyElement {
        let name = manager::find(c.id).map_or(c.id, |m| m.name);
        let mono = mandara_ui::fonts::fonts(cx).mono.clone();
        let switch = Self::button(p, "lm-switch", t("Switch").to_string(), "check", true).when(!c.runnable, |el| el.opacity(ALPHA_40).cursor_default());
        let switch = if c.runnable { switch.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.run_switch(cx))) } else { switch };
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .p(px(SPACE_14_PX))
            .rounded(px(RADIUS_18_PX))
            .border_1()
            .border_color(p.primary.with_alpha(ALPHA_50).hsla())
            .bg(p.surface_container.hsla())
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface.hsla()).child(tf("Switch the login screen to {name}?", &[("name", &name)])))
            .child(
                div()
                    .id("lm-plan")
                    .max_h(px(PLAN_MAX_H_PX))
                    .overflow_y_scroll()
                    .p(px(SPACE_10_PX))
                    .rounded(px(RADIUS_12_PX))
                    .bg(p.surface_container_high.hsla())
                    .font_family(mono)
                    .text_size(px(TEXT_11_5_PX))
                    .text_color(p.on_surface.hsla())
                    .child(SharedString::from(c.plan.clone())),
            )
            .child(
                div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(if c.runnable {
                    t("Takes effect the next time you log in or restart. Your current session isn't touched.")
                } else {
                    t("Fix the errors in the plan first.")
                }),
            )
            .child(
                div()
                    .row()
                    .gap(px(SPACE_6_PX))
                    .child(switch)
                    .child(Self::button(p, "lm-cancel", t("Cancel").to_string(), "close", false).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.manager.confirm = None;
                        cx.notify();
                    }))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_goes_through_pkexec_and_dry_run_never_does() {
        let exe = Path::new("/opt/mandara/bin/mandara");
        let (prog, args) = use_command(exe, "sddm", false, false);
        assert_eq!(prog, PKEXEC);
        assert_eq!(args, ["/opt/mandara/bin/mandara", "greeter", "use", "sddm", "--apply"].map(OsString::from));
        let (prog, args) = use_command(exe, "mandara", true, false);
        assert_eq!(prog, exe.as_os_str());
        assert_eq!(args, ["greeter", "use", "mandara", "--dry-run"].map(OsString::from));
        // "Install & use" installs what's missing as part of the switch.
        let (prog, args) = use_command(exe, "regreet", false, true);
        assert_eq!(prog, PKEXEC);
        assert_eq!(args, ["/opt/mandara/bin/mandara", "greeter", "use", "regreet", "--apply", "--install"].map(OsString::from));
        let (_, args) = use_command(exe, "regreet", true, true);
        assert_eq!(args, ["greeter", "use", "regreet", "--dry-run", "--install"].map(OsString::from));
    }
}
