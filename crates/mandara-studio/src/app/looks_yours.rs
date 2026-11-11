//! Studio → Themes → Looks: your own Looks ([`mandara_config::user_looks`]).
//!
//! * Their cards sit after the built-in ones with the same mock, a
//!   **Yours** badge and the Look they come from; each can be applied,
//!   renamed, duplicated, exported and deleted. Built-in cards export too.
//! * **Save as new Look** keeps what you changed on the active Look under a
//!   name; **Import a Look…** picks a file through the desktop portal.
//! * Customizing an applied Look makes its child: Studio notices the change
//!   ([`watch`]: one check, [`CHECK_DEBOUNCE`] after the last edit, off the
//!   UI thread, the stale result of an older check dropped), saves
//!   "‹Look› · Custom" once per Look per Studio session and keeps updating
//!   that one file as you go on. A built-in Look is never written. Studio
//!   does this while it runs, and on start for changes made meanwhile (the
//!   desktop edit mode, `mandara msg`).
//!
//! The list is read once, in the background, when the Looks gallery first
//! shows ([`looks`]); writes update it in place.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, ClickEvent, Context, Global, InteractiveElement as _, IntoElement, ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Task, div, prelude::FluentBuilder as _, px};
use mandara_config::looks::{self, LookOptions};
use mandara_config::user_looks::{self as ul, Customized, UserLook};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::compositor_page::{badge, chip, tip};
use super::{StudioView, form};
use crate::tokens::*;

/// Quiet time after your last change before Studio checks for one.
pub const CHECK_DEBOUNCE: Duration = Duration::from_millis(1500);
/// How long a delete waits for its confirming second click.
const ARM_WINDOW: Duration = Duration::from_secs(4);
/// The rename field's key.
const RENAME_FIELD: &str = "look-rename";

#[derive(Default)]
pub(crate) struct Yours {
    /// Your Looks, by name (`None` until first read).
    looks: Option<Rc<[UserLook]>>,
    loading: bool,
    /// Bumped by every write, so an older read can't land over it.
    generation: u64,
    pub(super) note: Option<(String, bool)>,
    armed: Option<(String, Instant)>,
    pub(super) renaming: Option<String>,
    /// This session's child of each Look you applied (applied id → child
    /// id), and the names they were given (an untouched name may go again
    /// when the child turns back into its parent).
    session: HashMap<String, String>,
    auto_names: HashMap<String, String>,
    /// The pending check (replaced, so cancelled, by every change).
    pending: Option<Task<()>>,
    check: u64,
    watching: bool,
}

impl Global for Yours {}

fn st(cx: &mut App) -> &mut Yours {
    cx.default_global::<Yours>()
}

/// Your Looks, reading them in the background the first time.
pub(super) fn looks(cx: &mut App) -> Option<Rc<[UserLook]>> {
    let s = st(cx);
    if s.looks.is_none() && !s.loading {
        reload(cx);
    }
    st(cx).looks.clone()
}

fn reload(cx: &mut App) {
    let s = st(cx);
    s.loading = true;
    let generation = s.generation;
    let task = cx.background_executor().spawn(async move { ul::list() });
    cx.spawn(async move |cx| {
        let list = task.await;
        cx.update(|cx| {
            let s = st(cx);
            s.loading = false;
            if s.generation == generation {
                s.looks = Some(list.into());
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Puts `look` into the cached list (replacing the one with its id).
fn remember(look: UserLook, cx: &mut App) {
    let s = st(cx);
    s.generation += 1;
    let mut list: Vec<UserLook> = s.looks.as_deref().map(<[UserLook]>::to_vec).unwrap_or_default();
    list.retain(|l| l.id != look.id);
    list.push(look);
    list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    s.looks = Some(list.into());
    cx.refresh_windows();
}

fn forget(id: &str, cx: &mut App) {
    let s = st(cx);
    s.generation += 1;
    if let Some(list) = &s.looks {
        s.looks = Some(list.iter().filter(|l| l.id != id).cloned().collect::<Vec<_>>().into());
    }
    s.session.retain(|_, child| child != id);
    cx.refresh_windows();
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

// ── Noticing customizations ─────────────────────────────────────────────

/// Starts following the config for customizations (once per Studio).
pub fn watch(cx: &mut App) {
    if std::mem::replace(&mut st(cx).watching, true) {
        return;
    }
    let config = mandara_ui::state::config(cx);
    cx.observe(&config, |_, cx| schedule(cx)).detach();
    // Changes made while Studio was closed.
    schedule(cx);
}

fn schedule(cx: &mut App) {
    if mandara_ui::state::config(cx).read(cx).get().looks.active.is_empty() {
        st(cx).pending = None;
        return;
    }
    let s = st(cx);
    s.check += 1;
    let check = s.check;
    let timer = cx.background_executor().timer(CHECK_DEBOUNCE);
    let task = cx.spawn(async move |cx| {
        timer.await;
        cx.update(|cx| run_check(check, cx));
    });
    st(cx).pending = Some(task);
}

/// Compares the config with its Look off the UI thread, then saves or
/// updates the child.
fn run_check(check: u64, cx: &mut App) {
    let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
    let active = cfg.looks.active.clone();
    if active.is_empty() {
        return;
    }
    let cached = st(cx).looks.as_deref().and_then(|l| l.iter().find(|x| x.id == active).cloned());
    let builtin = looks::find(&active).is_some();
    let work = cx.background_executor().spawn({
        let cfg = cfg.clone();
        async move {
            let user = if builtin { None } else { cached.or_else(|| ul::find(&cfg.looks.active)) };
            let outcome = customized_of(&cfg, user.as_ref());
            (user, outcome, ul::owned_print(&cfg))
        }
    });
    cx.spawn(async move |cx| {
        let (user, outcome, print) = work.await;
        cx.update(|cx| {
            if st(cx).check != check {
                return; // a newer check is on its way
            }
            settle(&cfg, user, outcome, print, cx);
        });
    })
    .detach();
}

fn customized_of(cfg: &ShellConfig, user: Option<&UserLook>) -> Customized {
    // A Look of yours that's gone (deleted elsewhere): nothing to compare.
    if user.is_none() && looks::find(&cfg.looks.active).is_none() {
        return Customized::No;
    }
    ul::customized(cfg, user)
}

fn settle(cfg: &ShellConfig, user: Option<UserLook>, outcome: Customized, print: String, cx: &mut App) {
    let active = cfg.looks.active.clone();
    match outcome {
        Customized::No => {}
        Customized::Restamp(print) => update_config(cx, move |c| {
            if c.looks.active == active {
                c.looks.print = print;
            }
        }),
        Customized::Changed { parent, delta } => {
            let s = st(cx);
            // This session's child of what you applied: the active Look
            // itself, or the one made from it earlier.
            let child_id = if s.session.values().any(|c| *c == active) { Some(active.clone()) } else { s.session.get(&active).cloned() };
            let existing = child_id.as_ref().and_then(|id| s.looks.as_deref().and_then(|l| l.iter().find(|x| x.id == *id).cloned()).or_else(|| (Some(id) == Some(&active)).then(|| user.clone()).flatten()));
            let names: Vec<String> = s.looks.as_deref().map(|l| l.iter().map(|x| x.name.clone()).collect()).unwrap_or_default();
            let ids: Vec<String> = s.looks.as_deref().map(|l| l.iter().map(|x| x.id.clone()).collect()).unwrap_or_default();
            let wallpaper_colors = cfg.looks.wallpaper_colors;
            let (look, reverted) = match existing {
                Some(mut look) => {
                    let reverted = delta.is_empty() && st(cx).auto_names.get(&look.id) == Some(&look.name);
                    look.set_delta(delta);
                    (look, reverted)
                }
                None if delta.is_empty() => return,
                None => {
                    let mut look = UserLook::child(parent, delta, &names, &today());
                    if let Some(base) = &user {
                        // A child of one of yours is named after it.
                        let mut name = format!("{} · Custom", base.name);
                        let mut n = 2;
                        while names.iter().any(|x| x.eq_ignore_ascii_case(&name)) {
                            name = format!("{} · Custom {n}", base.name);
                            n += 1;
                        }
                        look.name = name.chars().take(ul::MAX_NAME_CHARS).collect();
                    }
                    look.wallpaper_colors = wallpaper_colors;
                    look.id = ul::unique_id(&look.name, &ids);
                    let s = st(cx);
                    s.session.insert(active.clone(), look.id.clone());
                    s.auto_names.insert(look.id.clone(), look.name.clone());
                    (look, false)
                }
            };
            let write = cx.background_executor().spawn({
                let look = look.clone();
                async move { if reverted { ul::delete(&look.id) } else { ul::save(&look).map(|_| ()) } }
            });
            let parent_id = parent.id.to_string();
            cx.spawn(async move |cx| {
                let saved = write.await;
                cx.update(|cx| match saved {
                    Ok(()) if reverted => {
                        forget(&look.id, cx);
                        // Back to the Look itself.
                        update_config(cx, move |c| {
                            if c.looks.active == look.id {
                                c.looks.active = parent_id;
                                c.looks.print = print;
                            }
                        });
                    }
                    Ok(()) => {
                        let id = look.id.clone();
                        remember(look, cx);
                        update_config(cx, move |c| {
                            if c.looks.active == active || c.looks.active == id {
                                c.looks.active = id;
                                c.looks.print = print;
                            }
                        });
                    }
                    Err(err) => tracing::warn!("looks: saving your look failed: {err:#}"),
                });
            })
            .detach();
        }
    }
}

// ── Actions ─────────────────────────────────────────────────────────────

/// Applies one of your Looks (your setup saved first, as for a built-in).
pub(super) fn apply(look: &UserLook, cx: &mut App) {
    let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
    if let Some(before) = looks::before_snapshot(&cfg, &chrono::Local::now().format("%Y-%m-%d %H:%M").to_string())
        && let Err(err) = mandara_config::profiles::save(&before)
    {
        tracing::warn!("looks: saving your setup failed: {err:#}");
        return;
    }
    match look.apply(&cfg, LookOptions { wallpaper_colors: cfg.looks.wallpaper_colors }) {
        Ok(next) => update_config(cx, move |c| *c = next),
        Err(err) => st(cx).note = Some((format!("{err:#}"), true)),
    }
}

fn arm(key: &str, cx: &mut App) -> bool {
    let s = st(cx);
    if s.armed.as_ref().is_some_and(|(k, at)| k == key && at.elapsed() < ARM_WINDOW) {
        s.armed = None;
        true
    } else {
        s.armed = Some((key.to_string(), Instant::now()));
        false
    }
}

fn armed(key: &str, cx: &mut App) -> bool {
    st(cx).armed.as_ref().is_some_and(|(k, at)| k == key && at.elapsed() < ARM_WINDOW)
}

impl StudioView {
    /// Saves what you changed on the active Look as a new Look of yours.
    pub(super) fn looks_save_new(cx: &mut Context<Self>) {
        let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
        let applied = cfg.looks.active.clone();
        form::job(
            cx,
            move || mandara_ui::plugins::core_commands::save_current_look(&cfg, None).map(|l| (l, ul::owned_print(&cfg))).map_err(|e| format!("{e:#}")),
            move |_, result, cx| match result {
                Ok((look, print)) => {
                    let id = look.id.clone();
                    let s = st(cx);
                    // You go on editing it: your next changes update it.
                    s.session.insert(applied, id.clone());
                    s.note = Some((tf("Saved as {name}", &[("name", &look.name)]), false));
                    remember(look, cx);
                    update_config(cx, move |c| {
                        c.looks.active = id;
                        c.looks.print = print;
                    });
                }
                Err(e) => st(cx).note = Some((e, true)),
            },
        );
    }

    /// Picks a Look file through the portal and imports it.
    pub(super) fn looks_import(cx: &mut Context<Self>) {
        form::job(
            cx,
            || {
                let start = dirs::download_dir().filter(|d| d.is_dir());
                let picked = mandara_services::file_chooser::open_files(t("Import a Look"), t("Mandara Looks"), &[], &["*.toml"], start)?;
                let Some(path) = picked.into_iter().next() else { return Ok(None) };
                let text = mandara_ui::plugins::core_commands::read_look_file(&path)?;
                ul::import_in(&ul::looks_dir(), &text).map(Some).map_err(|e| format!("{e:#}"))
            },
            |_, result, cx| match result {
                Ok(Some(look)) => {
                    st(cx).note = Some((tf("Imported {name}", &[("name", &look.name)]), false));
                    remember(look, cx);
                }
                Ok(None) => {}
                Err(e) => st(cx).note = Some((tf("Couldn't import the look: {error}", &[("error", &e)]), true)),
            },
        );
    }

    fn looks_export(id: String, cx: &mut Context<Self>) {
        form::job(
            cx,
            move || {
                let text = ul::export_text_in(&ul::looks_dir(), &id, &today()).map_err(|e| format!("{e:#}"))?;
                let path = mandara_ui::plugins::core_commands::look_export_path(&id);
                mandara_config::write_atomic(&path, text.as_bytes()).map_err(|e| format!("{e:#}"))?;
                Ok(path)
            },
            |_, result, cx| {
                st(cx).note = Some(match result {
                    Ok(path) => (tf("Exported to {file}", &[("file", &super::apps_page::tilde(&path))]), false),
                    Err(e) => (e, true),
                });
            },
        );
    }

    fn looks_duplicate(id: String, cx: &mut Context<Self>) {
        form::job(cx, move || ul::duplicate_in(&ul::looks_dir(), &id).map_err(|e| format!("{e:#}")), |_, result, cx| match result {
            Ok(look) => remember(look, cx),
            Err(e) => st(cx).note = Some((e, true)),
        });
    }

    fn looks_delete(id: String, cx: &mut Context<Self>) {
        form::job(cx, { let id = id.clone(); move || ul::delete(&id).map_err(|e| format!("{e:#}")) }, move |_, result, cx| match result {
            Ok(()) => {
                forget(&id, cx);
                // Deleting the Look you're on leaves your setup as it is.
                update_config(cx, move |c| {
                    if c.looks.active == id {
                        c.looks.active.clear();
                    }
                });
            }
            Err(e) => st(cx).note = Some((e, true)),
        });
    }

    fn looks_rename(id: String, cx: &mut Context<Self>) {
        let name = form::value(RENAME_FIELD, cx);
        if name.is_empty() {
            return;
        }
        form::job(cx, move || ul::rename_in(&ul::looks_dir(), &id, &name).map_err(|e| format!("{e:#}")), |_, result, cx| {
            match result {
                Ok(look) => {
                    st(cx).renaming = None;
                    remember(look, cx);
                }
                Err(e) => st(cx).note = Some((e, true)),
            }
        });
    }

    /// The Looks heading's buttons of your own: Save as new Look (while a
    /// Look is applied) and Import.
    pub(super) fn looks_yours_options(cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::with_capacity(2);
        if !cfg.looks.active.is_empty() {
            out.push(chip("looks-save-new", t("Save as new Look"), false, p).child(icon("plus", ICON_12_PX, p.primary)).on_click(cx.listener(|_, _: &ClickEvent, _, cx| Self::looks_save_new(cx))).into_any_element());
        }
        out.push(chip("looks-import", t("Import a Look…"), false, p).child(icon("arrow-up", ICON_12_PX, p.primary)).on_click(cx.listener(|_, _: &ClickEvent, _, cx| Self::looks_import(cx))).into_any_element());
        out
    }

    /// The last action's note, if any.
    pub(super) fn looks_note(p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        st(cx).note.clone().map(|(text, error)| form::note(&text, error, p))
    }

    /// A card's small action buttons: export (every Look); rename,
    /// duplicate and delete (yours).
    pub(super) fn look_actions(id: &str, yours: bool, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div().row().items_center().gap(px(SPACE_3_PX));
        let button = |key: String, glyph: &'static str, hint: &'static str, danger: bool| {
            let hover = if danger { p.urgent_coral.with_alpha(ALPHA_14) } else { p.on_surface.with_alpha(ALPHA_8) };
            div()
                .id(SharedString::from(key))
                .size(px(DIM_26_PX))
                .flex_none()
                .rounded_full()
                .row()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |s| s.bg(hover.hsla()))
                .child(icon(glyph, ICON_12_PX, if danger { p.urgent_coral } else { p.on_surface_variant }))
                .tooltip(tip(hint, p))
        };
        if yours {
            let rid = id.to_string();
            row = row.child(button(format!("look-rename-{id}"), "pencil", t("Rename"), false).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                let name = st(cx).looks.as_deref().and_then(|l| l.iter().find(|x| x.id == rid).map(|x| x.name.clone())).unwrap_or_default();
                st(cx).renaming = Some(rid.clone());
                form::fill(RENAME_FIELD, &name, cx);
                cx.notify();
            })));
            let did = id.to_string();
            row = row.child(button(format!("look-duplicate-{id}"), "copy", t("Duplicate"), false).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                Self::looks_duplicate(did.clone(), cx);
            })));
        }
        let eid = id.to_string();
        row = row.child(button(format!("look-export-{id}"), "download", t("Export"), false).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            cx.stop_propagation();
            Self::looks_export(eid.clone(), cx);
        })));
        if yours {
            let key = format!("look-delete-{id}");
            let sure = armed(&key, cx);
            let xid = id.to_string();
            let del = button(key.clone(), "trash", if sure { t("Click again to delete") } else { t("Delete") }, true).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                if arm(&key, cx) {
                    Self::looks_delete(xid.clone(), cx);
                }
                cx.notify();
            }));
            row = row.child(if sure { del.bg(p.urgent_coral.with_alpha(ALPHA_14).hsla()) } else { del });
        }
        row.into_any_element()
    }

    /// The rename field and its buttons, while `id` is being renamed.
    pub(super) fn look_rename_row(&mut self, id: &str, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if st(cx).renaming.as_deref() != Some(id) {
            return None;
        }
        let field = form::input(RENAME_FIELD, tk("Name"), false, cx);
        let (save_id, _) = (id.to_string(), ());
        Some(
            div()
                .id(SharedString::from(format!("look-rename-row-{id}")))
                .row()
                .items_center()
                .gap(px(SPACE_6_PX))
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(div().flex_1().min_w(px(0.0)).child(field))
                .child(chip(SharedString::from(format!("look-rename-save-{id}")), t("Save"), true, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    Self::looks_rename(save_id.clone(), cx);
                })))
                .child(chip(SharedString::from(format!("look-rename-cancel-{id}")), t("Cancel"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    st(cx).renaming = None;
                    cx.notify();
                })))
                .into_any_element(),
        )
    }
}

/// "Yours" and the Look it comes from, for a card's title line.
pub(super) fn yours_badges(look: &UserLook, p: &MandaraPalette) -> AnyElement {
    let parent = look.parent().map_or("", |l| l.name);
    div()
        .row()
        .flex_wrap()
        .items_center()
        .gap(px(SPACE_4_PX))
        .child(badge(t("Yours"), p.tertiary))
        .when(!parent.is_empty(), |el| el.child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(tf("From {look}", &[("look", &parent)]))))
        .into_any_element()
}
