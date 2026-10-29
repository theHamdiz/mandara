//! Studio → Users: the people who use this computer, through
//! AccountsService (`mandara_services::accounts`).
//!
//! * Cards with large circular avatars (a gradient ring marks
//!   administrators), name, username, role and last login.
//! * A detail sheet edits full name, e-mail, language, account type,
//!   automatic login, locking, the password (strength meter + match check)
//!   and the profile photo.
//! * "Add user": full name → a suggested, live-validated username, account
//!   type, and a password now or "let them choose at first login".
//! * Delete with a confirmation that keeps or removes the home folder;
//!   deleting yourself or the last administrator is refused.
//! * The avatar editor crops a photo picked through the desktop portal
//!   (zoom, drag / sliders to position, circular preview) or uses one of
//!   the procedural Mandara presets.
//!
//! Every change is polkit-authorized: the shell's own agent (or any other
//! running agent) asks for a password when needed. `mandara studio users
//! --demo` swaps in an in-memory backend with fictional accounts.

use mandara_ui::bidi::Bidi as _;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, ElementId, Entity, FontWeight, Global, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, ParentElement, Pixels, Point, Render, SharedString, Size, Stateful,
    StatefulInteractiveElement, Styled, StyledImage as _, Window, anchored, deferred, div, img, point, prelude::FluentBuilder as _, px,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_services::accounts::avatar::{self, Crop, PRESETS, Prepared};
use mandara_services::accounts::mock::MockAccounts;
use mandara_services::accounts::validate::{self, LANGUAGES};
use mandara_services::accounts::{
    self, AccountType, AccountsBackend, AccountsError, DbusAccounts, ErrorKind, NewPassword, NewUser, PasswordMode, UserAccount, UserChange,
};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::icons::icon;
use mandara_ui::text_field::TextField;

use super::StudioView;
use super::compositor_page::{badge, button, card, chip, group_title, heading, tip};
use crate::tokens::*;

/// `mandara studio users --demo`: fictional accounts, nothing changes.
static DEMO: AtomicBool = AtomicBool::new(false);
/// Demo only: opens a dialog once the accounts load, for screenshots and
/// layout checks without pointer automation — `edit:<uid>`, `add:<full
/// name>`, `delete:<uid>`, `avatar:<uid>`, `avatar-preset:<uid>:<n>`,
/// `avatar-photo:<uid>:<image path>`, `system` (system accounts shown).
const DEMO_DIALOG_ENV: &str = "MANDARA_USERS_DEMO_DIALOG";
/// Password the `add:` demo dialog fills in, to show the strength meter.
const DEMO_PASSWORD: &str = "Corniche-Lighthouse-1952";

/// Switches the Users page to its in-memory demo backend (call before
/// Studio opens).
pub fn set_demo() {
    DEMO.store(true, Ordering::Relaxed);
}

// ── Layout ──────────────────────────────────────────────────────────────────
const CARD_AVATAR: f32 = 84.0;
const DETAIL_AVATAR: f32 = 92.0;
/// Gradient ring around administrators' avatars, and the gap inside it.
const RING_W: f32 = 3.0;
const RING_GAP: f32 = 3.0;
const EDITOR_PREVIEW: f32 = 196.0;
const EDITOR_RING: f32 = 4.0;
/// The preview's mask ring, relative to its diameter: it must cover the
/// clip square's corners, i.e. at least (√2 − 1) / 2 ≈ 0.207.
const MASK_RATIO: f32 = 0.25;
const PRESET_THUMB: f32 = 54.0;
const MODAL_W: f32 = 620.0;
const MODAL_MARGIN: f32 = 28.0;
const SWITCH_W: f32 = 40.0;
const SWITCH_H: f32 = 22.0;
const SWITCH_KNOB: f32 = 16.0;
const STRENGTH_SEGMENT_H: f32 = 6.0;
const SLIDER_W: f32 = 220.0;
/// Monogram size relative to the avatar.
const MONOGRAM_SCALE: f32 = 0.36;
/// Gradient angle of monogram avatars and admin rings (CSS degrees).
const GRADIENT_ANGLE: f32 = 145.0;

// ── Timing ──────────────────────────────────────────────────────────────────
const POLL: Duration = Duration::from_millis(500);
const NOTICE_TTL: Duration = Duration::from_secs(5);
/// The page counts as hidden once it hasn't rendered for this long.
const HIDDEN_AFTER: Duration = Duration::from_secs(2);

// ── Relative time ───────────────────────────────────────────────────────────
const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const MONTH: i64 = 30 * DAY;

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or_default()
}

/// "Last login 2 h ago", "Never signed in"…
fn last_login(ts: i64) -> String {
    if ts <= 0 {
        return t("Never signed in").to_string();
    }
    let ago = (now_secs() - ts).max(0);
    if ago < MINUTE {
        t("Signed in just now").to_string()
    } else if ago < HOUR {
        tf("Signed in {n} min ago", &[("n", &(ago / MINUTE).to_string())])
    } else if ago < DAY {
        tf("Signed in {n} h ago", &[("n", &(ago / HOUR).to_string())])
    } else if ago < MONTH {
        tf("Signed in {n} d ago", &[("n", &(ago / DAY).to_string())])
    } else {
        let date = chrono::DateTime::from_timestamp(ts, 0).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
        tf("Last signed in on {date}", &[("date", &date)])
    }
}

fn error_text(e: &AccountsError) -> String {
    match e.kind {
        ErrorKind::NotAuthorized => t("Not authorized: the request was cancelled or the password was wrong").to_string(),
        ErrorKind::Unavailable => t("AccountsService isn't running (accounts-daemon)").to_string(),
        ErrorKind::NoSuchUser => t("That user no longer exists").to_string(),
        ErrorKind::Failed => e.message.clone(),
    }
}

/// Dialog scrim opacity in dark and light mode.
const SCRIM_DARK: f32 = 0.55;
const SCRIM_LIGHT: f32 = 0.32;
/// Dialog drop shadow: vertical offset and blur.
const DIALOG_SHADOW_Y_PX: f32 = 18.0;
const DIALOG_SHADOW_BLUR_PX: f32 = 48.0;
/// Multiplier of the user-name hash (the classic 31 polynomial hash).
const NAME_HASH_MULTIPLIER: usize = 31;
/// Crop the demo photo dialog opens with (screenshots).
const DEMO_CROP: Crop = Crop { zoom: 1.6, center_x: 0.42, center_y: 0.5 };
/// A crop slider with nothing to move sits in the middle.
const CENTERED: f32 = 0.5;
/// Avatar in the delete-user dialog.
const DELETE_AVATAR_PX: f32 = 52.0;
/// Monogram size in a list row.
const LIST_MONOGRAM_PX: f32 = 30.0;

/// A stable preset for users without a photo, so their monogram keeps its
/// colors everywhere.
fn preset_for(name: &str) -> usize {
    name.bytes().fold(0usize, |h, b| h.wrapping_mul(NAME_HASH_MULTIPLIER).wrapping_add(usize::from(b))) % PRESETS.len()
}

fn preset_colors(index: usize) -> (Color, Color) {
    let p = PRESETS[index % PRESETS.len()];
    (Color::oklch(p.from.0, p.from.1, p.from.2).gamut_mapped(), Color::oklch(p.to.0, p.to.1, p.to.2).gamut_mapped())
}

fn set_border(el: &mut impl Styled, w: f32) {
    let w: gpui::AbsoluteLength = px(w).into();
    let widths = &mut el.style().border_widths;
    widths.top = Some(w);
    widths.right = Some(w);
    widths.bottom = Some(w);
    widths.left = Some(w);
}

// ── State ───────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
enum Load {
    Loading,
    Ready,
    Failed(String),
}

struct Notice {
    ok: bool,
    text: String,
    at: Instant,
}

/// The detail sheet of one user.
struct Editor {
    uid: u64,
    name: Entity<TextField>,
    email: Entity<TextField>,
    pw1: Entity<TextField>,
    pw2: Entity<TextField>,
    email_invalid: bool,
}

struct AddForm {
    full_name: Entity<TextField>,
    user_name: Entity<TextField>,
    pw1: Entity<TextField>,
    pw2: Entity<TextField>,
    /// The username the form last suggested; typing something else stops
    /// further suggestions.
    suggested: String,
    custom_name: bool,
    account_type: AccountType,
    password_now: bool,
    language: String,
}

struct DeleteDialog {
    uid: u64,
    remove_files: bool,
}

#[derive(Clone, PartialEq)]
enum AvatarChoice {
    /// The current photo (or monogram), unchanged.
    Keep,
    Photo(Prepared),
    Preset(usize),
    Remove,
}

struct AvatarEditor {
    uid: u64,
    initials: String,
    choice: AvatarChoice,
    crop: Crop,
    picking: bool,
    presets: Vec<PathBuf>,
    /// Drag start: pointer position and the crop at that moment.
    drag: Option<(Point<Pixels>, Crop)>,
}

pub struct UsersPage {
    backend: Option<Arc<dyn AccountsBackend>>,
    users: Vec<UserAccount>,
    system: Vec<UserAccount>,
    taken: Vec<String>,
    load: Load,
    show_system: bool,
    editor: Option<Editor>,
    adding: Option<AddForm>,
    deleting: Option<DeleteDialog>,
    avatar: Option<AvatarEditor>,
    /// What's waiting on the daemon / polkit right now.
    busy: Option<String>,
    notice: Option<Notice>,
    /// Photos set in this session (GPUI caches images by path and
    /// AccountsService reuses its icon path); `None` = removed.
    photo_override: HashMap<u64, Option<PathBuf>>,
    dirty: Arc<AtomicBool>,
    /// The signal / notice loop is running (only while the page is shown).
    polling: bool,
    /// Last render: the loop stops once the page hasn't been drawn for
    /// [`HIDDEN_AFTER`].
    rendered_at: Instant,
    /// Demo dialog still to open ([`DEMO_DIALOG_ENV`]).
    demo_dialog: Option<String>,
}

struct PageHandle(Entity<UsersPage>);
impl Global for PageHandle {}

/// The page element for Studio's body (one page entity per process).
pub(super) fn element(cx: &mut Context<StudioView>) -> AnyElement {
    let page = match cx.try_global::<PageHandle>() {
        Some(h) => h.0.clone(),
        None => {
            let page = cx.new(UsersPage::new);
            cx.set_global(PageHandle(page.clone()));
            page
        }
    };
    page.into_any_element()
}

/// A text field whose every keystroke / commit reaches the page.
fn field(cx: &mut Context<UsersPage>, value: &str, placeholder: &'static str, live: bool, masked: bool, on: impl Fn(&mut UsersPage, String, &mut Context<UsersPage>) + 'static) -> Entity<TextField> {
    let this = cx.entity().downgrade();
    let on = Rc::new(on);
    cx.new(|cx| {
        let f = TextField::new(
            value.to_string(),
            placeholder,
            live,
            Rc::new(move |v, _, cx: &mut App| {
                let on = on.clone();
                let _ = this.update(cx, move |page, cx| {
                    on(page, v, cx);
                    cx.notify();
                });
            }),
            cx,
        );
        if masked { f.masked() } else { f }
    })
}

impl UsersPage {
    fn new(cx: &mut Context<Self>) -> Self {
        let demo = DEMO.load(Ordering::Relaxed);
        let (backend, load): (Option<Arc<dyn AccountsBackend>>, Load) = if demo {
            (Some(Arc::new(MockAccounts::demo())), Load::Loading)
        } else {
            match DbusAccounts::connect() {
                Ok(b) => (Some(Arc::new(b)), Load::Loading),
                Err(e) => (None, Load::Failed(error_text(&e))),
            }
        };
        let dirty = Arc::new(AtomicBool::new(false));
        if !demo && backend.is_some() {
            let flag = dirty.clone();
            accounts::watch(move || {
                flag.store(true, Ordering::Relaxed);
                true
            });
        }
        let mut page = UsersPage {
            backend,
            users: Vec::new(),
            system: Vec::new(),
            taken: Vec::new(),
            load,
            show_system: false,
            editor: None,
            adding: None,
            deleting: None,
            avatar: None,
            busy: None,
            notice: None,
            photo_override: HashMap::new(),
            dirty,
            polling: false,
            rendered_at: Instant::now(),
            demo_dialog: if demo { std::env::var(DEMO_DIALOG_ENV).ok() } else { None },
        };
        page.reload(cx);
        page
    }

    /// Reloads on AccountsService signals and expires notices, every
    /// [`POLL`] while the page is on screen; restarted by the next render.
    fn ensure_polling(&mut self, cx: &mut Context<Self>) {
        self.rendered_at = Instant::now();
        if std::mem::replace(&mut self.polling, true) {
            return;
        }
        if self.dirty.swap(false, Ordering::Relaxed) {
            self.reload(cx);
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let running = this
                    .update(cx, |page, cx| {
                        if page.rendered_at.elapsed() > HIDDEN_AFTER {
                            page.polling = false;
                            return false;
                        }
                        if page.dirty.swap(false, Ordering::Relaxed) {
                            page.reload(cx);
                        }
                        if page.notice.as_ref().is_some_and(|n| n.at.elapsed() > NOTICE_TTL) {
                            page.notice = None;
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !running {
                    return;
                }
            }
        })
        .detach();
    }

    fn is_demo(&self) -> bool {
        self.backend.as_ref().is_some_and(|b| b.is_demo())
    }

    fn current_uid(&self) -> u64 {
        self.backend.as_ref().map(|b| b.current_uid()).unwrap_or_default()
    }

    fn user(&self, uid: u64) -> Option<&UserAccount> {
        self.users.iter().find(|u| u.uid == uid)
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else { return };
        cx.spawn(async move |this, cx| {
            let (users, system, taken) = cx.background_executor().spawn(async move { (backend.list_users(), backend.system_accounts(), backend.taken_names()) }).await;
            let _ = this.update(cx, |page, cx| {
                match users {
                    Ok(users) => {
                        page.users = users;
                        page.load = Load::Ready;
                    }
                    Err(e) => page.load = Load::Failed(error_text(&e)),
                }
                page.system = system;
                page.taken = taken;
                if let Some(spec) = page.demo_dialog.take() {
                    page.open_demo_dialog(&spec, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_demo_dialog(&mut self, spec: &str, cx: &mut Context<Self>) {
        // `kind:arg:rest`
        const DEMO_SPEC_PARTS: usize = 3;
        let mut parts = spec.splitn(DEMO_SPEC_PARTS, ':');
        let kind = parts.next().unwrap_or_default();
        let arg = parts.next().unwrap_or_default().to_string();
        let uid = arg.parse::<u64>().unwrap_or_default();
        match kind {
            "edit" => self.open_editor(uid, cx),
            "system" => self.show_system = true,
            "delete" => {
                self.open_editor(uid, cx);
                self.deleting = Some(DeleteDialog { uid, remove_files: false });
            }
            "add" => {
                self.open_add(cx);
                if let Some(form) = self.adding.as_mut() {
                    let suggestion = validate::suggest_username(&arg, &self.taken);
                    form.suggested = suggestion.clone();
                    form.full_name.update(cx, |f, _| f.set(&arg));
                    form.user_name.update(cx, |f, _| f.set(&suggestion));
                    form.pw1.update(cx, |f, _| f.set(DEMO_PASSWORD));
                    form.pw2.update(cx, |f, _| f.set(DEMO_PASSWORD));
                }
            }
            "avatar" | "avatar-preset" | "avatar-photo" => {
                self.open_editor(uid, cx);
                self.open_avatar(uid, cx);
                let rest = parts.next().unwrap_or_default().to_string();
                if kind == "avatar-preset"
                    && let (Some(ed), Ok(i)) = (self.avatar.as_mut(), rest.parse::<usize>())
                {
                    ed.choice = AvatarChoice::Preset(i % PRESETS.len());
                }
                if kind == "avatar-photo" {
                    match avatar::prepare_source(std::path::Path::new(&rest)) {
                        Ok(prep) => {
                            if let Some(ed) = self.avatar.as_mut() {
                                ed.crop = clamp_crop(&prep, DEMO_CROP);
                                ed.choice = AvatarChoice::Photo(prep);
                            }
                        }
                        Err(e) => tracing::warn!("users demo: {e}"),
                    }
                }
            }
            _ => tracing::warn!("users demo: unknown dialog {spec:?}"),
        }
    }

    fn notify_result(&mut self, result: Result<String, String>, cx: &mut Context<Self>) {
        self.notice = Some(match result {
            Ok(text) => Notice { ok: true, text, at: Instant::now() },
            Err(text) => Notice { ok: false, text, at: Instant::now() },
        });
        cx.notify();
    }

    /// Runs `job` off the UI thread (polkit may prompt), then reloads.
    fn run(
        &mut self,
        waiting: String,
        done: String,
        job: impl FnOnce(&dyn AccountsBackend) -> accounts::Result<()> + Send + 'static,
        on_ok: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(backend) = self.backend.clone() else { return };
        self.busy = Some(waiting);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { job(backend.as_ref()) }).await;
            let _ = this.update(cx, |page, cx| {
                page.busy = None;
                match result {
                    Ok(()) => {
                        on_ok(page, cx);
                        page.notify_result(Ok(done), cx);
                    }
                    Err(e) => page.notify_result(Err(error_text(&e)), cx),
                }
                page.reload(cx);
            });
        })
        .detach();
    }

    /// Applies one change to one user (optimistically in the list).
    fn change(&mut self, uid: u64, change: UserChange, cx: &mut Context<Self>) {
        if let Some(u) = self.users.iter_mut().find(|u| u.uid == uid) {
            change.apply_to(u);
        }
        let request = change.request(uid);
        self.run(t("Waiting for authorization…").to_string(), t("Saved").to_string(), move |b| b.execute(&request).map(|_| ()), |_, _| {}, cx);
    }

    // ── Opening dialogs ─────────────────────────────────────────────────────

    fn open_editor(&mut self, uid: u64, cx: &mut Context<Self>) {
        let Some(user) = self.user(uid).cloned() else { return };
        let name = field(cx, &user.real_name, tk("Full name"), false, false, move |page, v, cx| {
            if page.user(uid).is_some_and(|u| u.real_name != v.trim()) && !v.trim().is_empty() {
                page.change(uid, UserChange::RealName(v), cx);
            }
        });
        let email = field(cx, &user.email, tk("name@example.org"), false, false, move |page, v, cx| {
            let valid = validate::valid_email(&v);
            if let Some(ed) = page.editor.as_mut() {
                ed.email_invalid = !valid;
            }
            if valid && page.user(uid).is_some_and(|u| u.email != v.trim()) {
                page.change(uid, UserChange::Email(v), cx);
            }
        });
        let pw1 = field(cx, "", tk("New password"), true, true, |_, _, _| {});
        let pw2 = field(cx, "", tk("Repeat the password"), true, true, |_, _, _| {});
        self.editor = Some(Editor { uid, name, email, pw1, pw2, email_invalid: false });
        cx.notify();
    }

    fn open_add(&mut self, cx: &mut Context<Self>) {
        let full_name = field(cx, "", tk("Full name, e.g. Salma Adel"), true, false, |page, v, cx| {
            let taken = page.taken.clone();
            if let Some(form) = page.adding.as_mut()
                && !form.custom_name
            {
                let suggestion = if v.trim().is_empty() { String::new() } else { validate::suggest_username(&v, &taken) };
                form.suggested = suggestion.clone();
                form.user_name.update(cx, |f, _| f.set(&suggestion));
            }
        });
        let user_name = field(cx, "", tk("username"), true, false, |page, v, _| {
            if let Some(form) = page.adding.as_mut() {
                form.custom_name = !v.is_empty() && v != form.suggested;
            }
        });
        let pw1 = field(cx, "", tk("Password"), true, true, |_, _, _| {});
        let pw2 = field(cx, "", tk("Repeat the password"), true, true, |_, _, _| {});
        self.adding = Some(AddForm { full_name, user_name, pw1, pw2, suggested: String::new(), custom_name: false, account_type: AccountType::Standard, password_now: true, language: String::new() });
        cx.notify();
    }

    fn open_avatar(&mut self, uid: u64, cx: &mut Context<Self>) {
        let Some(user) = self.user(uid) else { return };
        let initials = user.initials();
        self.avatar = Some(AvatarEditor { uid, initials: initials.clone(), choice: AvatarChoice::Keep, crop: Crop::default(), picking: false, presets: Vec::new(), drag: None });
        cx.spawn(async move |this, cx| {
            let files = cx
                .background_executor()
                .spawn(async move { (0..PRESETS.len()).filter_map(|i| avatar::preset_file(i, &initials, avatar::AVATAR_SIZE).ok()).collect::<Vec<_>>() })
                .await;
            let _ = this.update(cx, |page, cx| {
                if let Some(ed) = page.avatar.as_mut().filter(|e| e.uid == uid) {
                    ed.presets = files;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn pick_photo(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.avatar.as_mut() else { return };
        ed.picking = true;
        cx.notify();
        let (title, filter) = (t("Choose a profile photo"), t("Images"));
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match mandara_services::file_chooser::pick_image(title, filter)? {
                        Some(path) => avatar::prepare_source(&path).map(Some),
                        None => Ok(None),
                    }
                })
                .await;
            let _ = this.update(cx, |page, cx| {
                let Some(ed) = page.avatar.as_mut() else { return };
                ed.picking = false;
                match result {
                    Ok(Some(prep)) => {
                        ed.choice = AvatarChoice::Photo(prep);
                        ed.crop = Crop::default();
                    }
                    Ok(None) => {}
                    Err(e) => page.notify_result(Err(tf("Couldn't open the photo: {error}", &[("error", &e)])), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_avatar(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.avatar.take() else { return };
        let (uid, initials, crop) = (ed.uid, ed.initials.clone(), ed.crop);
        let choice = ed.choice.clone();
        if choice == AvatarChoice::Keep {
            cx.notify();
            return;
        }
        let saved: Arc<parking_lot_free::Slot> = Arc::default();
        let slot = saved.clone();
        self.run(
            t("Waiting for authorization…").to_string(),
            t("Profile photo updated").to_string(),
            move |b| {
                let path = match &choice {
                    AvatarChoice::Photo(prep) => avatar::write_avatar(&prep.path, crop).map_err(|e| AccountsError::new(ErrorKind::Failed, e))?,
                    AvatarChoice::Preset(i) => avatar::preset_file(*i, &initials, avatar::AVATAR_SIZE).map_err(|e| AccountsError::new(ErrorKind::Failed, e))?,
                    AvatarChoice::Remove | AvatarChoice::Keep => PathBuf::new(),
                };
                b.execute(&UserChange::IconFile(path.clone()).request(uid))?;
                slot.set(path);
                Ok(())
            },
            move |page, _| {
                let path = saved.take();
                page.photo_override.insert(uid, (!path.as_os_str().is_empty()).then_some(path));
            },
            cx,
        );
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.adding.as_ref() else { return };
        let new = NewUser {
            user_name: form.user_name.read(cx).value.trim().to_string(),
            real_name: form.full_name.read(cx).value.trim().to_string(),
            account_type: form.account_type,
            password: if form.password_now { NewPassword::Now(form.pw1.read(cx).value.clone()) } else { NewPassword::AtFirstLogin },
            language: form.language.clone(),
        };
        let name = new.real_name.clone();
        self.run(
            t("Waiting for authorization…").to_string(),
            tf("Created {name}", &[("name", &name)]),
            move |b| accounts::create_account(b, &new).map(|_| ()),
            |page, cx| {
                page.adding = None;
                cx.notify();
            },
            cx,
        );
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.deleting.take() else { return };
        let (uid, remove) = (dialog.uid, dialog.remove_files);
        let name = self.user(uid).map(|u| u.display_name().to_string()).unwrap_or_default();
        self.run(
            t("Waiting for authorization…").to_string(),
            tf("Deleted {name}", &[("name", &name)]),
            move |b| b.execute(&accounts::Request::delete_user(uid, remove)).map(|_| ()),
            move |page, cx| {
                if page.editor.as_ref().is_some_and(|e| e.uid == uid) {
                    page.editor = None;
                }
                cx.notify();
            },
            cx,
        );
    }

    fn change_password(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.editor.as_ref() else { return };
        let uid = ed.uid;
        let password = ed.pw1.read(cx).value.clone();
        ed.pw1.update(cx, |f, _| f.set(""));
        ed.pw2.update(cx, |f, _| f.set(""));
        self.run(
            t("Waiting for authorization…").to_string(),
            t("Password changed").to_string(),
            move |b| accounts::change_password(b, uid, &password, ""),
            |_, _| {},
            cx,
        );
    }

    // ── Pieces ──────────────────────────────────────────────────────────────

    fn photo_of(&self, user: &UserAccount) -> Option<PathBuf> {
        match self.photo_override.get(&user.uid) {
            Some(over) => over.clone(),
            None => user.photo().map(|p| p.to_path_buf()),
        }
    }

    fn avatar_el(&self, user: &UserAccount, d: f32) -> AnyElement {
        match self.photo_of(user) {
            Some(path) => img(path).size(px(d)).flex_none().rounded_full().object_fit(ObjectFit::Cover).into_any_element(),
            None => monogram(&user.initials(), preset_for(&user.user_name), d),
        }
    }
}

fn monogram(initials: &str, preset: usize, d: f32) -> AnyElement {
    let (from, to) = preset_colors(preset);
    div()
        .size(px(d))
        .flex_none()
        .rounded_full()
        .bg(oklab_gradient(GRADIENT_ANGLE, from, to))
        .row()
        .items_center()
        .justify_center()
        .text_size(px(d * MONOGRAM_SCALE))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui::white())
        .child(initials.to_string())
        .into_any_element()
}

/// The avatar inside a gradient ring (administrators) or an equally sized
/// plain one, so cards line up.
fn ringed(avatar: AnyElement, admin: bool, gap_bg: Color, p: &MandaraPalette) -> AnyElement {
    let inner = div().p(px(RING_GAP)).rounded_full().bg(gap_bg.hsla()).child(avatar);
    div()
        .p(px(RING_W))
        .rounded_full()
        .flex_none()
        .when(admin, |el| el.bg(oklab_gradient(GRADIENT_ANGLE, p.accent_gradient_start, p.accent_gradient_end)))
        .child(inner)
        .into_any_element()
}

fn switch(id: impl Into<ElementId>, on: bool, enabled: bool, p: &MandaraPalette) -> Stateful<gpui::Div> {
    let pad = (SWITCH_H - SWITCH_KNOB) / 2.0;
    div()
        .id(id.into())
        .w(px(SWITCH_W))
        .h(px(SWITCH_H))
        .flex_none()
        .rounded_full()
        .relative()
        .bg(if on { p.primary.hsla() } else { p.surface_container_high.hsla() })
        .when(enabled, |el| el.cursor_pointer())
        .when(!enabled, |el| el.opacity(ALPHA_45))
        .child(
            div()
                .absolute()
                .top(px(pad))
                .inset_start(px(if on { SWITCH_W - SWITCH_KNOB - pad } else { pad }))
                .size(px(SWITCH_KNOB))
                .rounded_full()
                .bg(if on { p.on_primary.hsla() } else { p.on_surface_variant.hsla() }),
        )
}

/// A label + help on the start side, a control on the end side.
fn row(label: impl Into<SharedString>, help: impl Into<SharedString>, control: impl IntoElement, p: &MandaraPalette) -> gpui::Div {
    div()
        .row()
        .items_center()
        .gap(px(SPACE_16_PX))
        .py(px(SPACE_8_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .row()
                .flex_col()
                .gap(px(2.0))
                .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).child(label.into()))
                .child(div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(help))),
        )
        .child(control)
}

fn labeled(label: impl Into<SharedString>, control: impl IntoElement, p: &MandaraPalette) -> gpui::Div {
    div().row().flex_col().gap(px(SPACE_6_PX)).child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::MEDIUM).text_color(p.on_surface_variant.hsla()).child(label.into())).child(control)
}

fn hint(text: impl Into<SharedString>, color: Color) -> gpui::Div {
    div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(color.hsla()).child(mandara_ui::bidi::paragraph(text))
}

/// Strength meter + match check for a new password.
fn password_feedback(pw1: &str, pw2: &str, personal: &[&str], p: &MandaraPalette) -> AnyElement {
    let strength = validate::password_strength(pw1, personal);
    let filled = if pw1.is_empty() { 0 } else { strength.0.max(1) };
    let color = match strength.0 {
        0 | 1 => p.urgent_coral,
        2 => p.warning_amber,
        _ => p.success_emerald,
    };
    let segments = (0..validate::Strength::MAX).map(|i| {
        div().flex_1().h(px(STRENGTH_SEGMENT_H)).rounded_full().bg(if i < filled { color.hsla() } else { p.surface_container_high.hsla() })
    });
    div()
        .row()
        .flex_col()
        .gap(px(SPACE_6_PX))
        .child(div().row().gap(px(SPACE_4_PX)).children(segments))
        .child(
            div()
                .row()
                .flex_wrap()
                .justify_between()
                .gap(px(SPACE_8_PX))
                .child(hint(if pw1.is_empty() { t("Use 8 or more characters; a few words make a strong password").to_string() } else { strength.label().to_string() }, if pw1.is_empty() { p.on_surface_variant } else { color }))
                .when(!pw2.is_empty(), |el| {
                    el.child(if pw1 == pw2 { hint(t("Passwords match"), p.success_emerald) } else { hint(t("Passwords don't match"), p.urgent_coral) })
                }),
        )
        .into_any_element()
}

/// Is a new password acceptable to submit?
fn password_ready(pw1: &str, pw2: &str, personal: &[&str]) -> bool {
    !pw1.is_empty() && pw1 == pw2 && validate::password_strength(pw1, personal).0 >= 1
}

/// The circular photo preview: the image placed by the crop inside a
/// square clip, a mask ring hiding the corners, and an accent ring.
fn photo_preview(prep: &Prepared, crop: Crop, d: f32, mask: Color, accent: Color) -> AnyElement {
    let (x, y, side) = avatar::crop_rect(prep.width, prep.height, crop);
    let k = d / side.max(1) as f32;
    let b = d * MASK_RATIO;
    let mut mask_ring = div().absolute().left(px(-b)).top(px(-b)).size(px(d + 2.0 * b)).rounded_full().border_color(mask.hsla());
    set_border(&mut mask_ring, b);
    let mut ring = div().absolute().top_0().left_0().size(px(d + 2.0 * EDITOR_RING)).rounded_full().border_color(accent.hsla());
    set_border(&mut ring, EDITOR_RING);
    div()
        .relative()
        .size(px(d + 2.0 * EDITOR_RING))
        .flex_none()
        .child(
            div()
                .absolute()
                .left(px(EDITOR_RING))
                .top(px(EDITOR_RING))
                .size(px(d))
                .overflow_hidden()
                .child(img(prep.path.clone()).absolute().left(px(-(x as f32) * k)).top(px(-(y as f32) * k)).w(px(prep.width as f32 * k)).h(px(prep.height as f32 * k)).object_fit(ObjectFit::Fill))
                .child(mask_ring),
        )
        .child(ring)
        .into_any_element()
}

/// The valid range of a crop center along one axis.
fn center_range(dim: u32, side: u32) -> (f32, f32) {
    let half = side as f32 / 2.0 / dim.max(1) as f32;
    (half, 1.0 - half)
}

fn crop_side(prep: &Prepared, zoom: f32) -> u32 {
    avatar::crop_rect(prep.width, prep.height, Crop { zoom, ..Crop::default() }).2
}

/// Keeps the center where the crop can actually reach.
fn clamp_crop(prep: &Prepared, crop: Crop) -> Crop {
    let side = crop_side(prep, crop.zoom);
    let (x0, x1) = center_range(prep.width, side);
    let (y0, y1) = center_range(prep.height, side);
    Crop { zoom: crop.zoom.clamp(avatar::MIN_ZOOM, avatar::MAX_ZOOM), center_x: crop.center_x.clamp(x0, x1.max(x0)), center_y: crop.center_y.clamp(y0, y1.max(y0)) }
}

/// A one-shot value handed from a background job to the UI thread.
mod parking_lot_free {
    use std::path::PathBuf;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct Slot(Mutex<PathBuf>);

    impl Slot {
        pub fn set(&self, path: PathBuf) {
            if let Ok(mut g) = self.0.lock() {
                *g = path;
            }
        }

        pub fn take(&self) -> PathBuf {
            self.0.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
        }
    }
}

// ── Rendering ───────────────────────────────────────────────────────────────

impl UsersPage {
    fn header(&self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let can_add = self.backend.is_some() && self.load == Load::Ready;
        div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_12_PX))
            .child(
                div().flex_1().min_w(px(DIM_240_PX)).child(heading(
                    t("People who use this computer"),
                    t("Changes are authorized by polkit: you may be asked for an administrator's password."),
                    p,
                )),
            )
            .child(
                chip("users-system", if self.show_system { tf("Hide system accounts ({n})", &[("n", &self.system.len().to_string())]) } else { tf("Show system accounts ({n})", &[("n", &self.system.len().to_string())]) }, self.show_system, p)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.show_system = !this.show_system;
                        cx.notify();
                    })),
            )
            .when(can_add, |el| {
                el.child(button("users-add", "user-plus", t("Add user"), true, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_add(cx))))
            })
            .into_any_element()
    }

    fn status(&self, p: &MandaraPalette) -> Option<AnyElement> {
        if let Some(waiting) = &self.busy {
            return Some(
                div()
                    .px(px(SPACE_14_PX))
                    .py(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .bg(p.primary.with_alpha(ALPHA_12).hsla())
                    .child(icon("shield", ICON_15_PX, p.primary))
                    .child(div().text_color(p.primary.hsla()).child(waiting.clone()))
                    .into_any_element(),
            );
        }
        self.notice.as_ref().map(|n| {
            let color = if n.ok { p.success_emerald } else { p.urgent_coral };
            div()
                .px(px(SPACE_14_PX))
                .py(px(SPACE_10_PX))
                .rounded(px(RADIUS_14_PX))
                .row()
                .items_center()
                .gap(px(SPACE_10_PX))
                .bg(color.with_alpha(ALPHA_14).hsla())
                .child(icon(if n.ok { "check" } else { "alert" }, ICON_15_PX, color))
                .child(div().flex_1().text_color(color.hsla()).child(n.text.clone()))
                .into_any_element()
        })
    }

    fn user_card(&self, user: &UserAccount, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let uid = user.uid;
        let me = uid == self.current_uid();
        let avatar = ringed(self.avatar_el(user, CARD_AVATAR), user.is_admin(), p.surface_container, p);
        div()
            .id(("user-card", uid as usize))
            .flex_1()
            .p(px(SPACE_18_PX))
            .rounded(px(RADIUS_22_PX))
            .bg(p.surface_container.hsla())
            .shadow(mandara_core::glass_rim(p.specular_rim_light.with_alpha(ALPHA_50), p.specular_rim_shadow.with_alpha(ALPHA_25)))
            .row()
            .flex_col()
            .items_center()
            .gap(px(SPACE_8_PX))
            .cursor_pointer()
            .hover(|s| s.bg(p.surface_container_high.hsla()))
            .when(user.locked, |el| el.opacity(ALPHA_70))
            .child(avatar)
            .child(
                div()
                    .pt(px(SPACE_4_PX))
                    .w_full()
                    .text_center()
                    .text_size(px(TEXT_14_5_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(user.display_name().to_string()),
            )
            .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(format!("@{}", user.user_name)))
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .justify_center()
                    .gap(px(SPACE_6_PX))
                    .child(if user.is_admin() { badge(t("Administrator"), p.primary) } else { badge(t("Standard"), p.on_surface_variant) })
                    .when(me, |el| el.child(badge(t("You"), p.success_emerald)))
                    .when(user.locked, |el| el.child(badge(t("Locked"), p.urgent_coral))),
            )
            .child(div().text_center().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(last_login(user.login_time)))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_editor(uid, cx)))
            .into_any_element()
    }

    fn system_list(&self, p: &MandaraPalette) -> AnyElement {
        let mut col = card(p).gap(px(2.0)).p(px(SPACE_8_PX));
        for u in &self.system {
            col = col.child(
                div()
                    .px(px(SPACE_12_PX))
                    .py(px(SPACE_8_PX))
                    .rounded(px(RADIUS_12_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(monogram(&u.initials(), preset_for(&u.user_name), LIST_MONOGRAM_PX))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .child(div().font_weight(FontWeight::MEDIUM).child(u.user_name.clone()))
                            .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).overflow_hidden().text_ellipsis().whitespace_nowrap().child(if u.real_name.is_empty() { u.shell.clone() } else { format!("{} · {}", u.real_name, u.shell) })),
                    )
                    .child(div().text_size(px(TEXT_11_5_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(p.on_surface_variant.hsla()).child(tf("uid {uid}", &[("uid", &u.uid.to_string())]))),
            );
        }
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(group_title(t("System accounts"), p))
            .child(hint(t("Service accounts belong to your distribution and are shown read-only."), p.on_surface_variant).px(px(SPACE_10_PX)))
            .child(col)
            .into_any_element()
    }

    // ── Modals ──────────────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn modal(&self, id: &'static str, title: String, body: AnyElement, footer: AnyElement, viewport: Size<Pixels>, p: &MandaraPalette, cx: &mut Context<Self>, on_close: fn(&mut Self)) -> AnyElement {
        let w = modal_width(viewport);
        let h = (f32::from(viewport.height) - 2.0 * MODAL_MARGIN).max(0.0);
        deferred(
            anchored().position(point(px(0.0), px(0.0))).child(
                div()
                    .id(SharedString::from(format!("{id}-scrim")))
                    .occlude()
                    .w(viewport.width)
                    .h(viewport.height)
                    .bg(gpui::black().opacity(if p.is_dark { SCRIM_DARK } else { SCRIM_LIGHT }))
                    .row()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .id(id)
                            .w(px(w))
                            .max_h(px(h))
                            .rounded(px(RADIUS_26_PX))
                            .bg(p.surface_container_low.hsla())
                            .shadow(vec![gpui::BoxShadow { color: gpui::black().opacity(ALPHA_35), offset: point(px(0.0), px(DIALOG_SHADOW_Y_PX)), blur_radius: px(DIALOG_SHADOW_BLUR_PX), spread_radius: px(0.0), inset: false }])
                            .row()
                            .flex_col()
                            .text_color(p.on_surface.hsla())
                            .child(
                                div()
                                    .row()
                                    .items_center()
                                    .gap(px(SPACE_10_PX))
                                    .px(px(SPACE_22_PX))
                                    .pt(px(SPACE_18_PX))
                                    .pb(px(SPACE_10_PX))
                                    .child(div().flex_1().text_size(px(TEXT_17_PX)).font_weight(FontWeight::SEMIBOLD).child(title))
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("{id}-close")))
                                            .size(px(DIM_30_PX))
                                            .rounded_full()
                                            .row()
                                            .items_center()
                                            .justify_center()
                                            .cursor_pointer()
                                            .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
                                            .child(icon("close", ICON_13_PX, p.on_surface_variant))
                                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                                on_close(this);
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .child(div().id(SharedString::from(format!("{id}-body"))).flex_1().min_h(px(0.0)).overflow_y_scroll().px(px(SPACE_22_PX)).pb(px(SPACE_8_PX)).child(body))
                            .child(div().row().flex_wrap().items_center().gap(px(SPACE_10_PX)).px(px(SPACE_22_PX)).py(px(SPACE_16_PX)).child(footer)),
                    ),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn language_chips(&self, prefix: &'static str, current: &str, p: &MandaraPalette, cx: &mut Context<Self>, on_pick: fn(&mut Self, String, &mut Context<Self>)) -> AnyElement {
        let mut all: Vec<(String, String)> = vec![(String::new(), t("System default").to_string())];
        all.extend(LANGUAGES.iter().map(|(l, n)| (l.to_string(), n.to_string())));
        let current_code = current.split(['_', '.']).next().unwrap_or("").to_string();
        div()
            .row()
            .flex_wrap()
            .gap(px(SPACE_6_PX))
            .children(all.into_iter().enumerate().map(|(i, (locale, name))| {
                let active = if locale.is_empty() { current.is_empty() } else { locale.split('_').next() == Some(current_code.as_str()) };
                chip(SharedString::from(format!("{prefix}-lang-{i}")), name, active, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    on_pick(this, locale.clone(), cx);
                    cx.notify();
                }))
            }))
            .into_any_element()
    }

    fn editor_modal(&self, viewport: Size<Pixels>, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ed = self.editor.as_ref()?;
        let user = self.user(ed.uid)?.clone();
        let uid = user.uid;
        let me = uid == self.current_uid();
        let demote = accounts::demote_guard(&user, &self.users);
        let delete = accounts::delete_guard(&user, self.current_uid(), &self.users);
        let (pw1, pw2) = (ed.pw1.read(cx).value.clone(), ed.pw2.read(cx).value.clone());
        let personal = [user.user_name.as_str(), user.real_name.as_str()];
        let ready = password_ready(&pw1, &pw2, &personal);

        let top = div()
            .row()
            .items_center()
            .gap(px(SPACE_18_PX))
            .pb(px(SPACE_8_PX))
            .child(
                div()
                    .id("users-edit-avatar")
                    .relative()
                    .cursor_pointer()
                    .child(ringed(self.avatar_el(&user, DETAIL_AVATAR), user.is_admin(), p.surface_container_low, p))
                    .child(
                        div()
                            .absolute()
                            .bottom(px(0.0))
                            .inset_end(px(0.0))
                            .size(px(DIM_32_PX))
                            .rounded_full()
                            .bg(p.primary.hsla())
                            .border_2()
                            .border_color(p.surface_container_low.hsla())
                            .row()
                            .items_center()
                            .justify_center()
                            .child(icon("camera", ICON_15_PX, p.on_primary)),
                    )
                    .tooltip(tip(t("Change profile photo"), p))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_avatar(uid, cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_4_PX))
                    .child(div().text_size(px(TEXT_20_PX)).font_weight(FontWeight::SEMIBOLD).overflow_hidden().text_ellipsis().whitespace_nowrap().child(user.display_name().to_string()))
                    .child(div().text_color(p.on_surface_variant.hsla()).child(format!("@{} · {}", user.user_name, user.home_directory.display())))
                    .child(
                        div()
                            .row()
                            .flex_wrap()
                            .gap(px(SPACE_6_PX))
                            .child(if user.is_admin() { badge(t("Administrator"), p.primary) } else { badge(t("Standard"), p.on_surface_variant) })
                            .when(me, |el| el.child(badge(t("You"), p.success_emerald)))
                            .child(badge(last_login(user.login_time), p.on_surface_variant)),
                    ),
            );

        let profile = div()
            .row()
            .flex_col()
            .gap(px(SPACE_12_PX))
            .child(group_title(t("Profile"), p).px(px(0.0)))
            .child(labeled(t("Full name"), ed.name.clone(), p))
            .child(labeled(t("E-mail"), ed.email.clone(), p))
            .when(ed.email_invalid, |el| el.child(hint(t("That doesn't look like an e-mail address"), p.urgent_coral)))
            .child(labeled(
                t("Language"),
                self.language_chips("edit", &user.language, p, cx, |this, locale, cx| {
                    if let Some(uid) = this.editor.as_ref().map(|e| e.uid) {
                        this.change(uid, UserChange::Language(locale), cx);
                    }
                }),
                p,
            ));

        let kind_chip = |kind: AccountType, label: &'static str, enabled: bool, cx: &mut Context<Self>| {
            let active = user.account_type == kind;
            chip(SharedString::from(format!("edit-kind-{}", kind.code())), t(label), active, p)
                .when(!enabled && !active, |el| el.opacity(ALPHA_45).tooltip(tip(accounts::Guard::LastAdmin.message(), p)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if enabled && !active {
                        this.change(uid, UserChange::AccountType(kind), cx);
                    }
                }))
        };
        let account = div()
            .row()
            .flex_col()
            .gap(px(2.0))
            .pt(px(SPACE_10_PX))
            .child(group_title(t("Account"), p).px(px(0.0)))
            .child(row(
                t("Account type"),
                t("Administrators can install software, change system settings and manage other users."),
                div().row().gap(px(SPACE_6_PX)).child(kind_chip(AccountType::Standard, tk("Standard"), demote.is_ok(), cx)).child(kind_chip(AccountType::Administrator, tk("Administrator"), true, cx)),
                p,
            ))
            .child(row(
                t("Automatic login"),
                t("Sign in without a password when the computer starts."),
                switch("edit-autologin", user.automatic_login, true, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let on = this.user(uid).is_some_and(|u| u.automatic_login);
                    this.change(uid, UserChange::AutomaticLogin(!on), cx);
                })),
                p,
            ))
            .child(row(
                t("Locked"),
                if me { t("You can't lock the account you're signed in with.") } else { t("A locked account can't sign in until it's unlocked.") },
                switch("edit-locked", user.locked, !me, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if me {
                        return;
                    }
                    let on = this.user(uid).is_some_and(|u| u.locked);
                    this.change(uid, UserChange::Locked(!on), cx);
                })),
                p,
            ));

        let mode_text = match user.password_mode {
            PasswordMode::Regular => t("A password is set."),
            PasswordMode::SetAtLogin => t("They'll choose a new password at their next login."),
            PasswordMode::None => t("This account has no password."),
        };
        let password = div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .pt(px(SPACE_10_PX))
            .child(group_title(t("Password"), p).px(px(0.0)))
            .child(hint(mode_text, p.on_surface_variant))
            .child(div().row().flex_wrap().gap(px(SPACE_10_PX)).child(div().flex_1().min_w(px(DIM_200_PX)).child(ed.pw1.clone())).child(div().flex_1().min_w(px(DIM_200_PX)).child(ed.pw2.clone())))
            .child(password_feedback(&pw1, &pw2, &personal, p))
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .gap(px(SPACE_8_PX))
                    .child(button("edit-pw-set", "key", t("Change password"), true, p).when(!ready, |el| el.opacity(ALPHA_45)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if ready {
                            this.change_password(cx);
                        }
                    })))
                    .when(user.password_mode != PasswordMode::SetAtLogin, |el| {
                        el.child(button("edit-pw-next", "refresh", t("Ask for a new one at next login"), false, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.change(uid, UserChange::PasswordMode(PasswordMode::SetAtLogin), cx);
                        })))
                    }),
            );

        let body = div().row().flex_col().gap(px(SPACE_12_PX)).child(top).child(profile).child(account).child(password).into_any_element();
        let delete_btn = div()
            .id("edit-delete")
            .h(px(DIM_34_PX))
            .px(px(SPACE_14_PX))
            .rounded_full()
            .row()
            .items_center()
            .gap(px(SPACE_7_PX))
            .text_size(px(TEXT_12_5_PX))
            .font_weight(FontWeight::MEDIUM)
            .bg(p.urgent_coral.with_alpha(ALPHA_14).hsla())
            .text_color(p.urgent_coral.hsla())
            .child(icon("trash", ICON_14_PX, p.urgent_coral))
            .child(t("Delete user…"))
            .when_some(delete.err(), |el, guard| el.opacity(ALPHA_45).tooltip(tip(guard.message(), p)))
            .when(delete.is_ok(), |el| {
                el.cursor_pointer().hover(|s| s.opacity(ALPHA_85)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.deleting = Some(DeleteDialog { uid, remove_files: false });
                    cx.notify();
                }))
            });
        let footer = div()
            .w_full()
            .row()
            .items_center()
            .child(delete_btn)
            .child(div().flex_1())
            .child(button("edit-done", "check", t("Done"), true, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.editor = None;
                cx.notify();
            })))
            .into_any_element();
        Some(self.modal("users-editor", t("Edit user").to_string(), body, footer, viewport, p, cx, |this| this.editor = None))
    }

    fn add_modal(&self, viewport: Size<Pixels>, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.adding.as_ref()?;
        let full = form.full_name.read(cx).value.clone();
        let name = form.user_name.read(cx).value.clone();
        let (pw1, pw2) = (form.pw1.read(cx).value.clone(), form.pw2.read(cx).value.clone());
        let check = validate::validate_username(&name, &self.taken);
        let personal = [name.as_str(), full.as_str()];
        let ready = !full.trim().is_empty() && check.is_ok() && (!form.password_now || password_ready(&pw1, &pw2, &personal)) && self.busy.is_none();
        let name_hint = match check {
            Ok(()) => hint(t("Available"), p.success_emerald),
            Err(validate::UsernameProblem::Empty) => hint(t("Letters, digits, - and _; used for the home folder"), p.on_surface_variant),
            Err(problem) => hint(problem.message(), p.urgent_coral),
        };
        let kind = |k: AccountType, label: &'static str, help: &'static str, cx: &mut Context<Self>| {
            let active = form.account_type == k;
            div()
                .id(SharedString::from(format!("add-kind-{}", k.code())))
                .flex_1()
                .min_w(px(DIM_200_PX))
                .p(px(SPACE_14_PX))
                .rounded(px(RADIUS_16_PX))
                .cursor_pointer()
                .border_2()
                .border_color(if active { p.primary.hsla() } else { p.surface_container_high.hsla() })
                .bg(if active { p.primary.with_alpha(ALPHA_8).hsla() } else { p.surface_container.hsla() })
                .row()
                .flex_col()
                .gap(px(SPACE_4_PX))
                .child(div().row().items_center().gap(px(SPACE_8_PX)).child(icon(if k == AccountType::Administrator { "shield" } else { "user" }, ICON_16_PX, if active { p.primary } else { p.on_surface_variant })).child(div().font_weight(FontWeight::SEMIBOLD).child(t(label))))
                .child(hint(t(help), p.on_surface_variant))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(f) = this.adding.as_mut() {
                        f.account_type = k;
                    }
                    cx.notify();
                }))
        };
        let pw_mode = |now: bool, label: &'static str, cx: &mut Context<Self>| {
            chip(SharedString::from(format!("add-pw-{now}")), t(label), form.password_now == now, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if let Some(f) = this.adding.as_mut() {
                    f.password_now = now;
                }
                cx.notify();
            }))
        };
        let body = div()
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(labeled(t("Full name"), form.full_name.clone(), p))
            .child(labeled(t("Username"), div().row().flex_col().gap(px(SPACE_6_PX)).child(form.user_name.clone()).child(name_hint), p))
            .child(labeled(
                t("Account type"),
                div()
                    .row()
                    .flex_wrap()
                    .gap(px(SPACE_10_PX))
                    .child(kind(AccountType::Standard, tk("Standard"), tk("Everyday use: their own files and settings."), cx))
                    .child(kind(AccountType::Administrator, tk("Administrator"), tk("Can change system settings and manage users."), cx)),
                p,
            ))
            .child(labeled(
                t("Password"),
                div()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_10_PX))
                    .child(div().row().flex_wrap().gap(px(SPACE_6_PX)).child(pw_mode(true, tk("Set a password now"), cx)).child(pw_mode(false, tk("Let them choose at first login"), cx)))
                    .when(form.password_now, |el| {
                        el.child(div().row().flex_wrap().gap(px(SPACE_10_PX)).child(div().flex_1().min_w(px(DIM_200_PX)).child(form.pw1.clone())).child(div().flex_1().min_w(px(DIM_200_PX)).child(form.pw2.clone())))
                            .child(password_feedback(&pw1, &pw2, &personal, p))
                    })
                    .when(!form.password_now, |el| el.child(hint(t("They'll be asked to choose a password the first time they sign in."), p.on_surface_variant))),
                p,
            ))
            .child(labeled(
                t("Language"),
                self.language_chips("add", &form.language, p, cx, |this, locale, _| {
                    if let Some(f) = this.adding.as_mut() {
                        f.language = locale;
                    }
                }),
                p,
            ))
            .into_any_element();
        let footer = div()
            .w_full()
            .row()
            .items_center()
            .gap(px(SPACE_10_PX))
            .child(div().flex_1())
            .child(button("add-cancel", "close", t("Cancel"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.adding = None;
                cx.notify();
            })))
            .child(button("add-create", "user-plus", t("Create user"), true, p).when(!ready, |el| el.opacity(ALPHA_45)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if ready {
                    this.create(cx);
                }
            })))
            .into_any_element();
        Some(self.modal("users-add", t("Add a user").to_string(), body, footer, viewport, p, cx, |this| this.adding = None))
    }

    fn delete_modal(&self, viewport: Size<Pixels>, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.deleting.as_ref()?;
        let user = self.user(dialog.uid)?;
        let guard = accounts::delete_guard(user, self.current_uid(), &self.users);
        let option = |remove: bool, title: String, help: String, cx: &mut Context<Self>| {
            let active = dialog.remove_files == remove;
            let accent = if remove { p.urgent_coral } else { p.primary };
            div()
                .id(SharedString::from(format!("delete-opt-{remove}")))
                .p(px(SPACE_14_PX))
                .rounded(px(RADIUS_16_PX))
                .cursor_pointer()
                .border_2()
                .border_color(if active { accent.hsla() } else { p.surface_container_high.hsla() })
                .bg(if active { accent.with_alpha(ALPHA_8).hsla() } else { p.surface_container.hsla() })
                .row()
                .items_center()
                .gap(px(SPACE_12_PX))
                .child(
                    div()
                        .size(px(DIM_18_PX))
                        .flex_none()
                        .rounded_full()
                        .row()
                        .items_center()
                        .justify_center()
                        .border_2()
                        .border_color(if active { accent.hsla() } else { p.outline.with_alpha(ALPHA_60).hsla() })
                        .when(active, |el| el.child(div().size(px(DIM_8_PX)).rounded_full().bg(accent.hsla()))),
                )
                .child(div().flex_1().row().flex_col().gap(px(2.0)).child(div().font_weight(FontWeight::SEMIBOLD).child(title)).child(hint(help, p.on_surface_variant)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(d) = this.deleting.as_mut() {
                        d.remove_files = remove;
                    }
                    cx.notify();
                }))
        };
        let home = user.home_directory.display().to_string();
        let body = div()
            .row()
            .flex_col()
            .gap(px(SPACE_12_PX))
            .child(div().row().items_center().gap(px(SPACE_14_PX)).child(self.avatar_el(user, DELETE_AVATAR_PX)).child(hint(tf("{name} won't be able to sign in any more. This can't be undone.", &[("name", &user.display_name())]), p.on_surface_variant).text_size(px(TEXT_13_PX))))
            .child(option(false, t("Keep the home folder").to_string(), tf("Their files stay in {home}", &[("home", &home)]), cx))
            .child(option(true, t("Delete files").to_string(), tf("Removes {home} and everything in it", &[("home", &home)]), cx))
            .when_some(guard.err(), |el, g| el.child(hint(g.message(), p.urgent_coral)))
            .into_any_element();
        let ok = guard.is_ok();
        let footer = div()
            .w_full()
            .row()
            .items_center()
            .gap(px(SPACE_10_PX))
            .child(div().flex_1())
            .child(button("delete-cancel", "close", t("Cancel"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.deleting = None;
                cx.notify();
            })))
            .child(
                div()
                    .id("delete-confirm")
                    .h(px(DIM_34_PX))
                    .px(px(SPACE_16_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .gap(px(SPACE_7_PX))
                    .text_size(px(TEXT_12_5_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(p.urgent_coral.hsla())
                    .text_color(gpui::white())
                    .child(icon("trash", ICON_14_PX, Color::from_rgba8(u8::MAX, u8::MAX, u8::MAX, u8::MAX)))
                    .child(t("Delete user"))
                    .when(!ok, |el| el.opacity(ALPHA_45))
                    .when(ok, |el| el.cursor_pointer().hover(|s| s.opacity(ALPHA_88)).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.delete(cx)))),
            )
            .into_any_element();
        Some(self.modal("users-delete", tf("Delete {name}?", &[("name", &user.display_name())]), body, footer, viewport, p, cx, |this| this.deleting = None))
    }

    fn avatar_modal(&self, viewport: Size<Pixels>, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ed = self.avatar.as_ref()?;
        let user = self.user(ed.uid)?;
        let d = EDITOR_PREVIEW;
        let full = d + 2.0 * EDITOR_RING;
        let ring_only = |content: AnyElement| -> AnyElement {
            let mut ring = div().absolute().top_0().left_0().size(px(full)).rounded_full().border_color(p.primary.hsla());
            set_border(&mut ring, EDITOR_RING);
            div().relative().size(px(full)).flex_none().child(div().absolute().left(px(EDITOR_RING)).top(px(EDITOR_RING)).child(content)).child(ring).into_any_element()
        };
        let preview: AnyElement = match &ed.choice {
            AvatarChoice::Photo(prep) => photo_preview(prep, ed.crop, d, p.surface_container_low, p.primary),
            AvatarChoice::Preset(i) => ring_only(match ed.presets.get(*i) {
                Some(path) => img(path.clone()).size(px(d)).rounded_full().into_any_element(),
                None => monogram(&ed.initials, *i, d),
            }),
            AvatarChoice::Remove => ring_only(monogram(&ed.initials, preset_for(&user.user_name), d)),
            AvatarChoice::Keep => ring_only(self.avatar_el(user, d)),
        };
        let this = cx.entity().downgrade();
        let preview = match &ed.choice {
            AvatarChoice::Photo(prep) => {
                let (prep2, this2) = (prep.clone(), this.clone());
                div()
                    .id("avatar-drag")
                    .cursor_grab()
                    .child(preview)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                        if let Some(ed) = this.avatar.as_mut() {
                            ed.drag = Some((e.position, ed.crop));
                        }
                        cx.stop_propagation();
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &gpui::MouseUpEvent, _, _| {
                        if let Some(ed) = this.avatar.as_mut() {
                            ed.drag = None;
                        }
                    }))
                    .on_mouse_move(move |e: &MouseMoveEvent, _, cx| {
                        if e.pressed_button != Some(MouseButton::Left) {
                            return;
                        }
                        let _ = this2.update(cx, |page, cx| {
                            let Some(ed) = page.avatar.as_mut() else { return };
                            let Some((start, from)) = ed.drag else { return };
                            let side = crop_side(&prep2, from.zoom) as f32;
                            let k = d / side.max(1.0);
                            let delta = e.position - start;
                            let next = Crop {
                                center_x: from.center_x - f32::from(delta.x) / k / prep2.width.max(1) as f32,
                                center_y: from.center_y - f32::from(delta.y) / k / prep2.height.max(1) as f32,
                                ..from
                            };
                            ed.crop = clamp_crop(&prep2, next);
                            cx.notify();
                        });
                    })
                    .into_any_element()
            }
            _ => preview,
        };

        // Zoom + position sliders (photos only).
        let controls: Option<AnyElement> = match &ed.choice {
            AvatarChoice::Photo(prep) => {
                let crop = clamp_crop(prep, ed.crop);
                let side = crop_side(prep, crop.zoom);
                let (x0, x1) = center_range(prep.width, side);
                let (y0, y1) = center_range(prep.height, side);
                let frac = |v: f32, lo: f32, hi: f32| if hi > lo { (v - lo) / (hi - lo) } else { CENTERED };
                let slider = |id: &'static str, label: &'static str, value: f32, enabled: bool, set: fn(&Prepared, Crop, f32) -> Crop| {
                    let (this, prep) = (this.clone(), prep.clone());
                    div()
                        .row()
                        .items_center()
                        .gap(px(SPACE_12_PX))
                        .when(!enabled, |el| el.opacity(ALPHA_40))
                        .child(div().w(px(DIM_96_PX)).flex_none().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t(label)))
                        .child(div().w(px(SLIDER_W)).child(mandara_ui::kit::slider(id, value, p, move |f, _, cx| {
                            if !enabled {
                                return;
                            }
                            let prep = prep.clone();
                            let _ = this.update(cx, move |page, cx| {
                                if let Some(ed) = page.avatar.as_mut() {
                                    ed.crop = clamp_crop(&prep, set(&prep, ed.crop, f));
                                    cx.notify();
                                }
                            });
                        })))
                };
                Some(
                    div()
                        .row()
                        .flex_col()
                        .gap(px(SPACE_8_PX))
                        .child(slider("avatar-zoom", tk("Zoom"), (crop.zoom - avatar::MIN_ZOOM) / (avatar::MAX_ZOOM - avatar::MIN_ZOOM), true, |_, c, f| Crop { zoom: avatar::MIN_ZOOM + f * (avatar::MAX_ZOOM - avatar::MIN_ZOOM), ..c }))
                        .child(slider("avatar-x", tk("Horizontal"), frac(crop.center_x, x0, x1), x1 > x0, |prep, c, f| {
                            let (lo, hi) = center_range(prep.width, crop_side(prep, c.zoom));
                            Crop { center_x: lo + f * (hi - lo), ..c }
                        }))
                        .child(slider("avatar-y", tk("Vertical"), frac(crop.center_y, y0, y1), y1 > y0, |prep, c, f| {
                            let (lo, hi) = center_range(prep.height, crop_side(prep, c.zoom));
                            Crop { center_y: lo + f * (hi - lo), ..c }
                        }))
                        .child(hint(t("Drag the photo to position it."), p.on_surface_variant))
                        .into_any_element(),
                )
            }
            _ => None,
        };

        // Presets fill the dialog's rows: the circles grow with their cells.
        let body_w = modal_width(viewport) - 2.0 * SPACE_22_PX;
        let fit = super::grid::fit(body_w, AVATAR_PRESET_GRID).balanced(PRESETS.len());
        let thumb = (fit.card_w - 2.0 * (SPACE_3_PX + BORDER_2_PX)).floor().max(PRESET_THUMB);
        let thumbs: Vec<_> = (0..PRESETS.len()).map(|i| {
            let active = ed.choice == AvatarChoice::Preset(i);
            let content: AnyElement = match ed.presets.get(i) {
                Some(path) => img(path.clone()).size(px(thumb)).rounded_full().into_any_element(),
                None => monogram(&ed.initials, i, thumb),
            };
            div()
                .id(("avatar-preset", i))
                .p(px(SPACE_3_PX))
                .rounded_full()
                .cursor_pointer()
                .border_2()
                .border_color(if active { p.primary.hsla() } else { gpui::transparent_black() })
                .hover(|s| s.opacity(ALPHA_85))
                .child(content)
                .tooltip(tip(mandara_i18n::tr(PRESETS[i].name), p))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(ed) = this.avatar.as_mut() {
                        ed.choice = AvatarChoice::Preset(i);
                    }
                    cx.notify();
                }))
        }).collect();
        let picking = ed.picking;
        let has_photo = self.photo_of(user).is_some();
        let body = div()
            .row()
            .flex_col()
            .gap(px(SPACE_16_PX))
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_22_PX))
                    .child(preview)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(DIM_240_PX))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_12_PX))
                            .child(
                                div()
                                    .row()
                                    .flex_wrap()
                                    .gap(px(SPACE_8_PX))
                                    .child(button("avatar-pick", "image", if picking { t("Opening…") } else { t("Choose a photo…") }, true, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        if !picking {
                                            this.pick_photo(cx);
                                        }
                                    })))
                                    .when(has_photo, |el| {
                                        el.child(button("avatar-remove", "trash", t("Remove photo"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            if let Some(ed) = this.avatar.as_mut() {
                                                ed.choice = AvatarChoice::Remove;
                                            }
                                            cx.notify();
                                        })))
                                    }),
                            )
                            .when_some(controls, |el, c| el.child(c))
                            .when(!matches!(ed.choice, AvatarChoice::Photo(_)), |el| el.child(hint(t("PNG, JPEG or WebP. It's cropped to a square and saved at 256 × 256."), p.on_surface_variant))),
                    ),
            )
            .child(group_title(t("Mandara presets"), p).px(px(0.0)))
            .child(fit.render(thumbs))
            .into_any_element();
        let changed = ed.choice != AvatarChoice::Keep;
        let footer = div()
            .w_full()
            .row()
            .items_center()
            .gap(px(SPACE_10_PX))
            .child(div().flex_1())
            .child(button("avatar-cancel", "close", t("Cancel"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.avatar = None;
                cx.notify();
            })))
            .child(button("avatar-save", "check", t("Save photo"), true, p).when(!changed, |el| el.opacity(ALPHA_45)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if changed {
                    this.save_avatar(cx);
                }
            })))
            .into_any_element();
        Some(self.modal("users-avatar", t("Profile photo").to_string(), body, footer, viewport, p, cx, |this| this.avatar = None))
    }
}

/// A dialog's width: [`MODAL_W`], or the window less its margins.
fn modal_width(viewport: Size<Pixels>) -> f32 {
    (f32::from(viewport.width) - 2.0 * MODAL_MARGIN).clamp(0.0, MODAL_W)
}

impl Render for UsersPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_polling(cx);
        let p = mandara_core::frame_palette(window, cx);
        let viewport = window.viewport_size();
        let mut page = div().row().flex_col().gap(px(SPACE_20_PX));
        if self.is_demo() {
            page = page.child(
                div()
                    .px(px(SPACE_14_PX))
                    .py(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .bg(p.warning_amber.with_alpha(ALPHA_14).hsla())
                    .child(icon("alert", ICON_15_PX, p.warning_amber))
                    .child(div().flex_1().min_w(px(0.0)).text_color(p.warning_amber.hsla()).child(mandara_ui::bidi::paragraph(t("Demo mode: fictional accounts. Nothing on this computer changes.")))),
            );
        }
        page = page.child(self.header(&p, cx));
        if let Some(status) = self.status(&p) {
            page = page.child(status);
        }
        match self.load.clone() {
            Load::Loading => page = page.child(card(&p).child(div().text_color(p.on_surface_variant.hsla()).child(t("Reading accounts…")))),
            Load::Failed(e) => {
                page = page.child(card(&p).child(heading(t("Accounts aren't available"), tf("Mandara manages users through AccountsService (accounts-daemon): {error}", &[("error", &e)]), &p)));
            }
            Load::Ready => {
                let users = self.users.clone();
                let cards: Vec<AnyElement> = users.iter().map(|u| self.user_card(u, &p, cx)).collect();
                page = page.child(super::grid::fit(super::grid::PageMetrics::of(window).column_w, USER_GRID).balanced(cards.len()).render(cards));
            }
        }
        if self.show_system && !self.system.is_empty() {
            page = page.child(self.system_list(&p));
        }
        // One dialog at a time, the most specific on top.
        let modal = self
            .avatar_modal(viewport, &p, cx)
            .or_else(|| self.deleting.is_some().then(|| self.delete_modal(viewport, &p, cx)).flatten())
            .or_else(|| self.adding.is_some().then(|| self.add_modal(viewport, &p, cx)).flatten())
            .or_else(|| self.editor_modal(viewport, &p, cx));
        page.when_some(modal, |el, m| el.child(m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_login_times() {
        let now = now_secs();
        assert_eq!(last_login(0), "Never signed in");
        assert_eq!(last_login(now - 5), "Signed in just now");
        assert_eq!(last_login(now - 5 * MINUTE), "Signed in 5 min ago");
        assert_eq!(last_login(now - 3 * HOUR), "Signed in 3 h ago");
        assert_eq!(last_login(now - 2 * DAY), "Signed in 2 d ago");
        assert!(last_login(now - 90 * DAY).starts_with("Last signed in on "));
    }

    #[test]
    fn crop_center_stays_reachable() {
        let prep = Prepared { path: PathBuf::new(), width: 400, height: 200 };
        let c = clamp_crop(&prep, Crop { zoom: 2.0, center_x: -1.0, center_y: 2.0 });
        assert!((c.center_x - 0.125).abs() < 1e-4, "{c:?}");
        assert!((c.center_y - 0.75).abs() < 1e-4, "{c:?}");
        // At zoom 1 the vertical axis can't move.
        let (lo, hi) = center_range(200, crop_side(&prep, 1.0));
        assert!((lo - 0.5).abs() < 1e-4 && (hi - 0.5).abs() < 1e-4);
    }

    #[test]
    fn mask_ring_covers_the_corners() {
        assert!(MASK_RATIO >= (std::f32::consts::SQRT_2 - 1.0) / 2.0);
    }

    #[test]
    fn monogram_presets_are_stable() {
        assert_eq!(preset_for("nour"), preset_for("nour"));
        assert!(preset_for("mariam") < PRESETS.len());
    }

    #[test]
    fn password_readiness() {
        assert!(!password_ready("", "", &[]));
        assert!(!password_ready("Corniche-1952!", "Corniche-1952", &[]));
        assert!(password_ready("Corniche-1952!", "Corniche-1952!", &[]));
        assert!(!password_ready("nour1234", "nour1234", &["nour"]));
    }
}
