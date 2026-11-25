//! An in-memory [`AccountsBackend`]: Studio's `--demo` mode and the unit
//! tests. It applies requests to fictional users and records every call,
//! so nothing on the real system changes.

use std::path::PathBuf;

use parking_lot::Mutex;

use super::{AccountType, AccountsBackend, AccountsError, Arg, ErrorKind, PasswordMode, Reply, Request, Result, Target, UserAccount};

/// The demo's signed-in user.
pub const DEMO_CURRENT_UID: u64 = 1000;
const HOUR: i64 = 3_600;
const DAY: i64 = 24 * HOUR;

pub struct MockAccounts {
    users: Mutex<Vec<UserAccount>>,
    system: Vec<UserAccount>,
    log: Mutex<Vec<Request>>,
    current: u64,
    /// Report every mutation as refused by polkit.
    deny: bool,
}

fn user(uid: u64, name: &str, real: &str, kind: AccountType, language: &str, login_ago: Option<i64>) -> UserAccount {
    UserAccount {
        uid,
        user_name: name.into(),
        real_name: real.into(),
        account_type: kind,
        language: language.into(),
        login_time: login_ago.map(|ago| now() - ago).unwrap_or(0),
        home_directory: PathBuf::from(format!("/home/{name}")),
        shell: "/bin/bash".into(),
        ..Default::default()
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or_default()
}

impl MockAccounts {
    /// An empty backend whose signed-in user is `current`.
    pub fn new(users: Vec<UserAccount>, current: u64) -> Self {
        MockAccounts { users: Mutex::new(users), system: Vec::new(), log: Mutex::new(Vec::new()), current, deny: false }
    }

    /// A small household of fictional users plus a few service accounts.
    pub fn demo() -> Self {
        let mut nour = user(DEMO_CURRENT_UID, "nour", "Nour El-Sayed", AccountType::Administrator, "ar_EG.UTF-8", Some(2 * HOUR));
        nour.email = "nour@mandara.example".into();
        let mariam = user(1001, "mariam", "Mariam Fahmy", AccountType::Administrator, "en_US.UTF-8", Some(3 * DAY));
        let mut youssef = user(1002, "youssef", "Youssef Mansour", AccountType::Standard, "fr_FR.UTF-8", Some(26 * HOUR));
        youssef.automatic_login = false;
        let mut lina = user(1003, "lina", "Lina Haddad", AccountType::Standard, "", None);
        lina.password_mode = PasswordMode::SetAtLogin;
        let mut guest = user(1004, "visitor", "", AccountType::Standard, "", Some(40 * DAY));
        guest.locked = true;
        let system = [(0, "root", "root"), (81, "dbus", "System Message Bus"), (102, "polkitd", "PolicyKit daemon"), (964, "greeter", "greetd greeter user"), (65534, "nobody", "Kernel Overflow User")]
            .into_iter()
            .map(|(uid, name, real)| UserAccount {
                uid,
                user_name: name.into(),
                real_name: real.into(),
                account_type: if uid == 0 { AccountType::Administrator } else { AccountType::Standard },
                home_directory: PathBuf::from(if uid == 0 { "/root" } else { "/" }),
                shell: "/usr/bin/nologin".into(),
                system_account: true,
                ..Default::default()
            })
            .collect();
        MockAccounts { system, ..Self::new(vec![nour, mariam, youssef, lina, guest], DEMO_CURRENT_UID) }
    }

    /// Every mutation fails as if polkit refused it.
    pub fn denying(mut self) -> Self {
        self.deny = true;
        self
    }

    /// The requests executed so far.
    pub fn log(&self) -> Vec<Request> {
        self.log.lock().clone()
    }

    fn next_uid(users: &[UserAccount]) -> u64 {
        users.iter().map(|u| u.uid + 1).max().unwrap_or(DEMO_CURRENT_UID).max(DEMO_CURRENT_UID)
    }
}

fn no_such_user(uid: u64) -> AccountsError {
    AccountsError::new(ErrorKind::NoSuchUser, format!("no user with uid {uid}"))
}

impl AccountsBackend for MockAccounts {
    fn list_users(&self) -> Result<Vec<UserAccount>> {
        Ok(self.users.lock().clone())
    }

    fn system_accounts(&self) -> Vec<UserAccount> {
        self.system.clone()
    }

    fn taken_names(&self) -> Vec<String> {
        self.users.lock().iter().chain(self.system.iter()).map(|u| u.user_name.clone()).collect()
    }

    fn execute(&self, request: &Request) -> Result<Reply> {
        if self.deny {
            return Err(AccountsError::new(ErrorKind::NotAuthorized, "Not authorized"));
        }
        let mut users = self.users.lock();
        let reply = match (&request.target, &request.arg) {
            (Target::Manager, Arg::CreateUser(name, real, kind)) => {
                if users.iter().chain(self.system.iter()).any(|u| &u.user_name == name) {
                    return Err(AccountsError::new(ErrorKind::Failed, format!("user {name} already exists")));
                }
                let uid = Self::next_uid(&users);
                users.push(user(uid, name, real, AccountType::from_code(*kind), "", None));
                Reply::Created(uid)
            }
            (Target::Manager, Arg::DeleteUser(uid, _)) => {
                let uid = *uid as u64;
                let before = users.len();
                users.retain(|u| u.uid != uid);
                if users.len() == before {
                    return Err(no_such_user(uid));
                }
                Reply::Done
            }
            (Target::User(uid), _) => {
                let target = users.iter_mut().find(|u| u.uid == *uid).ok_or_else(|| no_such_user(*uid))?;
                apply_request(target, request);
                Reply::Done
            }
            _ => return Err(AccountsError::new(ErrorKind::Failed, format!("unsupported request {}", request.method))),
        };
        drop(users);
        self.log.lock().push(request.clone());
        Ok(reply)
    }

    fn current_uid(&self) -> u64 {
        self.current
    }

    fn is_demo(&self) -> bool {
        true
    }
}

/// Applies a `Set*` request the way accounts-daemon would.
fn apply_request(user: &mut UserAccount, request: &Request) {
    match (request.method, &request.arg) {
        ("SetRealName", Arg::Str(s)) => user.real_name = s.clone(),
        ("SetEmail", Arg::Str(s)) => user.email = s.clone(),
        ("SetLanguage", Arg::Str(s)) => user.language = s.clone(),
        ("SetIconFile", Arg::Str(s)) => user.icon_file = (!s.is_empty()).then(|| PathBuf::from(s)),
        ("SetAccountType", Arg::Int(i)) => user.account_type = AccountType::from_code(*i),
        ("SetPasswordMode", Arg::Int(i)) => user.password_mode = PasswordMode::from_code(*i),
        ("SetLocked", Arg::Bool(b)) => user.locked = *b,
        ("SetAutomaticLogin", Arg::Bool(b)) => user.automatic_login = *b,
        ("SetPassword", Arg::StrStr(..)) => user.password_mode = PasswordMode::Regular,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::UserChange;

    #[test]
    fn demo_has_two_admins_and_system_accounts() {
        let m = MockAccounts::demo();
        let users = m.list_users().unwrap();
        assert_eq!(users.iter().filter(|u| u.is_admin()).count(), 2);
        assert!(users.iter().any(|u| u.uid == m.current_uid()));
        assert!(m.system_accounts().iter().all(|u| u.system_account));
        assert!(m.taken_names().contains(&"root".to_string()));
    }

    #[test]
    fn denying_backend_refuses_without_changes() {
        let m = MockAccounts::demo().denying();
        let err = m.execute(&UserChange::Locked(true).request(1002)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotAuthorized);
        assert!(m.log().is_empty());
        assert!(!m.list_users().unwrap().iter().find(|u| u.uid == 1002).unwrap().locked);
    }

    #[test]
    fn creating_a_duplicate_fails() {
        let m = MockAccounts::demo();
        assert!(m.execute(&Request::create_user("nour", "x", AccountType::Standard)).is_err());
        assert!(m.execute(&Request::create_user("dbus", "x", AccountType::Standard)).is_err());
        assert_eq!(m.execute(&Request::create_user("salma", "Salma", AccountType::Standard)).unwrap(), Reply::Created(1005));
    }
}
