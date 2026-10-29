//! General → Keyrings & passwords: what unlocks when you log in.
//!
//! * GNOME Keyring / KWallet: whether greetd's PAM stack unlocks them at a
//!   password login ([`mandara_greeter::keyring_pam`], plain file reads on a
//!   background thread). "Set up" first runs
//!   `mandara greeter keyring-unlock --dry-run` and shows that plan; "Apply"
//!   then runs `pkexec mandara greeter keyring-unlock --apply` (argv only, no
//!   shell; `--remove` to undo). `MANDARA_GREETER_DRY_RUN=1` makes "Apply"
//!   a dry run: nothing is ever changed.
//! * KeePassXC: installed / running / locked, and "Unlock now", which asks
//!   the shell to open its unlock dialog (`keepassxc unlock` over IPC).
//!
//! The prompt toggles themselves are ordinary registry rows right above.

use std::ffi::OsString;
use std::path::Path;

use gpui::{AnyElement, AppContext as _, ClickEvent, Context, Entity, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_greeter::keyring_pam::{Action, Hook, Keyring, KeyringPlan, ModuleStatus};
use mandara_greeter::manager::{self, SysRoot};
use mandara_i18n::{t, tf};
use mandara_services::keepassxc::{self, Status};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;

use super::StudioView;
use crate::tokens::*;

/// The privilege helper the change runs through.
const PKEXEC: &str = "pkexec";
/// Height of the plan box before it scrolls.
const PLAN_MAX_H_PX: f32 = 260.0;

/// argv for `greeter keyring-unlock`: the dry run directly, the change
/// through pkexec.
pub(super) fn keyring_command(exe: &Path, action: Action, dry_run: bool) -> (OsString, Vec<OsString>) {
    let mut tail: Vec<OsString> = ["greeter", "keyring-unlock"].map(OsString::from).to_vec();
    if action == Action::Remove {
        tail.push("--remove".into());
    }
    tail.push(if dry_run { "--dry-run" } else if action == Action::Add { "--apply" } else { "" }.into());
    tail.retain(|a| !a.is_empty());
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

/// The plan waiting for "Apply".
#[derive(Clone)]
struct Confirm {
    action: Action,
    plan: String,
    runnable: bool,
}

pub(super) struct KeyringsCard {
    plan: Option<KeyringPlan>,
    keepass: Option<Status>,
    keepass_installed: bool,
    detecting: bool,
    confirm: Option<Confirm>,
    busy: bool,
    message: Option<(bool, String)>,
}

struct CardHandle(Entity<KeyringsCard>);

impl Global for CardHandle {}

/// The card for the General page (one entity per Studio process).
pub(super) fn element(cx: &mut Context<StudioView>) -> AnyElement {
    let card = match cx.try_global::<CardHandle>() {
        Some(h) => h.0.clone(),
        None => {
            let card = cx.new(KeyringsCard::new);
            cx.set_global(CardHandle(card.clone()));
            card
        }
    };
    card.into_any_element()
}

fn module_line(m: &ModuleStatus) -> String {
    match (&m.module, m.unlocks_at_login()) {
        (None, _) => tf("Not unlocked at login: its PAM module isn't installed (package {package})", &[("package", &m.package)]),
        (Some(_), true) if m.by_mandara() => t("Unlocks when you log in with Mandara Login").to_string(),
        (Some(_), true) => match (&m.auth, &m.session) {
            (Hook::Elsewhere(f), _) | (_, Hook::Elsewhere(f)) => tf("Unlocks when you log in (set up in {file})", &[("file", f)]),
            _ => t("Unlocks when you log in").to_string(),
        },
        (Some(_), false) => t("Installed, but not unlocked when you log in").to_string(),
    }
}

impl KeyringsCard {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut card = Self { plan: None, keepass: None, keepass_installed: false, detecting: false, confirm: None, busy: false, message: None };
        card.detect(cx);
        card
    }

    /// Reads the PAM state and KeePassXC's status (background).
    fn detect(&mut self, cx: &mut Context<Self>) {
        if self.detecting {
            return;
        }
        self.detecting = true;
        let task = cx.background_executor().spawn(async move { (KeyringPlan::build(&SysRoot::system(), Action::Add), keepassxc::installed()) });
        let status = keepassxc::spawn_status();
        cx.spawn(async move |this, cx| {
            let (plan, installed) = task.await;
            let keepass = status.recv().await.ok().and_then(Result::ok);
            let _ = this.update(cx, |this, cx| {
                this.plan = Some(plan);
                this.keepass_installed = installed;
                this.keepass = keepass;
                this.detecting = false;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// "Set up" / "Remove": the dry-run plan first.
    fn plan_change(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.confirm = None;
        self.message = Some((true, t("Preparing the plan…").to_string()));
        let exe = std::env::current_exe().unwrap_or_else(|_| "mandara".into());
        let (prog, args) = keyring_command(&exe, action, true);
        let task = cx.background_executor().spawn(async move { run(prog, args) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.message = None;
                let (plan, runnable) = match result {
                    Ok(plan) => (plan, true),
                    Err(plan) => (plan, false),
                };
                this.confirm = Some(Confirm { action, plan, runnable });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// "Apply": the confirmed plan through pkexec (or a dry run).
    fn apply(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm.take() else { return };
        if self.busy || !confirm.runnable {
            return;
        }
        let dry = manager::forced_dry_run();
        self.busy = true;
        self.message = Some((true, if dry { t("Preparing the plan…") } else { t("Waiting for authorization…") }.to_string()));
        let exe = std::env::current_exe().unwrap_or_else(|_| "mandara".into());
        let (prog, args) = keyring_command(&exe, confirm.action, dry);
        let task = cx.background_executor().spawn(async move { run(prog, args) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.message = Some(match result {
                    Ok(_) if dry => (true, t("Dry run: nothing was changed.").to_string()),
                    Ok(_) if confirm.action == Action::Add => (true, t("Done. Your keyrings unlock the next time you log in with your password.").to_string()),
                    Ok(_) => (true, t("Done. Keyrings are no longer unlocked at login.").to_string()),
                    Err(e) => (false, e),
                });
                this.detect(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Asks the shell for its KeePassXC dialog.
    fn unlock_keepassxc(&mut self, cx: &mut Context<Self>) {
        let Some(ipc) = cx.try_global::<crate::perf::ShellIpc>().cloned() else {
            self.message = Some((false, t("Mandara isn't running, so its dialog can't open.").to_string()));
            return cx.notify();
        };
        std::thread::spawn(move || {
            if let Err(err) = (ipc.0)(vec!["keepassxc".into(), "unlock".into()]) {
                tracing::warn!("keepassxc unlock: {err}");
            }
        });
    }

    fn button(p: &MandaraPalette, id: &'static str, label: String, ic: &'static str, primary: bool) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .h(px(DIM_32_PX))
            .px(px(SPACE_12_PX))
            .flex_none()
            .row()
            .items_center()
            .gap(px(SPACE_7_PX))
            .rounded(px(RADIUS_11_PX))
            .cursor_pointer()
            .text_size(px(TEXT_12_5_PX))
            .font_weight(FontWeight::MEDIUM)
            .when(primary, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()).hover(|s| s.opacity(ALPHA_90)))
            .when(!primary, |el| el.bg(p.surface_container_high.hsla()).hover(|s| s.bg(p.surface_container.hsla())))
            .child(icon(ic, ICON_15_PX, if primary { p.on_primary } else { p.on_surface }))
            .child(label)
    }

    fn entry(p: &MandaraPalette, ic: &'static str, name: String, sub: String, ok: bool, control: Option<AnyElement>) -> AnyElement {
        div()
            .min_h(px(DIM_52_PX))
            .px(px(SPACE_14_PX))
            .py(px(SPACE_8_PX))
            .row()
            .items_center()
            .gap(px(SPACE_12_PX))
            .child(div().size(px(DIM_32_PX)).flex_none().rounded(px(RADIUS_10_PX)).bg(p.surface_container_high.hsla()).row().items_center().justify_center().child(icon(ic, ICON_15_PX, if ok { p.success_emerald } else { p.primary })))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_3_PX))
                    .child(div().font_weight(FontWeight::MEDIUM).text_color(p.on_surface.hsla()).child(name))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(sub)),
            )
            .children(control)
            .into_any_element()
    }

    fn confirm_card(&self, p: &MandaraPalette, c: &Confirm, cx: &mut Context<Self>) -> AnyElement {
        let mono = mandara_ui::fonts::fonts(cx).mono.clone();
        let apply = Self::button(p, "kr-apply", t("Apply").to_string(), "check", true).when(!c.runnable, |el| el.opacity(ALPHA_40).cursor_default());
        let apply = if c.runnable { apply.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.apply(cx))) } else { apply };
        let title = match c.action {
            Action::Add => t("Unlock your keyrings when you log in?"),
            Action::Remove => t("Stop unlocking keyrings at login?"),
        };
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .p(px(SPACE_14_PX))
            .rounded(px(RADIUS_18_PX))
            .border_1()
            .border_color(p.primary.with_alpha(ALPHA_50).hsla())
            .bg(p.surface_container.hsla())
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface.hsla()).child(title))
            .child(
                div()
                    .id("kr-plan")
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
            .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(if c.runnable {
                t("Mandara asks for your administrator password, backs up greetd's PAM file and changes only its own marked lines.")
            } else {
                t("Fix the errors in the plan first.")
            }))
            .child(
                div().row().gap(px(SPACE_6_PX)).child(apply).child(Self::button(p, "kr-cancel", t("Cancel").to_string(), "close", false).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.confirm = None;
                    cx.notify();
                }))),
            )
            .into_any_element()
    }
}

impl Render for KeyringsCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = mandara_core::frame_palette(window, cx);
        let mut rows: Vec<AnyElement> = Vec::new();
        match &self.plan {
            None => rows.push(Self::entry(&p, "key", t("Unlock at login").to_string(), t("Checking…").to_string(), false, None)),
            Some(plan) => {
                for m in &plan.modules {
                    let ic = match m.keyring {
                        Keyring::Gnome => "key",
                        Keyring::KWallet => "shield",
                    };
                    rows.push(Self::entry(&p, ic, m.keyring.name().to_string(), module_line(m), m.unlocks_at_login(), None));
                }
                // One button for both: set up, or take Mandara's lines out.
                let control = if !plan.errors.is_empty() {
                    None
                } else if plan.changes() && !plan.nothing_installed() {
                    Some(Self::button(&p, "kr-setup", t("Set up").to_string(), "arrow", true).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.plan_change(Action::Add, cx))).into_any_element())
                } else if plan.modules.iter().any(ModuleStatus::by_mandara) {
                    Some(Self::button(&p, "kr-remove", t("Remove").to_string(), "undo", false).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.plan_change(Action::Remove, cx))).into_any_element())
                } else {
                    None
                };
                let sub = if !plan.errors.is_empty() {
                    t("Needs Mandara Login (greetd): its PAM file wasn't found.").to_string()
                } else if plan.nothing_installed() {
                    t("Neither GNOME Keyring's nor KWallet's PAM module is installed.").to_string()
                } else if plan.autologin {
                    t("Autologin is on: that login has no password, so keyrings stay locked.").to_string()
                } else {
                    t("With your password through Mandara Login (not autologin). Each keyring's password must be your login password.").to_string()
                };
                rows.push(Self::entry(&p, "lock-open", t("Unlock keyrings when you log in").to_string(), sub, false, control));
            }
        }
        if self.keepass_installed {
            let sub = match &self.keepass {
                Some(s) if s.running => match s.any_locked() {
                    Some(true) => t("Running · locked").to_string(),
                    Some(false) => t("Running · unlocked").to_string(),
                    None => t("Running").to_string(),
                },
                _ => t("Not running").to_string(),
            };
            let unlock = Self::button(&p, "kr-keepassxc", t("Unlock now").to_string(), "key", false).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.unlock_keepassxc(cx)));
            rows.push(Self::entry(&p, "key", t("KeePassXC").to_string(), sub, self.keepass.as_ref().is_some_and(|s| s.any_locked() == Some(false)), Some(unlock.into_any_element())));
        }
        let refresh = Self::button(&p, "kr-refresh", t("Refresh").to_string(), "refresh", false).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.detect(cx)));
        let confirm = self.confirm.clone().map(|c| self.confirm_card(&p, &c, cx));
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_6_PX))
            .child(
                div()
                    .px(px(SPACE_6_PX))
                    .row()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(t("Unlock at login")))
                    .child(refresh),
            )
            .children(confirm)
            .child(div().row().flex_col().py(px(SPACE_4_PX)).rounded(px(RADIUS_18_PX)).bg(p.surface_container.hsla()).children(rows))
            .when_some(self.message.clone(), |el, (ok, text)| el.child(div().px(px(SPACE_6_PX)).text_size(px(TEXT_12_PX)).text_color(if ok { p.on_surface_variant.hsla() } else { p.urgent_coral.hsla() }).child(text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_goes_through_pkexec_and_dry_runs_never_do() {
        let exe = Path::new("/opt/mandara/bin/mandara");
        let (prog, args) = keyring_command(exe, Action::Add, false);
        assert_eq!(prog, PKEXEC);
        assert_eq!(args, ["/opt/mandara/bin/mandara", "greeter", "keyring-unlock", "--apply"].map(OsString::from));
        let (prog, args) = keyring_command(exe, Action::Remove, false);
        assert_eq!(prog, PKEXEC);
        assert_eq!(args, ["/opt/mandara/bin/mandara", "greeter", "keyring-unlock", "--remove"].map(OsString::from));
        let (prog, args) = keyring_command(exe, Action::Remove, true);
        assert_eq!(prog, exe.as_os_str());
        assert_eq!(args, ["greeter", "keyring-unlock", "--remove", "--dry-run"].map(OsString::from));
    }
}
