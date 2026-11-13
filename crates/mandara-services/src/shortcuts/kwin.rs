//! KWin changes go through KGlobalAccel, with checked persistence and undo.

use crate::runtime::block_on;
use anyhow::{Context as _, Result, ensure};
use mandara_config::{
    doctor::{
        self,
        theme::{self, ServiceEditor},
    },
    keybinds::{self, Binding, PreparedEdit},
};
use mandara_i18n::t;
use mandara_theme::templates::WriteLock;
use zbus::{Connection, Proxy, connection::Builder};

use super::{Preferences, Saved, Session, Undo, transaction};

type Sequences = Vec<(Vec<i32>,)>;

fn normalized(values: Sequences) -> Result<Vec<[i32; 4]>> {
    ensure!(
        values.len() <= keybinds::MAX_FIELDS,
        "too many KWin shortcut alternatives"
    );
    let mut result = Vec::with_capacity(values.len());
    for (value,) in values {
        ensure!(value.len() <= 4, "invalid Qt shortcut sequence");
        let mut keys = [0; 4];
        keys[..value.len()].copy_from_slice(&value);
        if keys != [0; 4] {
            result.push(keys);
        }
    }
    result.sort_unstable();
    result.dedup();
    Ok(result)
}

fn keys(row: &Binding) -> Result<Vec<[i32; 4]>> {
    let mut result = row
        .fields
        .iter()
        .map(|field| keybinds::qt_sequence(&field.value))
        .collect::<Result<Vec<_>>>()?;
    result.retain(|keys| *keys != [0; 4]);
    result.sort_unstable();
    result.dedup();
    Ok(result)
}

struct Api {
    connection: Connection,
    owner: String,
    action: Vec<String>,
}
impl Api {
    fn open(row: &Binding) -> Result<Self> {
        let (group, name) = row
            .scope
            .split_once('/')
            .context("invalid KWin shortcut identity")?;
        let group = group
            .strip_prefix("[kwin]")
            .context("invalid KWin component")?;
        let component = if group.is_empty() {
            "kwin".into()
        } else {
            let context = group
                .strip_prefix('[')
                .and_then(|group| group.strip_suffix(']'))
                .context("invalid KWin context")?;
            ensure!(
                !context.contains(['[', ']', '|']),
                "unsupported nested KWin context"
            );
            format!("kwin|{context}")
        };
        block_on(async {
            let connection =
                crate::runtime::connect(Builder::session()?.method_timeout(doctor::CHECK_TIMEOUT))
                    .await?;
            let bus = Proxy::new(
                &connection,
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
            )
            .await?;
            let owner: String = bus.call("GetNameOwner", &("org.kde.kglobalaccel",)).await?;
            let proxy = Proxy::new(
                &connection,
                owner.as_str(),
                "/kglobalaccel",
                "org.kde.KGlobalAccel",
            )
            .await?;
            let actions: Vec<Vec<String>> = proxy
                .call(
                    "allActionsForComponent",
                    &([component.as_str(), "", "", ""],),
                )
                .await?;
            ensure!(
                actions.len() <= keybinds::MAX_BINDINGS,
                "too many KWin actions"
            );
            let action = actions
            .into_iter()
            .find(|action| action.len() == 4 && action[0] == component && action[1] == name)
            .context(t(
                "The shortcut is no longer registered with KWin. Refresh the list and try again.",
            ))?;
            Ok(Self {
                connection,
                owner,
                action,
            })
        })
    }
    fn proxy(&self) -> Result<Proxy<'_>> {
        Ok(block_on(Proxy::new(
            &self.connection,
            self.owner.as_str(),
            "/kglobalaccel",
            "org.kde.KGlobalAccel",
        ))?)
    }
    fn get(&self) -> Result<Vec<[i32; 4]>> {
        normalized(block_on(
            self.proxy()?.call("shortcutKeys", &(&self.action,)),
        )?)
    }
    fn set(&self, keys: &[[i32; 4]]) -> Result<()> {
        let values: Sequences = keys.iter().map(|keys| (keys.to_vec(),)).collect();
        block_on(
            self.proxy()?
                .call::<_, _, ()>("setForeignShortcutKeys", &(&self.action, values)),
        )?;
        Ok(())
    }
    fn available(&self, keys: &[[i32; 4]]) -> Result<()> {
        let proxy = self.proxy()?;
        for key in keys {
            let available: bool =
                block_on(proxy.call("globalShortcutAvailable", &((key.to_vec(),), "")))?;
            ensure!(
                available,
                "{}",
                t("Another application already uses this shortcut. Choose a different key.")
            );
        }
        Ok(())
    }
}

struct Change<'a> {
    api: Api,
    before: Vec<[i32; 4]>,
    after: Vec<[i32; 4]>,
    edit: &'a PreparedEdit,
    guard: transaction::DocumentGuard<'a>,
}
impl ServiceEditor for Change<'_> {
    fn verify(&self, after: bool) -> Result<()> {
        self.guard.fresh()?;
        ensure!(
            self.api.get()? == if after { &self.after } else { &self.before }.as_slice(),
            "{}",
            t("KWin's active shortcuts changed. Refresh the list and try again.")
        );
        if !after {
            let added: Vec<_> = self
                .after
                .iter()
                .filter(|key| !self.before.contains(key))
                .copied()
                .collect();
            self.api.available(&added)?;
        }
        Ok(())
    }
    fn apply(&self, after: bool) -> Result<()> {
        self.api.set(if after { &self.after } else { &self.before })
    }
    fn equivalent(&self, body: &str, after: bool) -> Result<bool> {
        keybinds::kwin_same_settings(
            body,
            if after {
                self.edit.body()
            } else {
                &self.edit.original
            },
        )
    }
}

pub(super) fn save(
    session: &Session,
    edit: &PreparedEdit,
    initial: &Preferences,
    preferences: &dyn Fn() -> Result<Preferences>,
    backend: &dyn transaction::Backend,
) -> Result<Saved> {
    let lock = WriteLock::new(&edit.id.file)?;
    ensure!(
        lock.target() == edit.id.target,
        "{}",
        t("KWin's active shortcuts changed. Refresh the list and try again.")
    );
    let change = Change {
        api: Api::open(&edit.old_binding)?,
        before: keys(&edit.old_binding)?,
        after: keys(edit.binding())?,
        edit,
        guard: transaction::DocumentGuard {
            session,
            destination: lock.target(),
            initial,
            preferences,
            backend,
        },
    };
    theme::guarded_service_write(&lock, &edit.original, edit.body(), &change)?;
    Ok(Saved {
        undo: edit.undo().map(Undo::Native),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dbus_sequences_are_structs_of_arrays_and_compare_independent_of_order_and_padding() {
        let wire: Sequences = vec![
            (vec![0x10000051],),
            (vec![0x08000073, 0, 0, 0],),
            (Vec::new(),),
        ];
        assert_eq!(
            <Sequences as zbus::zvariant::Type>::SIGNATURE.to_string(),
            "a(ai)"
        );
        assert_eq!(
            normalized(wire).unwrap(),
            [[0x08000073, 0, 0, 0], [0x10000051, 0, 0, 0]]
        );
        assert!(normalized(vec![(vec![1; 5],)]).is_err());
    }
}
