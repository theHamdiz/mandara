//! What the open panel holds: each subscription's snapshot, the unified
//! timeline over them and the filtered list on screen, cached by every
//! input it depends on. Nothing here touches the disk or the network.

use std::rc::Rc;
use std::sync::Arc;

use mandara_services::feeds::{Entry, Snapshot, Subscription};

/// Entries the timeline holds at most, newest first, across all feeds.
pub const MAX_TIMELINE: usize = 2000;

/// What the sidebar selects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    All,
    Starred,
    Folder(Rc<str>),
    /// A subscription by address.
    Feed(Rc<str>),
}

/// The list filter beside the search box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Unread,
}

/// One timeline row: indices into the library, and its time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    pub feed: u16,
    pub entry: u16,
    pub when: i64,
}

/// A subscription and its last snapshot.
pub struct FeedRow {
    pub sub: Subscription,
    pub snapshot: Option<Arc<Snapshot>>,
}

impl FeedRow {
    /// The name shown: yours, the feed's own, or its host.
    pub fn name(&self) -> &str {
        if !self.sub.title.is_empty() { return &self.sub.title; }
        match &self.snapshot {
            Some(snapshot) if !snapshot.feed.title.is_empty() => &snapshot.feed.title,
            _ => host(&self.sub.url),
        }
    }

    pub fn unread(&self) -> usize { self.snapshot.as_ref().map_or(0, |snapshot| snapshot.unread()) }

    pub fn error(&self) -> Option<&str> { self.snapshot.as_ref().and_then(|snapshot| snapshot.error.as_deref()) }
}

/// `https://www.example.org/feed` → `example.org`.
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host.strip_prefix("www.").unwrap_or(host)
}

/// The filtered list's key: everything it depends on.
#[derive(Clone, PartialEq, Eq)]
struct ListKey {
    revision: u64,
    scope: Scope,
    filter: Filter,
    query: Box<str>,
}

#[derive(Default)]
pub struct Library {
    pub feeds: Vec<FeedRow>,
    /// Bumped on every snapshot change; caches compare it.
    revision: u64,
    timeline: Option<(u64, Rc<[Item]>)>,
    list: Option<(ListKey, Rc<[Item]>)>,
}

impl Library {
    /// A library for `subs`, keeping snapshots of feeds still subscribed.
    /// Returns whether the list changed.
    pub fn sync(&mut self, subs: &[Subscription]) -> bool {
        if self.feeds.len() == subs.len() && self.feeds.iter().zip(subs).all(|(row, sub)| row.sub == *sub) { return false; }
        let mut old = std::mem::take(&mut self.feeds);
        self.feeds = subs.iter().map(|sub| {
            let snapshot = old.iter_mut().find(|row| row.sub.url == sub.url).and_then(|row| row.snapshot.take());
            FeedRow { sub: sub.clone(), snapshot }
        }).collect();
        self.changed();
        true
    }

    #[cfg(test)]
    pub fn revision(&self) -> u64 { self.revision }

    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.timeline = None;
        self.list = None;
    }

    /// Install a feed's snapshot (from disk, a refresh or a flag change).
    pub fn set(&mut self, snapshot: Arc<Snapshot>) -> bool {
        let Some(row) = self.feeds.iter_mut().find(|row| row.sub.url == snapshot.feed.url) else { return false };
        row.snapshot = Some(snapshot);
        self.changed();
        true
    }

    pub fn feed_index(&self, url: &str) -> Option<usize> { self.feeds.iter().position(|row| row.sub.url == url) }

    pub fn entry(&self, item: Item) -> Option<(&FeedRow, &Entry)> {
        let row = self.feeds.get(usize::from(item.feed))?;
        Some((row, row.snapshot.as_ref()?.feed.entries.get(usize::from(item.entry))?))
    }

    /// Find an entry by its feed and link.
    pub fn find(&self, url: &str, link: &str) -> Option<Item> {
        let feed = self.feed_index(url)?;
        let entries = &self.feeds[feed].snapshot.as_ref()?.feed.entries;
        let entry = entries.iter().position(|entry| entry.link == link)?;
        Some(Item { feed: feed as u16, entry: entry as u16, when: entries[entry].when() })
    }

    pub fn is_read(&self, item: Item) -> bool {
        self.feeds.get(usize::from(item.feed)).and_then(|row| row.snapshot.as_ref()).zip(self.entry(item)).is_some_and(|(snapshot, (_, entry))| snapshot.read.contains(&entry.link))
    }

    pub fn is_starred(&self, item: Item) -> bool {
        self.feeds.get(usize::from(item.feed)).and_then(|row| row.snapshot.as_ref()).zip(self.entry(item)).is_some_and(|(snapshot, (_, entry))| snapshot.starred.contains(&entry.link))
    }

    /// Change a flag locally (the disk write runs separately and replaces
    /// the snapshot when it lands).
    pub fn flag(&mut self, item: Item, read: Option<bool>, starred: Option<bool>) {
        let Some(row) = self.feeds.get_mut(usize::from(item.feed)) else { return };
        let Some(snapshot) = row.snapshot.as_mut() else { return };
        let Some(link) = snapshot.feed.entries.get(usize::from(item.entry)).map(|entry| entry.link.clone()) else { return };
        let snapshot = Arc::make_mut(snapshot);
        if let Some(read) = read { if read { snapshot.read.insert(link.clone()); } else { snapshot.read.remove(&link); } }
        if let Some(starred) = starred { if starred { snapshot.starred.insert(link); } else { snapshot.starred.remove(&link); } }
        self.changed();
    }

    /// Mark every unread entry of `scope` read locally; returns each feed
    /// touched with the links it marked.
    pub fn mark_scope_read(&mut self, scope: &Scope) -> Vec<(String, Vec<String>)> {
        let mut touched = Vec::new();
        for row in &mut self.feeds {
            if !in_scope(scope, row) && *scope != Scope::Starred { continue; }
            let Some(snapshot) = row.snapshot.as_mut() else { continue };
            if snapshot.unread() == 0 { continue; }
            let snapshot = Arc::make_mut(snapshot);
            let links: Vec<String> = snapshot.feed.entries.iter().filter(|entry| !snapshot.read.contains(&entry.link) && (*scope != Scope::Starred || snapshot.starred.contains(&entry.link))).map(|entry| entry.link.clone()).collect();
            if links.is_empty() { continue; }
            snapshot.read.extend(links.iter().cloned());
            touched.push((row.sub.url.clone(), links));
        }
        if !touched.is_empty() { self.changed(); }
        touched
    }

    /// Every entry of every feed, newest first, at most [`MAX_TIMELINE`].
    pub fn timeline(&mut self) -> Rc<[Item]> {
        if let Some((revision, items)) = &self.timeline && *revision == self.revision { return items.clone(); }
        let mut items: Vec<Item> = Vec::with_capacity(self.feeds.iter().filter_map(|row| row.snapshot.as_ref()).map(|snapshot| snapshot.feed.entries.len()).sum());
        for (feed, row) in self.feeds.iter().enumerate() {
            let Some(snapshot) = &row.snapshot else { continue };
            items.extend(snapshot.feed.entries.iter().enumerate().map(|(entry, value)| Item { feed: feed as u16, entry: entry as u16, when: value.when() }));
        }
        items.sort_by_key(|item| std::cmp::Reverse(item.when));
        items.truncate(MAX_TIMELINE);
        let items: Rc<[Item]> = items.into();
        self.timeline = Some((self.revision, items.clone()));
        items
    }

    /// The rows on screen for `scope`, `filter` and `query` (every word
    /// must appear in the title, excerpt, author or feed name).
    pub fn list(&mut self, scope: &Scope, filter: Filter, query: &str) -> Rc<[Item]> {
        let query = query.trim();
        if let Some((key, items)) = &self.list && key.revision == self.revision && key.scope == *scope && key.filter == filter && &*key.query == query { return items.clone(); }
        let timeline = self.timeline();
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut haystack = String::new();
        let items: Rc<[Item]> = timeline.iter().copied().filter(|item| {
            let Some((row, entry)) = self.entry(*item) else { return false };
            let snapshot = row.snapshot.as_ref().expect("entries come from snapshots");
            let scoped = match scope { Scope::Starred => snapshot.starred.contains(&entry.link), _ => in_scope(scope, row) };
            if !scoped || (filter == Filter::Unread && snapshot.read.contains(&entry.link)) { return false; }
            if words.is_empty() { return true; }
            haystack.clear();
            for part in [entry.title.as_str(), entry.summary.as_str(), entry.author.as_deref().unwrap_or_default(), row.name()] {
                haystack.push_str(&part.to_lowercase());
                haystack.push('\u{1f}');
            }
            words.iter().all(|word| haystack.contains(word.as_str()))
        }).collect();
        self.list = Some((ListKey { revision: self.revision, scope: scope.clone(), filter, query: query.into() }, items.clone()));
        items
    }

    /// Unread entries in a scope (sidebar counts).
    pub fn unread_in(&self, scope: &Scope) -> usize {
        match scope {
            Scope::Starred => self.feeds.iter().filter_map(|row| row.snapshot.as_ref()).map(|snapshot| snapshot.starred.iter().filter(|link| !snapshot.read.contains(*link)).count()).sum(),
            _ => self.feeds.iter().filter(|row| in_scope(scope, row)).map(FeedRow::unread).sum(),
        }
    }

    pub fn starred(&self) -> usize { self.feeds.iter().filter_map(|row| row.snapshot.as_ref()).map(|snapshot| snapshot.starred.len()).sum() }

    /// Folders in first-use order.
    pub fn folders(&self) -> Vec<Rc<str>> {
        let mut folders: Vec<Rc<str>> = Vec::new();
        for row in &self.feeds {
            if !row.sub.folder.is_empty() && !folders.iter().any(|folder| **folder == *row.sub.folder) { folders.push(row.sub.folder.as_str().into()); }
        }
        folders
    }
}

fn in_scope(scope: &Scope, row: &FeedRow) -> bool {
    match scope {
        Scope::All => true,
        Scope::Starred => false,
        Scope::Folder(folder) => *row.sub.folder == **folder,
        Scope::Feed(url) => *row.sub.url == **url,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::feeds::Feed;

    fn snapshot(url: &str, entries: &[(&str, i64)]) -> Arc<Snapshot> {
        Arc::new(Snapshot {
            feed: Feed { url: url.into(), title: format!("Feed {url}"), entries: entries.iter().map(|(title, when)| Entry { id: (*title).into(), title: (*title).into(), link: format!("{url}/{title}"), published: Some(*when), summary: format!("about {title}"), image: None, author: None, seen: *when }).collect(), ..Feed::default() },
            ..Snapshot::default()
        })
    }

    fn sub(url: &str, folder: &str) -> Subscription { Subscription { url: url.into(), folder: folder.into(), ..Subscription::default() } }

    fn library() -> Library {
        let mut lib = Library::default();
        lib.sync(&[sub("https://a.example", "Tech"), sub("https://b.example", ""), sub("https://c.example", "Tech")]);
        lib.set(snapshot("https://a.example", &[("alpha", 30), ("apple", 10)]));
        lib.set(snapshot("https://b.example", &[("beta", 20)]));
        lib
    }

    fn titles(lib: &Library, items: &[Item]) -> Vec<String> { items.iter().map(|item| lib.entry(*item).unwrap().1.title.clone()).collect() }

    fn listed(lib: &mut Library, scope: &Scope, filter: Filter, query: &str) -> Vec<String> { let items = lib.list(scope, filter, query); titles(lib, &items) }

    #[test]
    fn the_timeline_merges_feeds_newest_first_and_lists_are_cached() {
        let mut lib = library();
        let all = lib.list(&Scope::All, Filter::All, "");
        assert_eq!(titles(&lib, &all), ["alpha", "beta", "apple"]);
        assert!(Rc::ptr_eq(&all, &lib.list(&Scope::All, Filter::All, "")), "an unchanged library reuses its list");
        assert_eq!(listed(&mut lib, &Scope::Folder("Tech".into()), Filter::All, ""), ["alpha", "apple"]);
        assert_eq!(listed(&mut lib, &Scope::Feed("https://b.example".into()), Filter::All, ""), ["beta"]);
        assert_eq!(listed(&mut lib, &Scope::All, Filter::All, "  APP "), ["apple"]);
        assert_eq!(listed(&mut lib, &Scope::All, Filter::All, "about feed"), ["alpha", "beta", "apple"], "words match excerpts and feed names");
        assert_eq!(lib.folders().iter().map(|f| f.to_string()).collect::<Vec<_>>(), ["Tech"]);
    }

    #[test]
    fn flags_filter_and_count_locally() {
        let mut lib = library();
        let alpha = lib.find("https://a.example", "https://a.example/alpha").unwrap();
        lib.flag(alpha, Some(true), Some(true));
        assert!(lib.is_read(alpha) && lib.is_starred(alpha));
        assert_eq!(listed(&mut lib, &Scope::All, Filter::Unread, ""), ["beta", "apple"]);
        assert_eq!(listed(&mut lib, &Scope::Starred, Filter::All, ""), ["alpha"]);
        assert_eq!(lib.unread_in(&Scope::All), 2);
        assert_eq!(lib.unread_in(&Scope::Starred), 0);
        assert_eq!(lib.mark_scope_read(&Scope::Folder("Tech".into())), [("https://a.example".to_owned(), vec!["https://a.example/apple".to_owned()])], "only what was unread");
        let beta = lib.find("https://b.example", "https://b.example/beta").unwrap();
        lib.flag(beta, None, Some(true));
        let apple = lib.find("https://a.example", "https://a.example/apple").unwrap();
        lib.flag(apple, Some(false), None);
        assert_eq!(lib.mark_scope_read(&Scope::Starred), [("https://b.example".to_owned(), vec!["https://b.example/beta".to_owned()])], "saved articles only");
        assert!(!lib.is_read(apple), "an unsaved article keeps its flag");
        lib.flag(apple, Some(true), None);
        assert_eq!(lib.unread_in(&Scope::All), 0);
        assert!(lib.mark_scope_read(&Scope::Folder("Tech".into())).is_empty(), "nothing left to mark");
    }

    #[test]
    fn syncing_keeps_snapshots_of_feeds_still_subscribed() {
        let mut lib = library();
        let revision = lib.revision();
        lib.sync(&[sub("https://a.example", "Tech"), sub("https://b.example", ""), sub("https://c.example", "Tech")]);
        assert_eq!(lib.revision(), revision, "an unchanged list is not a change");
        lib.sync(&[sub("https://b.example", "News")]);
        assert_eq!(lib.feeds.len(), 1);
        assert!(lib.feeds[0].snapshot.is_some());
        assert_eq!(listed(&mut lib, &Scope::All, Filter::All, ""), ["beta"]);
        assert!(!lib.set(snapshot("https://gone.example", &[])), "a late result for a removed feed is dropped");
    }

    #[test]
    fn the_timeline_is_bounded() {
        let mut lib = Library::default();
        let subs: Vec<Subscription> = (0..12).map(|n| sub(&format!("https://{n}.example"), "")).collect();
        lib.sync(&subs);
        for n in 0..12 {
            let titles: Vec<String> = (0..200).map(|i| format!("t{i}")).collect();
            let entries: Vec<(&str, i64)> = titles.iter().map(|t| (t.as_str(), n * 1000 + 7)).collect();
            lib.set(snapshot(&format!("https://{n}.example"), &entries));
        }
        assert_eq!(lib.timeline().len(), MAX_TIMELINE);
        assert_eq!(host("https://www.example.org/feed?x"), "example.org");
    }
}
