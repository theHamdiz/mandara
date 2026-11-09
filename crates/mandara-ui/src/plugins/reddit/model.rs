//! The reader's state and its single worker.
//!
//! One job runs at a time: the newest page or thread request (a newer one
//! replaces a waiting one, and navigation makes a running one stale), then
//! queued previews. Results carry the generation they were asked for; a
//! stale one is dropped. Closing the panel drops previews at once and the
//! pages and thread a little later; switching the plugin off drops all of
//! it and the token.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use gpui::{App, ListAlignment, ListOffset, ListState, RenderImage, Task, px};
use mandara_services::reddit::{Error, Listing, Node, Post, Query, Session, Subreddit, Thread, Want};

use super::images::{self, Images, Request};
use super::text::Body;
use crate::plugins::panel_fx::ViewSet;

/// Pages appended to one list before it asks you to open Reddit instead.
pub(super) const MAX_PAGES: usize = 8;
/// Pictures stay this long after the panel closes (its closing animation).
const DROP_IMAGES_AFTER: Duration = Duration::from_millis(600);
/// Posts and comments stay a little longer, so a quick reopen is instant.
const RELEASE_AFTER: Duration = Duration::from_secs(90);
/// Rows rendered beyond the visible part of a list.
const OVERDRAW_PX: f32 = 600.0;

/// The connection is created on first use, never at registration.
pub(super) static SESSION: LazyLock<Mutex<Session>> = LazyLock::new(|| Mutex::new(Session::default()));

pub(super) fn session() -> std::sync::MutexGuard<'static, Session> {
    SESSION.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Which page of the panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Page { #[default] Posts, Discover, Post }

/// The posts of one query, page after page, without repeats.
pub(super) struct Feed {
    pub query: Option<Query>,
    pages: Vec<Arc<Listing>>,
    /// `(page, post)` of each shown post, in order.
    rows: Vec<(u16, u16)>,
    pub after: Option<Box<str>>,
    pub loading: bool,
    pub error: Option<Error>,
    pub list: ListState,
    pub loaded_at: Option<Instant>,
    /// After a rate limit, when the list asks again by itself (while shown).
    pub retry_at: Option<Instant>,
    /// Changes whenever the shown posts change (for the grid's rows).
    pub version: u64,
    /// The last page came from the public feeds.
    pub limited: bool,
}

impl Default for Feed {
    fn default() -> Self {
        Self { query: None, pages: Vec::new(), rows: Vec::new(), after: None, loading: false, error: None, list: ListState::new(0, ListAlignment::Top, px(OVERDRAW_PX)), loaded_at: None, retry_at: None, version: 0, limited: false }
    }
}

impl Feed {
    pub fn len(&self) -> usize { self.rows.len() }

    pub fn post(&self, row: usize) -> Option<&Post> {
        let (page, post) = *self.rows.get(row)?;
        self.pages.get(usize::from(page))?.posts.get(usize::from(post))
    }

    /// The page holding a shown post (shared) and the post's index in it.
    pub fn locate(&self, row: usize) -> Option<(Arc<Listing>, usize)> {
        let (page, post) = *self.rows.get(row)?;
        let page = self.pages.get(usize::from(page))?;
        (usize::from(post) < page.posts.len()).then(|| (page.clone(), usize::from(post)))
    }

    pub fn find(&self, id: &str) -> Option<&Post> {
        (0..self.rows.len()).filter_map(|row| self.post(row)).find(|post| &*post.id == id)
    }

    /// More pages exist and the list may still grow.
    pub fn more(&self) -> bool { self.after.is_some() && self.pages.len() < MAX_PAGES }

    /// The list's items: posts plus the "load more" footer.
    pub fn items(&self) -> usize { self.rows.len() + 1 }

    /// Append a page, skipping posts already shown (Reddit can repeat a
    /// post when rankings move between pages).
    pub fn append(&mut self, page: Arc<Listing>) {
        self.version += 1;
        let index = self.pages.len() as u16;
        let before = self.rows.len();
        for (at, post) in page.posts.iter().enumerate() {
            if self.find(&post.id).is_none() { self.rows.push((index, at as u16)); }
        }
        self.after = page.after.clone().filter(|_| !page.posts.is_empty());
        self.limited = page.limited;
        self.pages.push(page);
        if before == 0 {
            self.list.reset(self.items());
        } else {
            self.list.splice(before..before + 1, self.rows.len() - before + 1);
        }
    }

    /// Start over for another query.
    pub fn reset(&mut self, query: Option<Query>) {
        self.version += 1;
        self.query = query;
        self.pages.clear();
        self.pages.shrink_to_fit();
        self.rows.clear();
        self.rows.shrink_to_fit();
        self.after = None;
        self.error = None;
        self.loading = false;
        self.loaded_at = None;
        self.retry_at = None;
        self.limited = false;
        self.list.reset(0);
    }
}

/// The narrowest a grid tile gets before the grid drops a column.
pub(super) const GRID_MIN_TILE: f32 = 176.0;
pub(super) const GRID_GAP: f32 = 8.0;
const GRID_MIN_COLUMNS: usize = 2;
const GRID_MAX_COLUMNS: usize = 3;

/// Columns that fit `width` logical pixels: two to three.
pub(super) fn grid_columns(width: f32) -> usize {
    (((width + GRID_GAP) / (GRID_MIN_TILE + GRID_GAP)).floor().max(0.0) as usize).clamp(GRID_MIN_COLUMNS, GRID_MAX_COLUMNS)
}

/// A tile's width when `columns` tiles share `width`.
pub(super) fn tile_width(width: f32, columns: usize) -> f32 {
    ((width - GRID_GAP * (columns.saturating_sub(1)) as f32) / columns.max(1) as f32).floor()
}

/// The grid's rows of posts: shown posts only, in order, `columns` a row.
/// Rebuilt only when the posts, the NSFW gate or the column count change.
pub(super) struct Grid {
    key: Option<(u64, bool, usize)>,
    pub posts: Vec<u32>,
    pub columns: usize,
    pub list: ListState,
}

impl Default for Grid {
    fn default() -> Self { Self { key: None, posts: Vec::new(), columns: GRID_MIN_COLUMNS, list: ListState::new(0, ListAlignment::Top, px(OVERDRAW_PX)) } }
}

impl Grid {
    /// Rows of tiles (without the footer).
    pub fn rows(&self) -> usize { self.posts.len().div_ceil(self.columns.max(1)) }

    pub fn sync(&mut self, feed: &Feed, hidden: impl Fn(&Post) -> bool, hide_key: bool, columns: usize) {
        let key = (feed.version, hide_key, columns);
        if self.key == Some(key) { return; }
        let same_posts = self.key.is_some_and(|(version, hide, _)| version == feed.version && hide == hide_key);
        self.key = Some(key);
        self.columns = columns;
        if !same_posts {
            self.posts.clear();
            self.posts.extend((0..feed.len()).filter(|row| feed.post(*row).is_some_and(|post| !hidden(post))).map(|row| row as u32));
        }
        let top = self.list.logical_scroll_top();
        let count = self.rows() + 1;
        self.list.reset(count);
        self.list.scroll_to(ListOffset { item_ix: top.item_ix.min(count - 1), offset_in_item: top.offset_in_item });
    }

    /// The feed rows of one grid row.
    pub fn row(&self, index: usize) -> &[u32] {
        let start = (index * self.columns).min(self.posts.len());
        &self.posts[start..(start + self.columns).min(self.posts.len())]
    }
}

/// The sidebar card: the first posts of one community.
#[derive(Default)]
pub(super) struct Card {
    pub query: Option<Query>,
    pub page: Option<Arc<Listing>>,
    pub loading: bool,
    pub error: Option<Error>,
    pub loaded_at: Option<Instant>,
}

/// One comment, parsed for drawing.
pub(super) struct CommentRow {
    pub id: Box<str>,
    pub author: Box<str>,
    pub score: i64,
    pub score_hidden: bool,
    pub created: i64,
    pub depth: u8,
    pub op: bool,
    pub stickied: bool,
    pub distinguished: bool,
    pub edited: bool,
    pub body: Body,
}

pub(super) struct MoreRow {
    pub parent: Box<str>,
    pub children: Vec<Box<str>>,
    pub count: u32,
    pub depth: u8,
}

pub(super) enum Row { Comment(CommentRow), More(MoreRow) }

impl Row {
    pub fn depth(&self) -> u8 { match self { Self::Comment(c) => c.depth, Self::More(m) => m.depth } }

    pub fn from_node(node: Node) -> Self {
        match node {
            Node::Comment(c) => Self::Comment(CommentRow {
                body: Body::parse(&c.body),
                id: c.id, author: c.author, score: c.score, score_hidden: c.score_hidden, created: c.created,
                depth: c.depth, op: c.op, stickied: c.stickied, distinguished: c.distinguished, edited: c.edited,
            }),
            Node::More(m) => Self::More(MoreRow { parent: m.parent, children: m.children, count: m.count, depth: m.depth }),
        }
    }

    /// Key of a "load more" row, for its spinner.
    pub fn more_key(&self) -> Option<&str> {
        match self { Self::More(m) => Some(m.children.first().map_or(&*m.parent, |id| &**id)), Self::Comment(_) => None }
    }
}

/// Items above the comments in the reader's list: the post, then the
/// comments header.
pub(super) const HEAD_ITEMS: usize = 2;

/// An open post: shown at once from the list, comments as they arrive.
pub(super) struct Reader {
    pub id: Box<str>,
    pub post: Option<Arc<Post>>,
    pub body: Option<Body>,
    pub rows: Vec<Row>,
    pub truncated: bool,
    /// Comments folded by the reader (by id).
    pub collapsed: HashSet<Box<str>>,
    /// Indices of `rows` currently shown.
    pub visible: Vec<u32>,
    pub list: ListState,
    pub loading: bool,
    pub loading_more: Option<Box<str>>,
    pub error: Option<Error>,
    pub gallery: usize,
    /// Read from the public feeds: flat comments without scores.
    pub limited: bool,
    /// Spoilers and NSFW media this post shows uncovered.
    pub revealed: bool,
}

impl Reader {
    fn new(id: Box<str>, post: Option<Arc<Post>>) -> Self {
        let list = ListState::new(HEAD_ITEMS, ListAlignment::Top, px(OVERDRAW_PX));
        Self { id, post, body: None, rows: Vec::new(), truncated: false, collapsed: HashSet::new(), visible: Vec::new(), list, loading: true, loading_more: None, error: None, gallery: 0, revealed: false, limited: false }
    }

    /// Rebuild the shown rows after a fold or new replies, keeping the
    /// scroll position.
    pub fn relayout(&mut self) {
        self.visible.clear();
        let mut hide_below: Option<u8> = None;
        for (index, row) in self.rows.iter().enumerate() {
            if let Some(depth) = hide_below {
                if row.depth() > depth { continue; }
                hide_below = None;
            }
            self.visible.push(index as u32);
            if let Row::Comment(c) = row && self.collapsed.contains(&c.id) { hide_below = Some(c.depth); }
        }
        let top = self.list.logical_scroll_top();
        let count = HEAD_ITEMS + self.visible.len();
        self.list.reset(count);
        self.list.scroll_to(ListOffset { item_ix: top.item_ix.min(count.saturating_sub(1)), offset_in_item: top.offset_in_item });
    }

    /// Replies hidden under a folded comment.
    pub fn hidden_under(&self, index: usize) -> usize {
        let Some(depth) = self.rows.get(index).map(Row::depth) else { return 0 };
        self.rows[index + 1..].iter().take_while(|row| row.depth() > depth).count()
    }
}

/// The plugin's whole state; one per shell, shared by every bar's panel.
#[derive(Default)]
pub(super) struct Model {
    pub views: ViewSet,
    pub page: Page,
    pub feed: Feed,
    pub grid: Grid,
    pub reader: Option<Reader>,
    pub images: Images,
    pub searching: bool,
    /// Limited mode's banner shows the steps to full mode.
    pub setup_open: bool,
    /// NSFW or spoiler posts the reader chose to see in the list.
    pub revealed: HashSet<Box<str>>,
    pub want: Option<Want>,
    /// A short confirmation (a copied link).
    pub flash: crate::plugins::extras::Flash,
    /// Bumped by navigation; a page or thread result must match it.
    generation: u64,
    /// Bumped when previews are dropped; a picture must match it.
    image_generation: u64,
    api: Option<ApiJob>,
    card_job: Option<ApiJob>,
    pub card: Card,
    running: bool,
    pump_pending: bool,
    /// Limited mode: when the queue lets the waiting request go.
    pub waiting_until: Option<Instant>,
    wait_timer: Option<Task<()>>,
    open: bool,
    release: Option<Task<()>>,
}

/// Requests that use Reddit's budget. Only the newest waits.
#[derive(Clone)]
pub(super) enum ApiJob {
    Page { query: Query, after: Option<Box<str>>, count: usize, want: Want },
    Thread { id: Box<str>, permalink: Option<Box<str>>, want: Want },
    More { post: Box<str>, key: Box<str>, children: Vec<Box<str>>, depth: u8, room: usize },
    Continue { post: Box<str>, key: Box<str>, parent: Box<str>, depth: u8, room: usize },
    /// The sidebar card's community, waiting behind the panel's requests.
    Card { query: Query, want: Want },
}

enum Done {
    Page(Result<Arc<Listing>, Error>),
    Thread(Result<(Arc<Post>, Option<Body>, Vec<Row>, bool, bool), Error>),
    More(Box<str>, Result<Vec<Row>, Error>),
    Image(images::Key, Option<(Arc<RenderImage>, usize)>),
    Card(Query, Result<Arc<Listing>, Error>),
}

pub(super) type Shared = Rc<RefCell<Model>>;

thread_local! { static MODEL: RefCell<Option<Shared>> = const { RefCell::new(None) }; }

pub(super) fn model() -> Shared {
    MODEL.with(|slot| slot.borrow_mut().get_or_insert_with(Shared::default).clone())
}

/// The model only if something already created it.
pub(super) fn existing() -> Option<Shared> { MODEL.with(|slot| slot.borrow().clone()) }

/// Switched off: drop everything, and late results find nothing to fill.
pub(super) fn stop() {
    if let Some(model) = MODEL.with(|slot| slot.borrow_mut().take()) {
        let mut m = model.borrow_mut();
        m.generation += 1;
        m.image_generation += 1;
        m.release = None;
        drop(m.images.clear());
        m.feed.reset(None);
        m.reader = None;
    }
    with_session_later(Session::stop);
}

/// Change the session off the UI thread: a request in flight holds it for
/// up to its timeout. Does nothing if the session was never created.
fn with_session_later(change: fn(&mut Session)) {
    if LazyLock::get(&SESSION).is_none() {
        mandara_services::memory::request_trim();
        return;
    }
    let spawned = std::thread::Builder::new().name("mandara-reddit-release".into()).spawn(move || {
        change(&mut session());
        mandara_services::memory::request_trim();
    });
    if let Err(error) = spawned { tracing::warn!("reddit: {error}"); }
}

impl Model {

    /// Ask for the first page of `query`, or keep the current list.
    pub fn show(&mut self, query: Query, want: Want) {
        if self.feed.query.as_ref() == Some(&query) && (self.feed.len() > 0 || self.feed.loading) { return; }
        self.generation += 1;
        self.feed.reset(Some(query.clone()));
        self.feed.loading = true;
        self.api = Some(ApiJob::Page { query, after: None, count: 0, want });
    }

    /// Reload the current list from its first page.
    pub fn refresh(&mut self, want: Want) {
        let Some(query) = self.feed.query.clone() else { return };
        self.feed.reset(None);
        self.show(query, want);
    }

    /// The next page of the current list.
    pub fn load_more(&mut self, want: Want) {
        if self.feed.loading || !self.feed.more() { return; }
        let Some(query) = self.feed.query.clone() else { return };
        self.feed.loading = true;
        self.feed.error = None;
        self.api = Some(ApiJob::Page { query, after: self.feed.after.clone(), count: self.feed.len(), want });
    }

    /// Ask for the sidebar card's community when it changed or its posts
    /// are older than `every`. Only a shown card calls this.
    pub fn want_card(&mut self, query: Query, want: Want, every: Duration, now: Instant) -> Option<Duration> {
        let same = self.card.query.as_ref() == Some(&query);
        if !same {
            self.card = Card { query: Some(query.clone()), ..Card::default() };
        }
        if self.card.loading { return None; }
        let age = self.card.loaded_at.map(|at| now.saturating_duration_since(at));
        match age {
            Some(age) if age < every => Some(every - age),
            _ => {
                self.card.loading = true;
                self.card_job = Some(ApiJob::Card { query, want });
                None
            }
        }
    }

    /// The sidebar hid: its card's posts go (the session keeps the page
    /// briefly, so showing it again soon costs no request).
    pub fn drop_card(&mut self) {
        self.card = Card::default();
        self.card_job = None;
    }

    /// Open a post the reader already knows (from the sidebar card).
    pub fn open_known(&mut self, post: &Post, want: Want) {
        self.generation += 1;
        let permalink = (!post.permalink.is_empty()).then(|| post.permalink.clone());
        self.reader = Some(Reader::new(post.id.clone(), Some(Arc::new(post.clone()))));
        self.page = Page::Post;
        self.api = Some(ApiJob::Thread { id: post.id.clone(), permalink, want });
    }

    /// Open a post, from the list (shown at once) or by id.
    pub fn open_post(&mut self, id: &str, want: Want) {
        if self.reader.as_ref().is_some_and(|reader| &*reader.id == id && reader.error.is_none()) {
            self.page = Page::Post;
            return;
        }
        self.generation += 1;
        let post = self.feed.find(id).cloned().map(Arc::new);
        let permalink = post.as_ref().map(|post| post.permalink.clone()).filter(|link| !link.is_empty());
        self.reader = Some(Reader::new(id.into(), post));
        self.page = Page::Post;
        self.api = Some(ApiJob::Thread { id: id.into(), permalink, want });
    }

    pub fn reload_post(&mut self, want: Want) {
        let Some(id) = self.reader.as_ref().map(|reader| reader.id.clone()) else { return };
        self.reader = None;
        self.open_post(&id, want);
    }

    /// Back from a post or Discover to the list.
    pub fn back_or_home(&mut self) {
        match self.page {
            Page::Post => self.back(),
            Page::Discover => self.page = Page::Posts,
            Page::Posts => {}
        }
    }

    /// Back to the list; a running thread request becomes stale.
    pub fn back(&mut self) {
        if self.page == Page::Post { self.generation += 1; }
        if matches!(self.api, Some(ApiJob::Thread { .. } | ApiJob::More { .. } | ApiJob::Continue { .. })) { self.api = None; }
        if let Some(reader) = &mut self.reader { reader.loading = false; reader.loading_more = None; }
        self.page = Page::Posts;
    }

    /// Load the replies behind a "more" row.
    pub fn expand(&mut self, index: usize) {
        let Some(reader) = &mut self.reader else { return };
        if reader.loading_more.is_some() { return; }
        let room = mandara_services::reddit::MAX_COMMENTS.saturating_sub(reader.rows.len());
        let Some(Row::More(more)) = reader.rows.get(index) else { return };
        let key: Box<str> = reader.rows[index].more_key().unwrap_or_default().into();
        let post = reader.id.clone();
        self.api = Some(if more.children.is_empty() {
            ApiJob::Continue { post, key: key.clone(), parent: more.parent.clone(), depth: more.depth, room }
        } else {
            ApiJob::More { post, key: key.clone(), children: more.children.iter().take(mandara_services::reddit::MORE_BATCH).cloned().collect(), depth: more.depth, room }
        });
        reader.loading_more = Some(key);
    }

    /// The panel opened or closed. Closing drops pictures after the close
    /// animation and the rest a little later; reopening cancels both.
    pub fn follow_panel(shared: &Shared, open: bool, cx: &mut App) {
        let mut m = shared.borrow_mut();
        if m.open == open { return; }
        m.open = open;
        if open {
            m.release = None;
            return;
        }
        let weak = Rc::downgrade(shared);
        m.release = Some(cx.spawn(async move |cx| {
            cx.background_executor().timer(DROP_IMAGES_AFTER).await;
            let Some(shared) = weak.upgrade() else { return };
            let gone = { let mut m = shared.borrow_mut(); m.image_generation += 1; m.images.clear() };
            cx.update(|cx| for image in gone { cx.drop_image(image, None); });
            mandara_services::memory::request_trim();
            cx.background_executor().timer(RELEASE_AFTER.saturating_sub(DROP_IMAGES_AFTER)).await;
            {
                let mut m = shared.borrow_mut();
                m.generation += 1;
                m.api = None;
                let query = m.feed.query.take();
                m.feed.reset(None);
                m.feed.query = query;
                m.reader = None;
                m.grid = Grid::default();
                m.wait_timer = None;
                m.waiting_until = None;
                m.page = if m.page == Page::Post { Page::Posts } else { m.page };
                m.release = None;
            }
            with_session_later(Session::release);
        }));
    }


    /// What a status command reports about the reader.
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "page": match self.page { Page::Posts => "posts", Page::Discover => "discover", Page::Post => "post" },
            "open": self.open,
            "query": self.feed.query,
            "posts": self.feed.len(),
            "pages": self.feed.pages.len(),
            "post": self.reader.as_ref().map(|reader| &reader.id),
            "comments": self.reader.as_ref().map_or(0, |reader| reader.rows.len()),
            "images": { "decoded": self.images.len(), "bytes": self.images.bytes(), "queued": self.images.queued() },
        })
    }
}

/// Rows queue pictures while the list lays out, after this frame's render
/// started the worker: start it once that layout is done.
pub(super) fn pump_soon(shared: &Shared, cx: &mut App) {
    let idle = { let m = shared.borrow(); !m.running && m.images.queued() > 0 && !m.pump_pending };
    if !idle { return; }
    shared.borrow_mut().pump_pending = true;
    let shared = shared.clone();
    cx.defer(move |cx| {
        shared.borrow_mut().pump_pending = false;
        pump(&shared, cx);
    });
}

/// How long limited mode's queue still holds requests, when requests go to
/// the public feeds (no app, or the last request had to fall back).
fn limited_wait(settings: &super::Settings) -> Option<Duration> {
    let queue = mandara_services::reddit::limited().lock();
    if !(settings.client_id.is_empty() || queue.active) { return None; }
    queue.pacer.wait(Instant::now())
}

/// Start the next job unless one is running. Page and thread requests go
/// before pictures.
pub(super) fn pump(shared: &Shared, cx: &mut App) {
    // The client id comes from Studio at the moment a request starts.
    let settings = super::current(cx);
    let (job, generation, image_generation) = {
        let mut m = shared.borrow_mut();
        if m.running { return; }
        // The panel's request first, then the sidebar card's.
        let next = match m.api.take() { Some(api) => Some(api), None => m.card_job.take() };
        let job = match next {
            // Limited mode: the newest request waits for the one-a-minute
            // queue (a newer one replaces it meanwhile); pictures still load.
            Some(api) if let Some(wait) = limited_wait(&settings) => {
                if matches!(api, ApiJob::Card { .. }) { m.card_job = Some(api) } else { m.api = Some(api) }
                m.waiting_until = Some(Instant::now() + wait);
                if m.wait_timer.is_none() {
                    let weak = Rc::downgrade(shared);
                    m.wait_timer = Some(cx.spawn(async move |cx| {
                        cx.background_executor().timer(wait).await;
                        let Some(shared) = weak.upgrade() else { return };
                        { let mut m = shared.borrow_mut(); m.wait_timer = None; m.waiting_until = None; }
                        cx.update(|cx| pump(&shared, cx));
                    }));
                }
                match m.images.next() { Some(request) => Job::Image(request), None => return }
            }
            Some(api) => Job::Api(api, (settings.client_id.clone(), settings.username.clone())),
            None => match m.images.next() { Some(request) => Job::Image(request), None => return },
        };
        m.running = true;
        (job, m.generation, m.image_generation)
    };
    let weak = Rc::downgrade(shared);
    cx.spawn(async move |cx| {
        let done = mandara_services::runtime::unblock(move || run(job)).await;
        let Some(shared) = weak.upgrade() else { return };
        let (views, trash) = {
            let mut m = shared.borrow_mut();
            m.running = false;
            let trash = apply(&mut m, done, generation, image_generation);
            (m.views.clone(), trash)
        };
        cx.update(|cx| {
            for image in trash { cx.drop_image(image, None); }
            pump(&shared, cx);
        });
        views.notify_async(cx);
    }).detach();
}

enum Job { Api(ApiJob, (Box<str>, Option<Box<str>>)), Image(Request) }

/// The worker's side: one request, parsed off the UI thread.
fn run(job: Job) -> Done {
    let now = Instant::now();
    let job = match job {
        Job::Api(api, (client_id, username)) => {
            session().configure(&client_id, username.as_deref());
            api
        }
        Job::Image(request) => {
            let decoded = session_image(&request.url).and_then(|bytes| images::decode(bytes, request.width, request.height, request.blur));
            return Done::Image(request.key, decoded);
        }
    };
    match job {
        ApiJob::Page { query, after, count, want } => Done::Page(session().listing(&query, after.as_deref(), count, want, now)),
        ApiJob::Card { query, want } => {
            let page = session().listing(&query, None, 0, want, now);
            Done::Card(query, page)
        }
        ApiJob::Thread { id, permalink, want } => {
            let thread = session().thread(&id, permalink.as_deref(), want, now);
            Done::Thread(thread.map(|Thread { mut post, nodes, truncated, limited }| {
                // The parsed body replaces the Markdown source.
                let body = (!post.text.is_empty()).then(|| Body::parse(&post.text));
                post.text = Box::default();
                let mut rows = Vec::with_capacity(nodes.len());
                rows.extend(nodes.into_iter().map(Row::from_node));
                (Arc::new(post), body, rows, truncated, limited)
            }))
        }
        ApiJob::More { post, key, children, depth, room } => {
            let rows = session().more(&post, &children, |_| depth.checked_sub(1), room, now);
            Done::More(key, rows.map(|nodes| nodes.into_iter().map(Row::from_node).collect()))
        }
        ApiJob::Continue { post, key, parent, depth, room } => {
            let rows = session().continue_thread(&post, &parent, depth, room, now);
            Done::More(key, rows.map(|nodes| nodes.into_iter().map(Row::from_node).collect()))
        }
    }
}

/// Download, releasing the session before decoding.
fn session_image(url: &str) -> Option<Vec<u8>> {
    let result = session().image(url);
    result.ok()
}

/// Apply a result if it still belongs to what is shown. Returns pictures
/// to release from the GPU.
fn apply(m: &mut Model, done: Done, generation: u64, image_generation: u64) -> Vec<Arc<RenderImage>> {
    match done {
        Done::Image(key, decoded) => {
            if image_generation != m.image_generation { return decoded.map(|(image, _)| image).into_iter().collect(); }
            let (image, bytes) = decoded.map_or((None, 0), |(image, bytes)| (Some(image), bytes));
            m.images.finish(key, image, bytes)
        }
        Done::Card(query, result) => {
            if m.card.query.as_ref() != Some(&query) { return Vec::new(); }
            m.card.loading = false;
            m.card.loaded_at = Some(Instant::now());
            match result {
                Ok(page) => { m.card.page = Some(page); m.card.error = None; }
                Err(error) => m.card.error = Some(error),
            }
            Vec::new()
        }
        _ if generation != m.generation => Vec::new(),
        Done::Page(result) => {
            m.feed.loading = false;
            match result {
                Ok(page) => { m.feed.error = None; m.feed.loaded_at = Some(Instant::now()); m.feed.append(page); }
                Err(error) => {
                    m.feed.retry_at = match &error { Error::Wait(wait) => Some(Instant::now() + *wait), _ => None };
                    m.feed.error = Some(error);
                }
            }
            Vec::new()
        }
        Done::Thread(result) => {
            let Some(reader) = &mut m.reader else { return Vec::new() };
            reader.loading = false;
            match result {
                Ok((post, body, rows, truncated, limited)) => {
                    reader.post = Some(post);
                    reader.body = body;
                    reader.rows = rows;
                    reader.truncated = truncated;
                    reader.limited = limited;
                    reader.error = None;
                    reader.relayout();
                }
                Err(error) => reader.error = Some(error),
            }
            Vec::new()
        }
        Done::More(key, result) => {
            let Some(reader) = &mut m.reader else { return Vec::new() };
            if reader.loading_more.as_deref() != Some(&*key) { return Vec::new(); }
            reader.loading_more = None;
            match result {
                Ok(rows) => {
                    if let Some(at) = reader.rows.iter().position(|row| row.more_key() == Some(&*key)) {
                        let Row::More(stub) = &reader.rows[at] else { unreachable!() };
                        // A partial batch leaves the rest behind a new stub.
                        let rest: Vec<Box<str>> = stub.children.iter().skip(mandara_services::reddit::MORE_BATCH).cloned().collect();
                        let depth = stub.depth;
                        let parent = stub.parent.clone();
                        let count = stub.count.saturating_sub(rows.len() as u32);
                        let mut insert = rows;
                        if !rest.is_empty() { insert.push(Row::More(MoreRow { parent, children: rest, count, depth })); }
                        reader.rows.splice(at..=at, insert);
                    }
                    reader.error = None;
                    reader.relayout();
                }
                Err(error) => reader.error = Some(error),
            }
            Vec::new()
        }
    }
}

/// Is this community among `list`?
pub(super) fn subscribed(list: &[Subreddit], name: &str) -> bool { list.iter().any(|sub| sub.same(name)) }

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::reddit::{Sort, Span};

    const WANT: Want = Want { thumb: 140, media: 640 };

    fn page(ids: &[&str], after: Option<&str>) -> Arc<Listing> {
        Arc::new(Listing { posts: ids.iter().map(|id| Post { id: (*id).into(), title: "A post".into(), ..Post::default() }).collect(), after: after.map(Into::into), limited: false })
    }

    fn query(name: &str) -> Query { Query { subreddit: Subreddit::parse(name).unwrap(), sort: Sort::Hot, span: Span::Day, search: None } }

    #[test]
    fn pages_append_without_repeats_and_stop_at_the_cap() {
        let mut feed = Feed::default();
        feed.reset(Some(query("rust")));
        feed.append(page(&["a", "b", "c"], Some("t3_c")));
        feed.append(page(&["c", "d", "a", "e"], Some("t3_e")));
        let ids: Vec<&str> = (0..feed.len()).map(|row| &*feed.post(row).unwrap().id).collect();
        assert_eq!(ids, ["a", "b", "c", "d", "e"]);
        assert_eq!(feed.items(), 6, "posts plus the footer");
        assert!(feed.more());
        for _ in 2..MAX_PAGES { feed.append(page(&["z"], Some("t3_z"))); }
        assert!(!feed.more(), "the list stops growing after {MAX_PAGES} pages");
        feed.append(page(&[], Some("t3_y")));
        assert!(feed.after.is_none(), "an empty page ends the list");
    }

    #[test]
    fn navigation_makes_older_results_stale_and_coalesces_requests() {
        let mut m = Model::default();
        m.show(query("rust"), WANT);
        let first = m.generation;
        m.show(query("linux"), WANT);
        assert!(matches!(&m.api, Some(ApiJob::Page { query, .. }) if query.subreddit.as_str() == "linux"), "only the newest request waits");
        let stale = apply(&mut m, Done::Page(Ok(page(&["old"], None))), first, 0);
        assert!(stale.is_empty());
        assert_eq!(m.feed.len(), 0, "a result for the earlier list is dropped");
        let current = m.generation;
        apply(&mut m, Done::Page(Ok(page(&["new"], None))), current, 0);
        assert_eq!(m.feed.len(), 1);
        m.show(query("linux"), WANT);
        assert!(m.api.is_none() || matches!(m.api, Some(ApiJob::Page { .. })));
        let generation = m.generation;
        m.open_post("new", WANT);
        assert!(m.generation > generation);
        m.back();
        assert!(m.api.is_none(), "leaving a post cancels its waiting request");
        let post = Arc::new(Post { id: "new".into(), title: "A post".into(), ..Post::default() });
        apply(&mut m, Done::Thread(Ok((post, None, vec![comment("late", 0)], false, false))), generation + 1, 0);
        assert!(m.reader.as_ref().unwrap().rows.is_empty(), "a thread that arrives after going back is dropped");
    }

    fn comment(id: &str, depth: u8) -> Row {
        Row::Comment(CommentRow { id: id.into(), author: "a".into(), score: 1, score_hidden: false, created: 0, depth, op: false, stickied: false, distinguished: false, edited: false, body: Body::parse(id) })
    }

    #[test]
    fn folding_hides_replies_and_more_rows_splice_in_place() {
        let mut m = Model::default();
        m.open_post("p", WANT);
        let reader = m.reader.as_mut().unwrap();
        reader.rows = vec![comment("a", 0), comment("b", 1), comment("c", 2), comment("d", 0), Row::More(MoreRow { parent: "t3_p".into(), children: vec!["e".into(), "f".into()], count: 2, depth: 0 })];
        reader.relayout();
        assert_eq!(reader.visible, [0, 1, 2, 3, 4]);
        reader.collapsed.insert("a".into());
        reader.relayout();
        assert_eq!(reader.visible, [0, 3, 4]);
        assert_eq!(reader.hidden_under(0), 2);
        m.expand(4);
        assert!(matches!(&m.api, Some(ApiJob::More { children, .. }) if children.len() == 2));
        let generation = m.generation;
        apply(&mut m, Done::More("e".into(), Ok(vec![comment("e", 0), comment("f", 1)])), generation, 0);
        let reader = m.reader.as_ref().unwrap();
        assert_eq!(reader.rows.len(), 6, "the stub is replaced by its replies");
        assert!(reader.rows.iter().all(|row| matches!(row, Row::Comment(_))));
        assert!(reader.loading_more.is_none());
    }

    #[test]
    fn grid_geometry_fits_two_or_three_columns() {
        assert_eq!(grid_columns(428.0), 2, "the usual panel");
        assert_eq!(grid_columns(588.0), 3, "the widened grid panel");
        assert_eq!(grid_columns(120.0), 2, "never fewer than two");
        assert_eq!(grid_columns(2000.0), 3, "never more than three");
        let tile = tile_width(588.0, 3);
        assert!(tile >= GRID_MIN_TILE && tile * 3.0 + GRID_GAP * 2.0 <= 588.0, "{tile}");
        assert_eq!(tile_width(428.0, 2), 210.0);
    }

    #[test]
    fn grid_rows_skip_hidden_posts_and_rebuild_only_on_change() {
        let mut feed = Feed::default();
        feed.reset(Some(query("rust")));
        let mut posts = page(&["a", "b", "c", "d", "e"], None);
        Arc::get_mut(&mut posts).unwrap().posts[1].nsfw = true;
        feed.append(posts);
        let mut grid = Grid::default();
        grid.sync(&feed, |post| post.nsfw, true, 2);
        assert_eq!(grid.posts, [0, 2, 3, 4]);
        assert_eq!(grid.rows(), 2);
        assert_eq!(grid.row(0), [0, 2]);
        assert_eq!(grid.row(1), [3, 4]);
        assert!(grid.row(5).is_empty());
        grid.sync(&feed, |_| false, false, 3);
        assert_eq!((grid.rows(), grid.row(0)), (2, &[0, 1, 2][..]), "showing NSFW and a third column");
        let before = grid.posts.as_ptr();
        grid.sync(&feed, |_| unreachable!("unchanged posts are not filtered again"), false, 3);
        assert_eq!(grid.posts.as_ptr(), before);
    }

    #[test]
    fn the_sidebar_card_asks_once_then_waits_for_its_interval() {
        let mut m = Model::default();
        let now = Instant::now();
        let every = Duration::from_secs(600);
        assert_eq!(m.want_card(query("rust"), WANT, every, now), None);
        assert!(matches!(&m.card_job, Some(ApiJob::Card { query, .. }) if query.subreddit.as_str() == "rust"));
        m.card_job = None;
        assert_eq!(m.want_card(query("rust"), WANT, every, now), None, "a request in flight is not repeated");
        assert!(m.card_job.is_none());
        apply(&mut m, Done::Card(query("rust"), Ok(page(&["a"], None))), 0, 0);
        let loaded = m.card.loaded_at.unwrap();
        assert!(m.want_card(query("rust"), WANT, every, loaded + Duration::from_secs(60)).is_some_and(|wait| wait <= Duration::from_secs(540)));
        assert!(m.card_job.is_none());
        apply(&mut m, Done::Card(query("linux"), Ok(page(&["late"], None))), 0, 0);
        assert_eq!(&*m.card.page.as_ref().unwrap().posts[0].id, "a", "a result for another community is dropped");
        m.want_card(query("linux"), WANT, every, loaded);
        assert!(m.card.page.is_none() && m.card_job.is_some(), "a new community starts over");
        m.drop_card();
        assert!(m.card.query.is_none() && m.card_job.is_none());
    }

    #[test]
    fn closing_late_pictures_are_returned_for_release_not_kept() {
        let mut m = Model::default();
        let key = images::Key::new("https://i.redd.it/a.png", 4, 4, false);
        m.images.get(|| Request { key, url: "https://i.redd.it/a.png".into(), width: 4, height: 4, blur: false }, key);
        let old = m.image_generation;
        m.image_generation += 1;
        drop(m.images.clear());
        let image = crate::plugins::render_image::PreparedImage::new().finish(image::RgbaImage::new(1, 1));
        let current = m.generation;
        let trash = apply(&mut m, Done::Image(key, Some((image, 4))), current, old);
        assert_eq!(trash.len(), 1);
        assert_eq!(m.images.len(), 0);
    }
}
