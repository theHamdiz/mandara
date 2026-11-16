//! Stable card identities across marketplace refreshes.

use std::sync::Arc;

use mandara_services::marketplace::{Listing, Results};

#[derive(Default)]
pub(super) struct Catalog {
    pub items: Arc<[Arc<Listing>]>,
    pub recent: Arc<[Arc<Listing>]>,
    pub total: u64,
}

impl Catalog {
    pub fn installed(
        &self,
        path: &std::path::Path,
        origins: &std::collections::HashMap<String, (std::path::PathBuf, String)>,
    ) -> Option<Arc<Listing>> {
        let (repo, _) = origins.iter().find(|(_, (dir, _))| dir == path)?;
        self.items.iter().find(|l| l.full_name == *repo).cloned()
    }
    /// Preserve existing positions and shared records; append new repositories.
    /// Missing records leave too, including newly moderated repositories.
    pub fn merge(&self, mut next: Results) -> Self {
        let mut items = Vec::with_capacity(next.items.len());
        for old in self.items.iter() {
            if let Some(i) = next.items.iter().position(|l| l.full_name == old.full_name) {
                let mut listing = next.items.remove(i);
                // A failed metadata fetch must not erase a known release's text.
                // Transfers and reused repository names cannot inherit it.
                if listing.plugin.is_none()
                    && old.id.is_some()
                    && old.id == listing.id
                    && old.owner.id.is_some()
                    && old.owner.id == listing.owner.id
                {
                    listing.plugin.clone_from(&old.plugin);
                }
                items.push(if **old == listing {
                    old.clone()
                } else {
                    Arc::new(listing)
                });
            }
        }
        items.extend(next.items.into_iter().map(Arc::new));
        let mut recent: Vec<_> = items.iter().filter(|l| !l.archived).cloned().collect();
        recent.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Self {
            items: items.into(),
            recent: recent.into(),
            total: next.total,
        }
    }

    pub fn truncated(&self) -> bool {
        (self.items.len() as u64) < self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_failures_keep_release_text_only_for_the_same_repository_and_publisher() {
        let mut original = listing("author/picker", "mandara-picker");
        original.id = Some(1);
        original.owner.id = Some(2);
        original.plugin = Some(
            toml::from_str(
                "id='picker'\nname='Real Picker'\ndescription='Published description'\n",
            )
            .unwrap(),
        );
        let catalog = Catalog::default().merge(Results {
            items: vec![original.clone()],
            total: 1,
        });
        let mut missing = original;
        missing.plugin = None;
        let refreshed = catalog.merge(Results {
            items: vec![missing.clone()],
            total: 1,
        });
        assert!(Arc::ptr_eq(&catalog.items[0], &refreshed.items[0]));
        assert_eq!(
            refreshed.items[0].plugin.as_ref().unwrap().name,
            "Real Picker"
        );
        missing.owner.id = Some(3);
        let transferred = catalog.merge(Results {
            items: vec![missing],
            total: 1,
        });
        assert!(transferred.items[0].plugin.is_none());
    }

    fn listing(repo: &str, name: &str) -> Listing {
        serde_json::from_value(serde_json::json!({"full_name":repo,"name":name,"html_url":"https://example.invalid","owner":{"login":"author"}})).unwrap()
    }

    #[test]
    fn refresh_keeps_card_identity_and_order_but_applies_removals_and_metadata_changes() {
        let catalog = Catalog::default().merge(Results {
            items: vec![
                listing("a/one", "One"),
                listing("a/two", "Two"),
                listing("a/blocked", "Hidden"),
            ],
            total: 3,
        });
        let next = catalog.merge(Results {
            items: vec![
                listing("a/new", "New"),
                listing("a/two", "Updated"),
                listing("a/one", "One"),
            ],
            total: 3,
        });
        assert_eq!(
            next.items
                .iter()
                .map(|l| l.full_name.as_str())
                .collect::<Vec<_>>(),
            ["a/one", "a/two", "a/new"]
        );
        assert!(Arc::ptr_eq(&catalog.items[0], &next.items[0]));
        assert!(!Arc::ptr_eq(&catalog.items[1], &next.items[1]));
        assert_eq!(next.items[1].name, "Updated");
        assert!(!next.items.iter().any(|l| l.full_name == "a/blocked"));
    }

    #[test]
    fn installation_reuses_the_listing_only_for_its_verified_origin_folder() {
        let catalog = Catalog::default().merge(Results {
            items: vec![listing("a/picker", "Picker")],
            total: 1,
        });
        let path = std::path::Path::new("/plugins/picker");
        let origins =
            std::collections::HashMap::from([("a/picker".into(), (path.into(), "v0.1.0".into()))]);
        assert!(
            Arc::ptr_eq(
                &catalog.items[0],
                &catalog.installed(path, &origins).unwrap()
            ),
            "installation keeps the original card"
        );
        assert!(
            catalog
                .installed(std::path::Path::new("/plugins-dev/picker"), &origins)
                .is_none(),
            "a development copy with the same id is a separate plugin"
        );
        let moved =
            std::collections::HashMap::from([("b/picker".into(), (path.into(), "v0.1.0".into()))]);
        assert!(
            catalog.installed(path, &moved).is_none(),
            "a different publisher cannot replace a listing by reusing its id"
        );
    }
}
