//! API keys belong in the desktop keyring. Config files hold the server
//! address, which also scopes each key so changing servers cannot reuse it.

use std::fmt;

use mandara_i18n::{t, tf};
use zeroize::Zeroizing;

/// Provider keys and JWTs are short. Reject a pasted document before it
/// reaches the keyring or an HTTP header.
const MAX_KEY_BYTES: usize = 4096;
const APPLICATION: &str = "mandara-shell";
const PURPOSE: &str = "plugin-api-key";
/// Replacement includes duplicate cleanup; writes must not delete one another.
static WRITES: async_lock::Mutex<()> = async_lock::Mutex::new(());

/// Hold the write gate through the entire operation, without blocking a worker.
async fn mutation(operation: impl std::future::Future<Output = Result<(), String>>) -> Result<(), String> {
    let _guard = WRITES.lock().await;
    operation.await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider { DeepL, LibreTranslate, HomeAssistant }

impl Provider {
    /// The provider name shown beside a saved key.
    pub fn name(self) -> &'static str {
        match self { Self::DeepL => "DeepL", Self::LibreTranslate => "LibreTranslate", Self::HomeAssistant => "Home Assistant" }
    }

    /// A stable keyring attribute; display names may change without moving keys.
    pub fn id(self) -> &'static str {
        match self {
            Self::DeepL => "deepl",
            Self::LibreTranslate => "libretranslate",
            Self::HomeAssistant => "home-assistant",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub provider: Provider,
    pub server: String,
}

impl Key {
    fn attributes(&self) -> [(&str, &str); 4] {
        [("application", APPLICATION), ("purpose", PURPOSE), ("provider", self.provider.id()), ("server", self.server.trim_end_matches('/'))]
    }
}

pub struct Secret(Zeroizing<String>);

impl Secret {
    /// Trim a pasted key and reject whitespace, control characters and oversized values.
    pub fn new(value: String) -> Result<Self, String> {
        let value = Zeroizing::new(value);
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.len() > MAX_KEY_BYTES || trimmed.chars().any(char::is_whitespace) || trimmed.chars().any(char::is_control) {
            return Err(t("Paste an API key without spaces or line breaks").into());
        }
        Ok(Self(Zeroizing::new(trimmed.to_owned())))
    }

    /// Only the request header or keyring write needs the clear value.
    pub fn expose(&self) -> &str { &self.0 }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("<redacted>") }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access { Quiet, Prompt }

pub trait Store {
    fn read(&self, key: &Key, access: Access) -> Result<Option<Secret>, String>;
    fn save(&self, key: &Key, secret: &Secret) -> Result<(), String>;
    fn remove(&self, key: &Key) -> Result<(), String>;
}

#[derive(Default)]
pub struct Keyring;

fn failed(error: impl fmt::Display) -> String {
    tf("Could not open the keyring: {error}", &[("error", &error)]).to_string()
}

/// Open the default collection, prompting to unlock only when explicitly allowed.
async fn collection(access: Access) -> Result<oo7::dbus::Collection, String> {
    let service = oo7::dbus::Service::new().await.map_err(failed)?;
    // Reading a plugin's settings must never create a new default keyring.
    let collection = service.with_alias(oo7::dbus::Service::DEFAULT_COLLECTION).await.map_err(failed)?
        .ok_or_else(|| t("There is no default keyring; create one in your keyring app").to_string())?;
    if collection.is_locked().await.map_err(failed)? {
        match access {
            Access::Quiet => return Err(t("The keyring is locked").into()),
            Access::Prompt => collection.unlock(None).await.map_err(failed)?,
        }
    }
    Ok(collection)
}

impl Store for Keyring {
    /// Read exactly one key for this provider and server without creating a collection.
    fn read(&self, key: &Key, access: Access) -> Result<Option<Secret>, String> {
        crate::runtime::block_on(async {
            let collection = collection(access).await?;
            let items = collection.search_items(&key.attributes()).await.map_err(failed)?;
            let Some(item) = items.first() else { return Ok(None) };
            if items.len() != 1 { return Err(t("More than one API key matches this server; save the key again").into()); }
            if item.is_locked().await.map_err(failed)? {
                match access {
                    Access::Quiet => return Err(t("The keyring is locked").into()),
                    Access::Prompt => item.unlock(None).await.map_err(failed)?,
                }
            }
            let value = item.secret().await.map_err(failed)?;
            let value = std::str::from_utf8(value.as_bytes()).map_err(|_| t("The saved API key is not text").to_string())?;
            Secret::new(value.to_owned()).map(Some)
        })
    }

    /// Replace the provider/server entry with the supplied secret.
    fn save(&self, key: &Key, secret: &Secret) -> Result<(), String> {
        crate::runtime::block_on(mutation(async {
            let collection = collection(Access::Prompt).await?;
            let label = format!("Mandara · {} · {}", key.provider.name(), key.server);
            let saved = collection.create_item(&label, &key.attributes(), oo7::Secret::text(secret.expose()), true, None).await.map_err(failed)?;
            // Secret Service replaces one matching item. Older duplicates
            // must be removed after the replacement is safely stored.
            for item in collection.search_items(&key.attributes()).await.map_err(failed)? {
                if item.path() != saved.path() { item.delete(None).await.map_err(failed)?; }
            }
            Ok(())
        }))
    }

    /// Delete only entries matching this provider and server.
    fn remove(&self, key: &Key) -> Result<(), String> {
        crate::runtime::block_on(mutation(async {
            let collection = collection(Access::Prompt).await?;
            for item in collection.search_items(&key.attributes()).await.map_err(failed)? {
                item.delete(None).await.map_err(failed)?;
            }
            Ok(())
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutations_wait_for_replacement_cleanup_and_release_after_errors() {
        crate::runtime::block_on(async {
            let events = std::cell::RefCell::new(Vec::new());
            let (release, wait) = async_channel::bounded(1);
            let mut first = Box::pin(mutation(async {
                events.borrow_mut().push("first stored");
                wait.recv().await.map_err(|e| e.to_string())?;
                events.borrow_mut().push("first cleaned");
                Ok(())
            }));
            let mut second = Box::pin(mutation(async {
                events.borrow_mut().push("second stored");
                events.borrow_mut().push("second cleaned");
                Ok(())
            }));
            assert!(futures::poll!(&mut first).is_pending());
            assert!(futures::poll!(&mut second).is_pending());
            assert_eq!(*events.borrow(), ["first stored"]);
            release.try_send(()).unwrap();
            first.await.unwrap();
            second.await.unwrap();
            assert_eq!(*events.borrow(), ["first stored", "first cleaned", "second stored", "second cleaned"]);
            assert!(mutation(async { Err("fixture failure".into()) }).await.is_err());
            mutation(async { Ok(()) }).await.unwrap();
        });
    }

    #[test]
    fn secret_validation_preserves_a_pasted_key_and_hides_debug_output() {
        let secret = Secret::new("  fixture-key:fx\n".into()).unwrap();
        assert_eq!(secret.expose(), "fixture-key:fx");
        assert_eq!(format!("{secret:?}"), "<redacted>");
        for value in [String::new(), "two words".into(), "one\ntwo".into(), "x\0y".into(), "x".repeat(MAX_KEY_BYTES + 1)] {
            assert!(Secret::new(value).is_err());
        }
    }

    #[test]
    fn credentials_are_scoped_to_the_plugin_and_server() {
        let key = Key { provider: Provider::HomeAssistant, server: "https://home.example/".into() };
        assert_eq!(key.attributes(), [("application", "mandara-shell"), ("purpose", "plugin-api-key"), ("provider", "home-assistant"), ("server", "https://home.example")]);
        let other = Key { provider: Provider::HomeAssistant, server: "https://another.example".into() };
        assert_ne!(key.attributes(), other.attributes());
        let provider = Key { provider: Provider::LibreTranslate, ..key.clone() };
        assert_ne!(key.attributes(), provider.attributes());
    }
}
