//! User accounts through AccountsService (`org.freedesktop.Accounts` on the
//! system bus): list, create, edit and delete users, set profile photos and
//! passwords, and watch for changes.
//!
//! Every mutation is polkit-authorized by `accounts-daemon`. Calls carry the
//! D-Bus `ALLOW_INTERACTIVE_AUTHORIZATION` flag so polkit may ask the
//! session's authentication agent (Mandara's own, or any other running
//! agent) for a password instead of failing outright.
//!
//! The D-Bus layer sits behind [`AccountsBackend`], which executes plain
//! [`Request`] values. Studio's `--demo` mode and the unit tests use the
//! in-memory [`mock::MockAccounts`], so mutation logic (validation, hashing,
//! request construction, guards) is tested without touching real accounts.
//!
//! * [`validate`] — usernames, suggestions, password strength, languages.
//! * [`crypt`] — SHA-512 crypt (`$6$`) hashes for `SetPassword`.
//! * [`avatar`] — square crops, resizing and procedural preset avatars.

pub mod avatar;
pub mod crypt;
pub mod mock;
pub mod validate;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt as _;
use serde::Serialize;
use zbus::proxy::{CacheProperties, MethodFlags};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

pub const ACCOUNTS_DEST: &str = "org.freedesktop.Accounts";
pub const ACCOUNTS_PATH: &str = "/org/freedesktop/Accounts";
pub const ACCOUNTS_IFACE: &str = "org.freedesktop.Accounts";
pub const USER_IFACE: &str = "org.freedesktop.Accounts.User";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
/// Where accounts-daemon keeps copied profile photos.
pub const ICON_DIR: &str = "/var/lib/AccountsService/icons";
/// Read-only lookups (avatars for the lock screen / greeter) must never
/// stall a UI thread for long.
const LOOKUP_TIMEOUT: Duration = Duration::from_millis(400);
/// First uid of regular users when `/etc/login.defs` doesn't say.
pub const DEFAULT_UID_MIN: u64 = 1000;
/// `nobody`'s uid: never a regular account even though it is ≥ UID_MIN.
const NOBODY_UID: u64 = 65534;

/// `AccountType`: 0 standard, 1 administrator.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AccountType {
    #[default]
    Standard,
    Administrator,
}

impl AccountType {
    pub const STANDARD_CODE: i32 = 0;
    pub const ADMINISTRATOR_CODE: i32 = 1;

    pub fn code(self) -> i32 {
        match self {
            AccountType::Standard => Self::STANDARD_CODE,
            AccountType::Administrator => Self::ADMINISTRATOR_CODE,
        }
    }

    pub fn from_code(code: i32) -> Self {
        if code == Self::ADMINISTRATOR_CODE { AccountType::Administrator } else { AccountType::Standard }
    }
}

/// `PasswordMode`: 0 regular, 1 set at next login, 2 none (passwordless).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PasswordMode {
    #[default]
    Regular,
    SetAtLogin,
    None,
}

impl PasswordMode {
    pub const REGULAR_CODE: i32 = 0;
    pub const SET_AT_LOGIN_CODE: i32 = 1;
    pub const NONE_CODE: i32 = 2;

    pub fn code(self) -> i32 {
        match self {
            PasswordMode::Regular => Self::REGULAR_CODE,
            PasswordMode::SetAtLogin => Self::SET_AT_LOGIN_CODE,
            PasswordMode::None => Self::NONE_CODE,
        }
    }

    pub fn from_code(code: i32) -> Self {
        match code {
            Self::SET_AT_LOGIN_CODE => PasswordMode::SetAtLogin,
            Self::NONE_CODE => PasswordMode::None,
            _ => PasswordMode::Regular,
        }
    }
}

/// One user, as AccountsService describes it.
#[derive(Clone, Debug, PartialEq, Serialize, Default)]
pub struct UserAccount {
    pub uid: u64,
    pub user_name: String,
    pub real_name: String,
    pub account_type: AccountType,
    /// `None` when the user has no photo.
    pub icon_file: Option<PathBuf>,
    pub email: String,
    /// Locale such as `ar_EG.UTF-8`; empty = the system default.
    pub language: String,
    pub locked: bool,
    pub automatic_login: bool,
    pub password_mode: PasswordMode,
    /// Unix seconds of the last login; 0 = never.
    pub login_time: i64,
    pub home_directory: PathBuf,
    pub shell: String,
    pub system_account: bool,
}

impl UserAccount {
    /// Full name, or the username when it has none.
    pub fn display_name(&self) -> &str {
        if self.real_name.trim().is_empty() { &self.user_name } else { self.real_name.trim() }
    }

    /// Up to two initials of the display name ("Nour El-Sayed" → "NE").
    pub fn initials(&self) -> String {
        initials(self.display_name())
    }

    pub fn is_admin(&self) -> bool {
        self.account_type == AccountType::Administrator
    }

    /// The photo, when it exists on disk.
    pub fn photo(&self) -> Option<&Path> {
        self.icon_file.as_deref().filter(|p| p.is_file())
    }
}

/// Up to two uppercase initials of `name`.
pub fn initials(name: &str) -> String {
    let s: String = name.split(|c: char| c.is_whitespace() || c == '-').filter_map(|w| w.chars().next()).take(2).flat_map(char::to_uppercase).collect();
    if s.is_empty() { "?".into() } else { s }
}

/// Which object a request goes to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Manager,
    User(u64),
}

/// Method arguments, in D-Bus order.
#[derive(Clone, PartialEq)]
pub enum Arg {
    Str(String),
    Int(i32),
    Bool(bool),
    /// `SetPassword(hash, hint)`.
    StrStr(String, String),
    /// `CreateUser(name, real name, account type)`.
    CreateUser(String, String, i32),
    /// `DeleteUser(uid, remove files)`.
    DeleteUser(i64, bool),
}

impl std::fmt::Debug for Arg {
    /// Password hashes never reach logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Arg::Str(s) => write!(f, "Str({s:?})"),
            Arg::Int(i) => write!(f, "Int({i})"),
            Arg::Bool(b) => write!(f, "Bool({b})"),
            Arg::StrStr(_, hint) => write!(f, "StrStr(<redacted>, {hint:?})"),
            Arg::CreateUser(n, r, t) => write!(f, "CreateUser({n:?}, {r:?}, {t})"),
            Arg::DeleteUser(u, rm) => write!(f, "DeleteUser({u}, {rm})"),
        }
    }
}

/// One AccountsService method call.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub target: Target,
    pub method: &'static str,
    pub arg: Arg,
}

impl Request {
    pub fn create_user(user_name: &str, real_name: &str, account_type: AccountType) -> Self {
        Request { target: Target::Manager, method: "CreateUser", arg: Arg::CreateUser(user_name.into(), real_name.into(), account_type.code()) }
    }

    pub fn delete_user(uid: u64, remove_files: bool) -> Self {
        Request { target: Target::Manager, method: "DeleteUser", arg: Arg::DeleteUser(uid as i64, remove_files) }
    }
}

/// A per-user edit (`org.freedesktop.Accounts.User.Set*`).
#[derive(Clone, PartialEq)]
pub enum UserChange {
    RealName(String),
    Email(String),
    /// Locale; empty = system default.
    Language(String),
    AccountType(AccountType),
    Locked(bool),
    AutomaticLogin(bool),
    PasswordMode(PasswordMode),
    /// A readable image file (copied by the daemon); empty path = remove.
    IconFile(PathBuf),
    /// A crypt(3) hash — never the plain password.
    Password { hash: String, hint: String },
}

impl std::fmt::Debug for UserChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserChange::Password { .. } => write!(f, "Password(<redacted>)"),
            other => write!(f, "{:?}", other.request(0).arg),
        }
    }
}

impl UserChange {
    pub fn request(&self, uid: u64) -> Request {
        let (method, arg) = match self {
            UserChange::RealName(s) => ("SetRealName", Arg::Str(s.trim().to_string())),
            UserChange::Email(s) => ("SetEmail", Arg::Str(s.trim().to_string())),
            UserChange::Language(s) => ("SetLanguage", Arg::Str(s.clone())),
            UserChange::AccountType(t) => ("SetAccountType", Arg::Int(t.code())),
            UserChange::Locked(b) => ("SetLocked", Arg::Bool(*b)),
            UserChange::AutomaticLogin(b) => ("SetAutomaticLogin", Arg::Bool(*b)),
            UserChange::PasswordMode(m) => ("SetPasswordMode", Arg::Int(m.code())),
            UserChange::IconFile(p) => ("SetIconFile", Arg::Str(p.to_string_lossy().into_owned())),
            UserChange::Password { hash, hint } => ("SetPassword", Arg::StrStr(hash.clone(), hint.clone())),
        };
        Request { target: Target::User(uid), method, arg }
    }

    /// Applies the change to a local copy (optimistic UI, mock backend).
    pub fn apply_to(&self, user: &mut UserAccount) {
        match self {
            UserChange::RealName(s) => user.real_name = s.trim().to_string(),
            UserChange::Email(s) => user.email = s.trim().to_string(),
            UserChange::Language(s) => user.language = s.clone(),
            UserChange::AccountType(t) => user.account_type = *t,
            UserChange::Locked(b) => user.locked = *b,
            UserChange::AutomaticLogin(b) => user.automatic_login = *b,
            UserChange::PasswordMode(m) => user.password_mode = *m,
            UserChange::IconFile(p) => user.icon_file = (!p.as_os_str().is_empty()).then(|| p.clone()),
            UserChange::Password { .. } => user.password_mode = PasswordMode::Regular,
        }
    }
}

/// What a successful request returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Done,
    /// `CreateUser`: the new user's uid.
    Created(u64),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// Polkit refused, or the authentication dialog was cancelled.
    NotAuthorized,
    /// accounts-daemon isn't running / the system bus is unreachable.
    Unavailable,
    /// The user doesn't exist (any more).
    NoSuchUser,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountsError {
    pub kind: ErrorKind,
    pub message: String,
}

impl AccountsError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        AccountsError { kind, message: message.into() }
    }

    fn from_zbus(err: zbus::Error) -> Self {
        let kind = match &err {
            zbus::Error::MethodError(name, _, _) => classify_error_name(name.as_str()),
            zbus::Error::InputOutput(_) | zbus::Error::Address(_) | zbus::Error::Handshake(_) => ErrorKind::Unavailable,
            _ => ErrorKind::Failed,
        };
        let message = match &err {
            zbus::Error::MethodError(_, Some(detail), _) => detail.clone(),
            other => other.to_string(),
        };
        AccountsError { kind, message }
    }
}

impl std::fmt::Display for AccountsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AccountsError {}

/// Maps a D-Bus error name to an [`ErrorKind`].
pub fn classify_error_name(name: &str) -> ErrorKind {
    match name {
        "org.freedesktop.Accounts.Error.PermissionDenied"
        | "org.freedesktop.DBus.Error.AccessDenied"
        | "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired"
        | "org.freedesktop.PolicyKit1.Error.NotAuthorized"
        | "org.freedesktop.PolicyKit1.Error.Cancelled" => ErrorKind::NotAuthorized,
        "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner" | "org.freedesktop.DBus.Error.Spawn.ServiceNotFound" => {
            ErrorKind::Unavailable
        }
        "org.freedesktop.Accounts.Error.UserDoesNotExist" => ErrorKind::NoSuchUser,
        _ => ErrorKind::Failed,
    }
}

pub type Result<T> = std::result::Result<T, AccountsError>;

/// Where accounts come from: AccountsService ([`DbusAccounts`]) or an
/// in-memory fake ([`mock::MockAccounts`]).
pub trait AccountsBackend: Send + Sync {
    /// Regular (non-system) users.
    fn list_users(&self) -> Result<Vec<UserAccount>>;
    /// System / service accounts (read-only; listed on request).
    fn system_accounts(&self) -> Vec<UserAccount>;
    /// Every login name in use (including system accounts and groups), so
    /// new usernames can't collide.
    fn taken_names(&self) -> Vec<String>;
    fn execute(&self, request: &Request) -> Result<Reply>;
    /// The uid running Studio (it may not delete itself).
    fn current_uid(&self) -> u64;
    /// An in-memory demo (nothing on the system changes).
    fn is_demo(&self) -> bool {
        false
    }
}

/// Everything needed to create an account.
#[derive(Clone)]
pub struct NewUser {
    pub user_name: String,
    pub real_name: String,
    pub account_type: AccountType,
    pub password: NewPassword,
    pub language: String,
}

#[derive(Clone)]
pub enum NewPassword {
    /// Plain text from the form; hashed before it leaves this module.
    Now(String),
    /// `PasswordMode::SetAtLogin`: the user chooses one when first logging in.
    AtFirstLogin,
}

/// Creates a user: validation, `CreateUser`, then the password (hashed) or
/// "set at first login", then the language. Returns the new uid.
pub fn create_account(backend: &dyn AccountsBackend, new: &NewUser) -> Result<u64> {
    let taken = backend.taken_names();
    validate::validate_username(&new.user_name, &taken).map_err(|p| AccountsError::new(ErrorKind::Failed, p.message()))?;
    if let NewPassword::Now(pw) = &new.password
        && pw.is_empty()
    {
        return Err(AccountsError::new(ErrorKind::Failed, mandara_i18n::t("Enter a password, or let the user choose one at first login")));
    }
    let uid = match backend.execute(&Request::create_user(&new.user_name, new.real_name.trim(), new.account_type))? {
        Reply::Created(uid) => uid,
        Reply::Done => return Err(AccountsError::new(ErrorKind::Failed, "CreateUser returned no user")),
    };
    let password = match &new.password {
        NewPassword::Now(pw) => UserChange::Password { hash: crypt::hash_password(pw).map_err(|e| AccountsError::new(ErrorKind::Failed, e))?, hint: String::new() },
        NewPassword::AtFirstLogin => UserChange::PasswordMode(PasswordMode::SetAtLogin),
    };
    backend.execute(&password.request(uid))?;
    if !new.language.is_empty() {
        backend.execute(&UserChange::Language(new.language.clone()).request(uid))?;
    }
    Ok(uid)
}

/// Sets a new password (hashed with SHA-512 crypt).
pub fn change_password(backend: &dyn AccountsBackend, uid: u64, password: &str, hint: &str) -> Result<()> {
    if password.is_empty() {
        return Err(AccountsError::new(ErrorKind::Failed, mandara_i18n::t("The password is empty")));
    }
    let hash = crypt::hash_password(password).map_err(|e| AccountsError::new(ErrorKind::Failed, e))?;
    backend.execute(&UserChange::Password { hash, hint: hint.to_string() }.request(uid)).map(|_| ())
}

/// Why an account may not be deleted or demoted.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Guard {
    IsYou,
    LastAdmin,
    SystemAccount,
}

impl Guard {
    pub fn message(self) -> &'static str {
        match self {
            Guard::IsYou => mandara_i18n::t("You can't delete the account you're signed in with"),
            Guard::LastAdmin => mandara_i18n::t("At least one administrator must remain"),
            Guard::SystemAccount => mandara_i18n::t("System accounts are managed by your distribution"),
        }
    }
}

fn admins_other_than(users: &[UserAccount], uid: u64) -> usize {
    users.iter().filter(|u| u.uid != uid && u.is_admin() && !u.system_account).count()
}

/// May `target` be deleted by `current_uid`?
pub fn delete_guard(target: &UserAccount, current_uid: u64, users: &[UserAccount]) -> std::result::Result<(), Guard> {
    if target.system_account {
        Err(Guard::SystemAccount)
    } else if target.uid == current_uid {
        Err(Guard::IsYou)
    } else if target.is_admin() && admins_other_than(users, target.uid) == 0 {
        Err(Guard::LastAdmin)
    } else {
        Ok(())
    }
}

/// May `target` become a standard user?
pub fn demote_guard(target: &UserAccount, users: &[UserAccount]) -> std::result::Result<(), Guard> {
    if target.is_admin() && admins_other_than(users, target.uid) == 0 { Err(Guard::LastAdmin) } else { Ok(()) }
}

// ───────────────────────────── D-Bus backend ─────────────────────────────

/// AccountsService over the system bus.
pub struct DbusAccounts {
    conn: zbus::Connection,
    uid: u64,
}

impl DbusAccounts {
    /// Connects to the system bus (blocking).
    pub fn connect() -> Result<Self> {
        let conn = crate::runtime::block_on(crate::runtime::system()).map_err(|e| AccountsError::new(ErrorKind::Unavailable, e.to_string()))?;
        Ok(DbusAccounts { conn, uid: current_uid() })
    }

    async fn proxy(&self, path: &str, iface: &'static str) -> zbus::Result<zbus::Proxy<'static>> {
        zbus::proxy::Builder::new(&self.conn)
            .destination(ACCOUNTS_DEST)?
            .path(path.to_string())?
            .interface(iface)?
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }

    async fn user_path(&self, uid: u64) -> zbus::Result<OwnedObjectPath> {
        self.proxy(ACCOUNTS_PATH, ACCOUNTS_IFACE).await?.call("FindUserById", &(uid as i64)).await
    }

    async fn read_user(&self, path: &str) -> zbus::Result<UserAccount> {
        let props: HashMap<String, OwnedValue> = self.proxy(path, PROPERTIES_IFACE).await?.call("GetAll", &(USER_IFACE,)).await?;
        Ok(parse_user(&props))
    }

    async fn list(&self) -> zbus::Result<Vec<UserAccount>> {
        let paths: Vec<OwnedObjectPath> = self.proxy(ACCOUNTS_PATH, ACCOUNTS_IFACE).await?.call("ListCachedUsers", &()).await?;
        let mut users = Vec::with_capacity(paths.len());
        for p in paths {
            match self.read_user(p.as_str()).await {
                Ok(u) => users.push(u),
                Err(e) => tracing::debug!("accounts: reading {p}: {e}"),
            }
        }
        users.sort_by_key(|u| u.uid);
        Ok(users)
    }

    async fn call(&self, req: &Request) -> zbus::Result<Reply> {
        let flags = MethodFlags::AllowInteractiveAuth.into();
        let proxy = match req.target {
            Target::Manager => self.proxy(ACCOUNTS_PATH, ACCOUNTS_IFACE).await?,
            Target::User(uid) => {
                let path = self.user_path(uid).await?;
                self.proxy(path.as_str(), USER_IFACE).await?
            }
        };
        match &req.arg {
            Arg::CreateUser(name, real, kind) => {
                let path: Option<OwnedObjectPath> = proxy.call_with_flags(req.method, flags, &(name.as_str(), real.as_str(), *kind)).await?;
                let path = path.ok_or_else(|| zbus::Error::Failure("CreateUser returned no path".into()))?;
                Ok(Reply::Created(self.read_user(path.as_str()).await?.uid))
            }
            Arg::DeleteUser(uid, rm) => proxy.call_with_flags::<_, _, ()>(req.method, flags, &(*uid, *rm)).await.map(|_| Reply::Done),
            Arg::Str(s) => proxy.call_with_flags::<_, _, ()>(req.method, flags, &(s.as_str(),)).await.map(|_| Reply::Done),
            Arg::Int(i) => proxy.call_with_flags::<_, _, ()>(req.method, flags, &(*i,)).await.map(|_| Reply::Done),
            Arg::Bool(b) => proxy.call_with_flags::<_, _, ()>(req.method, flags, &(*b,)).await.map(|_| Reply::Done),
            Arg::StrStr(a, b) => proxy.call_with_flags::<_, _, ()>(req.method, flags, &(a.as_str(), b.as_str())).await.map(|_| Reply::Done),
        }
    }
}

impl AccountsBackend for DbusAccounts {
    fn list_users(&self) -> Result<Vec<UserAccount>> {
        crate::runtime::block_on(self.list()).map_err(AccountsError::from_zbus)
    }

    fn system_accounts(&self) -> Vec<UserAccount> {
        let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        let uid_min = uid_min(&std::fs::read_to_string("/etc/login.defs").unwrap_or_default());
        system_accounts_from_passwd(&passwd, uid_min)
    }

    fn taken_names(&self) -> Vec<String> {
        let mut names = names_in(&std::fs::read_to_string("/etc/passwd").unwrap_or_default());
        names.extend(names_in(&std::fs::read_to_string("/etc/group").unwrap_or_default()));
        names.sort();
        names.dedup();
        names
    }

    fn execute(&self, request: &Request) -> Result<Reply> {
        crate::runtime::block_on(self.call(request)).map_err(AccountsError::from_zbus)
    }

    fn current_uid(&self) -> u64 {
        self.uid
    }
}

pub fn current_uid() -> u64 {
    u64::from(rustix::process::getuid().as_raw())
}

fn prop<'a, T>(props: &'a HashMap<String, OwnedValue>, key: &str) -> Option<T>
where
    T: TryFrom<&'a zbus::zvariant::Value<'a>>,
    <T as TryFrom<&'a zbus::zvariant::Value<'a>>>::Error: Into<zbus::zvariant::Error>,
{
    props.get(key).and_then(|v| v.downcast_ref::<T>().ok())
}

fn prop_string(props: &HashMap<String, OwnedValue>, key: &str) -> String {
    prop::<&str>(props, key).unwrap_or_default().to_string()
}

/// Builds a [`UserAccount`] from `GetAll(org.freedesktop.Accounts.User)`.
pub fn parse_user(props: &HashMap<String, OwnedValue>) -> UserAccount {
    let icon = prop_string(props, "IconFile");
    UserAccount {
        uid: prop::<u64>(props, "Uid").unwrap_or_default(),
        user_name: prop_string(props, "UserName"),
        real_name: prop_string(props, "RealName"),
        account_type: AccountType::from_code(prop::<i32>(props, "AccountType").unwrap_or_default()),
        icon_file: (!icon.is_empty()).then(|| PathBuf::from(icon)),
        email: prop_string(props, "Email"),
        language: prop_string(props, "Language"),
        locked: prop::<bool>(props, "Locked").unwrap_or_default(),
        automatic_login: prop::<bool>(props, "AutomaticLogin").unwrap_or_default(),
        password_mode: PasswordMode::from_code(prop::<i32>(props, "PasswordMode").unwrap_or_default()),
        login_time: prop::<i64>(props, "LoginTime").unwrap_or_default(),
        home_directory: PathBuf::from(prop_string(props, "HomeDirectory")),
        shell: prop_string(props, "Shell"),
        system_account: prop::<bool>(props, "SystemAccount").unwrap_or_default(),
    }
}

/// `UID_MIN` from `/etc/login.defs`.
pub fn uid_min(login_defs: &str) -> u64 {
    login_defs
        .lines()
        .filter_map(|l| l.trim().strip_prefix("UID_MIN"))
        .find_map(|rest| rest.trim().parse().ok())
        .unwrap_or(DEFAULT_UID_MIN)
}

fn names_in(table: &str) -> Vec<String> {
    table.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split(':').next()).filter(|n| !n.is_empty()).map(str::to_string).collect()
}

/// Service accounts from `/etc/passwd` (uid below `uid_min`, or `nobody`).
pub fn system_accounts_from_passwd(passwd: &str, uid_min: u64) -> Vec<UserAccount> {
    passwd
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|line| {
            let f: Vec<&str> = line.split(':').collect();
            if f.len() < 7 {
                return None;
            }
            let uid: u64 = f[2].parse().ok()?;
            if uid >= uid_min && uid != NOBODY_UID {
                return None;
            }
            let gecos = f[4].split(',').next().unwrap_or("").trim();
            Some(UserAccount {
                uid,
                user_name: f[0].to_string(),
                real_name: gecos.to_string(),
                account_type: if uid == 0 { AccountType::Administrator } else { AccountType::Standard },
                home_directory: PathBuf::from(f[5]),
                shell: f[6].to_string(),
                system_account: true,
                ..Default::default()
            })
        })
        .collect()
}

// ───────────────────────────── watching ─────────────────────────────

/// Calls `on_change` whenever AccountsService reports `UserAdded`,
/// `UserDeleted`, a user's `Changed` or a property change; stops once
/// `on_change` returns `false` (checked on the next signal).
pub fn watch(on_change: impl Fn() -> bool + Send + 'static) {
    crate::runtime::spawn("accounts-watch", async move {
        let result: zbus::Result<()> = async move {
            // Private connection, closed when the watcher stops.
            let conn = crate::runtime::connect(zbus::connection::Builder::system()?).await?;
            let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).sender(ACCOUNTS_DEST)?.build();
            let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;
            while let Some(msg) = stream.next().await {
                let Ok(msg) = msg else { continue };
                let relevant = msg.header().member().is_some_and(|m| matches!(m.as_str(), "UserAdded" | "UserDeleted" | "Changed" | "PropertiesChanged"));
                if relevant && !on_change() {
                    break;
                }
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            tracing::debug!("accounts: watcher stopped: {e}");
        }
    });
}

// ───────────────────────────── avatar lookup ─────────────────────────────

/// The user's AccountsService `IconFile`, when set and readable — the same
/// photo GNOME / KDE / Studio show. A short, read-only system-bus query;
/// `None` when the daemon isn't running (callers fall back to `~/.face`).
pub fn icon_file_for(user_name: &str) -> Option<PathBuf> {
    let icon = match crate::runtime::block_on(icon_file_lookup(user_name)) {
        Ok(icon) => icon,
        Err(e) => {
            tracing::debug!("accounts: IconFile of {user_name}: {e}");
            return None;
        }
    };
    let path = PathBuf::from(icon);
    (!path.as_os_str().is_empty() && std::fs::File::open(&path).is_ok()).then_some(path)
}

async fn icon_file_lookup(user_name: &str) -> zbus::Result<String> {
    let conn = crate::runtime::connect(zbus::connection::Builder::system()?.method_timeout(LOOKUP_TIMEOUT)).await?;
    let manager = zbus::proxy::Builder::<zbus::Proxy>::new(&conn)
        .destination(ACCOUNTS_DEST)?
        .path(ACCOUNTS_PATH)?
        .interface(ACCOUNTS_IFACE)?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;
    let path: OwnedObjectPath = manager.call("FindUserByName", &(user_name,)).await?;
    let props = zbus::proxy::Builder::<zbus::Proxy>::new(&conn)
        .destination(ACCOUNTS_DEST)?
        .path(path)?
        .interface(PROPERTIES_IFACE)?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;
    let value: OwnedValue = props.call("Get", &(USER_IFACE, "IconFile")).await?;
    Ok(value.downcast_ref::<&str>().map(str::to_string).or_else(|_| value.downcast_ref::<String>()).unwrap_or_default())
}

/// Avatar candidates in order: AccountsService's `IconFile`, its icon
/// directory, then `~/.face` / `~/.face.icon`.
pub fn avatar_candidates(user_name: &str, home: &Path) -> Vec<PathBuf> {
    let mut list = Vec::with_capacity(4);
    if let Some(p) = icon_file_for(user_name) {
        list.push(p);
    }
    let cached = Path::new(ICON_DIR).join(user_name);
    if !list.contains(&cached) {
        list.push(cached);
    }
    list.push(home.join(".face"));
    list.push(home.join(".face.icon"));
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use mock::MockAccounts;
    use zbus::zvariant::Value;

    fn ov(v: Value<'_>) -> OwnedValue {
        v.try_into().unwrap()
    }

    #[test]
    fn parses_accountsservice_properties() {
        let mut props = HashMap::new();
        props.insert("Uid".to_string(), ov(Value::U64(1001)));
        props.insert("UserName".to_string(), ov(Value::from("nour")));
        props.insert("RealName".to_string(), ov(Value::from("Nour El-Sayed")));
        props.insert("AccountType".to_string(), ov(Value::I32(1)));
        props.insert("IconFile".to_string(), ov(Value::from("/var/lib/AccountsService/icons/nour")));
        props.insert("Locked".to_string(), ov(Value::Bool(true)));
        props.insert("PasswordMode".to_string(), ov(Value::I32(1)));
        props.insert("LoginTime".to_string(), ov(Value::I64(1_792_739_638)));
        props.insert("SystemAccount".to_string(), ov(Value::Bool(false)));
        let u = parse_user(&props);
        assert_eq!(u.uid, 1001);
        assert_eq!(u.user_name, "nour");
        assert!(u.is_admin());
        assert!(u.locked);
        assert_eq!(u.password_mode, PasswordMode::SetAtLogin);
        assert_eq!(u.icon_file.as_deref(), Some(Path::new("/var/lib/AccountsService/icons/nour")));
        assert_eq!(u.login_time, 1_792_739_638);
        assert_eq!(u.initials(), "NE");
        // Empty IconFile → no photo.
        props.insert("IconFile".to_string(), ov(Value::from("")));
        assert_eq!(parse_user(&props).icon_file, None);
    }

    #[test]
    fn builds_requests() {
        assert_eq!(UserChange::RealName("  Mariam Fahmy ".into()).request(1002), Request { target: Target::User(1002), method: "SetRealName", arg: Arg::Str("Mariam Fahmy".into()) });
        assert_eq!(UserChange::AccountType(AccountType::Administrator).request(7).arg, Arg::Int(1));
        assert_eq!(UserChange::PasswordMode(PasswordMode::SetAtLogin).request(7).arg, Arg::Int(1));
        assert_eq!(UserChange::Locked(true).request(7).method, "SetLocked");
        assert_eq!(UserChange::AutomaticLogin(false).request(7).method, "SetAutomaticLogin");
        assert_eq!(UserChange::IconFile(PathBuf::from("/tmp/a.png")).request(7).arg, Arg::Str("/tmp/a.png".into()));
        assert_eq!(Request::create_user("youssef", "Youssef", AccountType::Standard).arg, Arg::CreateUser("youssef".into(), "Youssef".into(), 0));
        assert_eq!(Request::delete_user(1003, true), Request { target: Target::Manager, method: "DeleteUser", arg: Arg::DeleteUser(1003, true) });
    }

    #[test]
    fn password_hash_is_redacted_in_debug() {
        let change = UserChange::Password { hash: "$6$salt$secret".into(), hint: String::new() };
        assert!(!format!("{change:?}").contains("secret"));
        assert!(!format!("{:?}", change.request(1)).contains("secret"));
    }

    #[test]
    fn classifies_errors() {
        assert_eq!(classify_error_name("org.freedesktop.Accounts.Error.PermissionDenied"), ErrorKind::NotAuthorized);
        assert_eq!(classify_error_name("org.freedesktop.DBus.Error.ServiceUnknown"), ErrorKind::Unavailable);
        assert_eq!(classify_error_name("org.freedesktop.Accounts.Error.Failed"), ErrorKind::Failed);
    }

    #[test]
    fn create_account_hashes_the_password() {
        let mock = MockAccounts::demo();
        let new = NewUser { user_name: "salma".into(), real_name: "Salma Adel".into(), account_type: AccountType::Standard, password: NewPassword::Now("correct horse battery".into()), language: "ar_EG.UTF-8".into() };
        let uid = create_account(&mock, &new).unwrap();
        let log = mock.log();
        assert_eq!(log[0].method, "CreateUser");
        let Arg::StrStr(hash, _) = &log[1].arg else { panic!("expected SetPassword, got {:?}", log[1]) };
        assert_eq!(log[1].method, "SetPassword");
        assert!(hash.starts_with("$6$") && !hash.contains("correct horse"));
        assert!(crypt::verify("correct horse battery", hash));
        assert_eq!(log[2], UserChange::Language("ar_EG.UTF-8".into()).request(uid));
        let user = mock.list_users().unwrap().into_iter().find(|u| u.uid == uid).unwrap();
        assert_eq!(user.real_name, "Salma Adel");
        assert_eq!(user.language, "ar_EG.UTF-8");
    }

    #[test]
    fn create_account_at_first_login() {
        let mock = MockAccounts::demo();
        let new = NewUser { user_name: "omar".into(), real_name: "Omar".into(), account_type: AccountType::Administrator, password: NewPassword::AtFirstLogin, language: String::new() };
        let uid = create_account(&mock, &new).unwrap();
        let log = mock.log();
        assert_eq!(log.len(), 2, "no SetLanguage for the system default");
        assert_eq!(log[1], UserChange::PasswordMode(PasswordMode::SetAtLogin).request(uid));
        let user = mock.list_users().unwrap().into_iter().find(|u| u.uid == uid).unwrap();
        assert!(user.is_admin());
        assert_eq!(user.password_mode, PasswordMode::SetAtLogin);
    }

    #[test]
    fn create_account_rejects_bad_or_taken_names() {
        let mock = MockAccounts::demo();
        for bad in ["", "Nour", "9lives", "root", "nour", "a b"] {
            let new = NewUser { user_name: bad.into(), real_name: "X".into(), account_type: AccountType::Standard, password: NewPassword::AtFirstLogin, language: String::new() };
            assert!(create_account(&mock, &new).is_err(), "{bad:?} must be rejected");
        }
        assert!(mock.log().is_empty(), "nothing reaches the backend");
    }

    #[test]
    fn guards_protect_self_and_last_admin() {
        let mock = MockAccounts::demo();
        let users = mock.list_users().unwrap();
        let me = users.iter().find(|u| u.uid == mock.current_uid()).unwrap();
        assert_eq!(delete_guard(me, mock.current_uid(), &users), Err(Guard::IsYou));
        let standard = users.iter().find(|u| !u.is_admin()).unwrap();
        assert_eq!(delete_guard(standard, mock.current_uid(), &users), Ok(()));
        // Two admins: either may be demoted / deleted (if not you)…
        let other_admin = users.iter().find(|u| u.is_admin() && u.uid != me.uid).unwrap();
        assert_eq!(delete_guard(other_admin, mock.current_uid(), &users), Ok(()));
        assert_eq!(demote_guard(other_admin, &users), Ok(()));
        // …but never the last one.
        let only: Vec<UserAccount> = users.iter().filter(|u| u.uid != other_admin.uid).cloned().collect();
        assert_eq!(demote_guard(me, &only), Err(Guard::LastAdmin));
        assert_eq!(delete_guard(me, 0, &only), Err(Guard::LastAdmin));
        let sys = UserAccount { system_account: true, ..Default::default() };
        assert_eq!(delete_guard(&sys, 1, &users), Err(Guard::SystemAccount));
    }

    #[test]
    fn mock_applies_changes_and_deletes() {
        let mock = MockAccounts::demo();
        let target = mock.list_users().unwrap().into_iter().find(|u| !u.is_admin()).unwrap();
        mock.execute(&UserChange::Email("y@example.org".into()).request(target.uid)).unwrap();
        mock.execute(&UserChange::Locked(true).request(target.uid)).unwrap();
        let after = mock.list_users().unwrap().into_iter().find(|u| u.uid == target.uid).unwrap();
        assert_eq!(after.email, "y@example.org");
        assert!(after.locked);
        mock.execute(&Request::delete_user(target.uid, false)).unwrap();
        assert!(mock.list_users().unwrap().iter().all(|u| u.uid != target.uid));
        assert_eq!(mock.execute(&UserChange::Locked(false).request(target.uid)).unwrap_err().kind, ErrorKind::NoSuchUser);
    }

    #[test]
    fn change_password_sends_a_hash() {
        let mock = MockAccounts::demo();
        change_password(&mock, 1000, "s3cret-Pass", "").unwrap();
        let Arg::StrStr(hash, hint) = &mock.log()[0].arg else { panic!() };
        assert!(crypt::verify("s3cret-Pass", hash));
        assert!(hint.is_empty());
        assert!(change_password(&mock, 1000, "", "").is_err());
    }

    #[test]
    fn reads_system_accounts_and_uid_min() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\nbin:x:1:1::/:/usr/bin/nologin\nnour:x:1000:1000:Nour:/home/nour:/bin/bash\nnobody:x:65534:65534:Kernel Overflow User:/:/usr/bin/nologin\n";
        let sys = system_accounts_from_passwd(passwd, 1000);
        let names: Vec<&str> = sys.iter().map(|u| u.user_name.as_str()).collect();
        assert_eq!(names, ["root", "bin", "nobody"]);
        assert!(sys.iter().all(|u| u.system_account));
        assert_eq!(uid_min("# comment\nUID_MIN\t\t\t 1000\nUID_MAX 60000\n"), 1000);
        assert_eq!(uid_min("UID_MIN 500"), 500);
        assert_eq!(uid_min(""), DEFAULT_UID_MIN);
        assert_eq!(names_in(passwd), ["root", "bin", "nour", "nobody"]);
    }

    #[test]
    fn initials_of_names() {
        assert_eq!(initials("Nour El-Sayed"), "NE");
        assert_eq!(initials("youssef"), "Y");
        assert_eq!(initials("مريم فهمي"), "مف");
        assert_eq!(initials(""), "?");
    }

    /// Read-only: lists users from the live AccountsService when it runs.
    #[test]
    #[ignore = "needs the system bus"]
    fn lists_live_users_read_only() {
        let backend = DbusAccounts::connect().unwrap();
        let users = backend.list_users().unwrap();
        assert!(!users.is_empty());
        let me = users.iter().find(|u| u.uid == backend.current_uid()).unwrap();
        println!("{} ({}) icon={:?}", me.display_name(), me.user_name, me.icon_file);
        println!("{:?}", crate::runtime::block_on(icon_file_lookup(&me.user_name)));
        assert_eq!(icon_file_for(&me.user_name), me.photo().map(Path::to_path_buf));
    }
}
