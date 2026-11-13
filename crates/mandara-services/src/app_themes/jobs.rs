//! Catalog network work is explicit, cancellable between requests and serialized.

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use mandara_config::app_themes::CatalogSource;

use super::catalog::{self, Entry, Http, Installed, Transport};

static NETWORK: async_lock::Mutex<()> = async_lock::Mutex::new(());
static REQUESTS: AtomicUsize = AtomicUsize::new(0);
/// One transfer runs at a time; at most eight callers retain request metadata.
const MAX_REQUESTS: usize = 8;
struct Admission<'a>(&'a AtomicUsize);
impl<'a> Admission<'a> {
    fn acquire(count: &'a AtomicUsize) -> Option<Self> {
        count
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < MAX_REQUESTS).then_some(current + 1)
            })
            .ok()
            .map(|_| Self(count))
    }
}
impl Drop for Admission<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Fetch {
    http: Http,
    cancel: Arc<AtomicBool>,
}
impl Transport for Fetch {
    fn get(&self, url: &str, limit: u64) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(
            !self.cancel.load(Ordering::Acquire),
            "catalog request was cancelled"
        );
        self.http.get(url, limit)
    }
}

#[derive(Default)]
pub struct Discovery {
    pub entries: Vec<Entry>,
    pub errors: Vec<String>,
}

pub async fn discover(
    sources: Vec<CatalogSource>,
    root: PathBuf,
    refresh: bool,
    cancel: Arc<AtomicBool>,
) -> Discovery {
    let Some(admission) = Admission::acquire(&REQUESTS) else {
        return Discovery {
            entries: Vec::new(),
            errors: vec!["app template downloads are busy; try again in a moment".into()],
        };
    };
    let lease = NETWORK.lock().await;
    crate::runtime::unblock(move || {
        let _admission = admission;
        let _lease = lease;
        let fetch = Fetch {
            http: Http::default(),
            cancel,
        };
        let mut result = Discovery::default();
        for source in sources {
            if fetch.cancel.load(Ordering::Acquire) {
                break;
            }
            match catalog::list_with(&source, &root, refresh, &fetch) {
                Ok(entries) => result.entries.extend(entries),
                Err(error) => result.errors.push(format!("{}: {error:#}", source.name)),
            }
        }
        result
    })
    .await
}

pub async fn install(
    source: CatalogSource,
    entry: Entry,
    root: PathBuf,
    cancel: Arc<AtomicBool>,
) -> anyhow::Result<Installed> {
    let admission = Admission::acquire(&REQUESTS)
        .ok_or_else(|| anyhow::anyhow!("app template downloads are busy; try again in a moment"))?;
    let lease = NETWORK.lock().await;
    crate::runtime::unblock(move || {
        let _lease = lease;
        let _admission = admission;
        let installed = catalog::install_with(
            &source,
            &entry,
            &root,
            &Fetch {
                http: Http::default(),
                cancel,
            },
        )?;
        let protected = super::apply::protected_revisions(&super::paths::Locations::current()?)?;
        catalog::prune(&root, &entry.qualified_id(), &protected)?;
        Ok(installed)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_capacity_is_returned_on_cancellation_and_completion() {
        let count = AtomicUsize::new(0);
        let mut requests: Vec<_> = (0..MAX_REQUESTS)
            .map(|_| Admission::acquire(&count).unwrap())
            .collect();
        assert!(Admission::acquire(&count).is_none());
        requests.pop();
        let last = Admission::acquire(&count).unwrap();
        assert_eq!(count.load(Ordering::Acquire), MAX_REQUESTS);
        drop(last);
        drop(requests);
        assert_eq!(count.load(Ordering::Acquire), 0);
    }
}
