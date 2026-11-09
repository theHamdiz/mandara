//! OAuth, bounded requests, the request budget and a brief reply cache.

use std::io::Read as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use mandara_i18n::{t, tf};
use serde::Serialize;
use zeroize::Zeroizing;

use super::budget::{Budget, Limits};
use super::{Listing, MAX_COMMENTS, Node, Query, Thread, Want};

/// The documented application-only grant for apps without a secret.
const INSTALLED_CLIENT: &str = "https://oauth.reddit.com/grants/installed_client";
/// Reddit's opt-out device id: the token is not tied to this computer.
const DEVICE_ID: &str = "DO_NOT_TRACK_THIS_DEVICE";
const OFFICIAL_API: &str = "https://oauth.reddit.com";
const OFFICIAL_TOKEN: &str = "https://www.reddit.com/api/v1/access_token";
/// Point the plugin at a local test server (loopback only).
const API_OVERRIDE: &str = "MANDARA_REDDIT_API";
/// A thread page with 500 comments is about half a MiB; listings are far
/// smaller. Images are previews, never originals.
const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;
const MAX_TOKEN_BYTES: usize = 16 * 1024;
pub const MAX_IMAGE_BYTES: usize = 6 * 1024 * 1024;
/// A stalled request must not hold the plugin's only worker.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Renew a token this long before Reddit says it expires.
const TOKEN_MARGIN: Duration = Duration::from_secs(60);
/// Reddit's tokens last an hour; trust a shorter reply, never a longer one.
const MAX_TOKEN_LIFE: Duration = Duration::from_secs(3600);
/// Reopening a community you just read reuses its page.
const FRESH_FOR: Duration = Duration::from_secs(120);
const CACHED_PAGES: usize = 8;
/// Limited mode can ask once a minute, so its replies are kept longer.
const RSS_FRESH_FOR: Duration = Duration::from_secs(300);
const RSS_CACHED_THREADS: usize = 4;
const OFFICIAL_SITE: &str = "https://www.reddit.com";

/// Limited mode, shared by every session of the shell: the one-a-minute
/// queue and whether the last request had to use it. Held only briefly,
/// so the panel can read it without waiting for a request.
#[derive(Debug, Default)]
pub struct Limited {
    pub pacer: super::Pacer,
    /// The last request went to the public feeds.
    pub active: bool,
}

/// The shell's one limited-mode queue.
pub fn limited() -> &'static parking_lot::Mutex<Limited> {
    static LIMITED: parking_lot::Mutex<Limited> = parking_lot::Mutex::new(Limited { pacer: super::Pacer::new(), active: false });
    &LIMITED
}
/// Client ids are short; anything else was pasted by mistake.
const MAX_CLIENT_ID: usize = 64;

/// Where requests go: Reddit, or a loopback test server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    pub api: String,
    pub token: String,
    /// The public site, for limited mode's feeds.
    pub site: String,
}

impl Default for Endpoints {
    fn default() -> Self { Self { api: OFFICIAL_API.into(), token: OFFICIAL_TOKEN.into(), site: OFFICIAL_SITE.into() } }
}

impl Endpoints {
    /// Reddit, unless `MANDARA_REDDIT_API` names a loopback test server.
    pub fn from_env() -> Self {
        std::env::var(API_OVERRIDE).ok().and_then(|base| Self::local(&base)).unwrap_or_default()
    }

    pub(super) fn local(base: &str) -> Option<Self> {
        let base = base.trim().trim_end_matches('/');
        let host = base.strip_prefix("http://")?.split(['/', ':']).next()?;
        matches!(host, "127.0.0.1" | "localhost" | "[::1]").then(|| Self { api: base.into(), token: format!("{base}/api/v1/access_token"), site: base.into() })
    }
}

/// Reddit asks for `<platform>:<app id>:<version> (by /u/<name>)`.
pub fn user_agent(username: Option<&str>) -> String {
    let version = env!("CARGO_PKG_VERSION");
    match username.filter(|name| (3..=20).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')) {
        Some(name) => format!("linux:org.mandara.shell:v{version} (by /u/{name})"),
        None => format!("linux:org.mandara.shell:v{version} (+https://github.com/theHamdiz/mandara)"),
    }
}

/// What went wrong, so the reader can show the right state.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// No client id yet.
    Setup,
    /// Reddit refused the client id (unknown, or not approved yet).
    Rejected,
    /// Wait this long before asking again.
    Wait(Duration),
    /// The community or post is private, banned or gone.
    Missing { private: bool },
    Offline(String),
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text: String = match self {
            Self::Setup => t("Add your Reddit app's client id in Studio to start reading").into(),
            Self::Rejected => t("Reddit did not accept this client id. Check it in Studio, and that the app is an installed app").into(),
            Self::Wait(wait) => tf("Reddit asked Mandara to slow down. Trying again in {seconds} s", &[("seconds", &wait.as_secs().max(1))]),
            Self::Missing { private: true } => t("This community is private").into(),
            Self::Missing { private: false } => t("This community or post isn't available").into(),
            Self::Offline(message) | Self::Failed(message) => message.clone(),
        };
        f.write_str(&text)
    }
}

/// One reply: status, rate-limit headers and a bounded body.
pub struct Response {
    pub status: u16,
    pub limits: Limits,
    pub body: Vec<u8>,
}

/// The network, replaceable in tests.
pub trait Transport: Send {
    fn get(&self, url: &str, bearer: Option<&str>, agent: &str, max: usize) -> Result<Response, String>;
    fn token(&self, url: &str, client_id: &str, agent: &str) -> Result<Response, String>;
}

pub struct Http { agent: ureq::Agent }

impl Default for Http {
    fn default() -> Self {
        Self { agent: ureq::Agent::config_builder().timeout_global(Some(REQUEST_TIMEOUT)).max_redirects(0).http_status_as_error(false).build().into() }
    }
}

impl Http {
    fn read(mut response: ureq::http::Response<ureq::Body>, max: usize) -> Result<Response, String> {
        let status = response.status().as_u16();
        let limits = Limits::from_headers(|name| response.headers().get(name).and_then(|value| value.to_str().ok()));
        let mut body = Vec::new();
        response.body_mut().as_reader().take((max + 1) as u64).read_to_end(&mut body).map_err(|error| failed(&error))?;
        if body.len() > max { return Err(t("Reddit's reply was too large").into()); }
        Ok(Response { status, limits, body })
    }
}

fn failed(error: &dyn std::fmt::Display) -> String { tf("Could not reach Reddit: {error}", &[("error", error)]) }

impl Transport for Http {
    fn get(&self, url: &str, bearer: Option<&str>, agent: &str, max: usize) -> Result<Response, String> {
        let mut request = self.agent.get(url).header("User-Agent", agent);
        let auth = bearer.map(|token| Zeroizing::new(format!("bearer {token}")));
        if let Some(auth) = &auth { request = request.header("Authorization", auth.as_str()); }
        Self::read(request.call().map_err(|error| failed(&error))?, max)
    }

    fn token(&self, url: &str, client_id: &str, agent: &str) -> Result<Response, String> {
        let basic = Zeroizing::new(format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{client_id}:"))));
        let form = format!("grant_type={}&device_id={DEVICE_ID}", INSTALLED_CLIENT.replace(':', "%3A").replace('/', "%2F"));
        let response = self.agent.post(url).header("User-Agent", agent).header("Authorization", basic.as_str())
            .header("Content-Type", "application/x-www-form-urlencoded").send(form.as_str()).map_err(|error| failed(&error))?;
        Self::read(response, MAX_TOKEN_BYTES)
    }
}

struct Token {
    value: Zeroizing<String>,
    expires: Instant,
}

#[derive(serde::Deserialize)]
struct TokenReply {
    access_token: Option<String>,
    expires_in: Option<f64>,
}

/// A few recent replies, newest first, each kept for `ttl`.
struct Recent<T> { items: Vec<(String, Instant, Arc<T>)>, cap: usize, ttl: Duration }

impl<T> Recent<T> {
    const fn new(cap: usize, ttl: Duration) -> Self { Self { items: Vec::new(), cap, ttl } }

    fn get(&mut self, key: &str, now: Instant) -> Option<Arc<T>> {
        let ttl = self.ttl;
        self.items.retain(|(_, at, _)| now.saturating_duration_since(*at) < ttl);
        let at = self.items.iter().position(|(saved, _, _)| saved == key)?;
        let item = self.items.remove(at);
        let value = item.2.clone();
        self.items.insert(0, item);
        Some(value)
    }

    fn put(&mut self, key: String, value: Arc<T>, now: Instant) {
        self.items.retain(|(saved, _, _)| *saved != key);
        if self.items.capacity() == 0 { self.items.reserve_exact(self.cap); }
        self.items.truncate(self.cap.saturating_sub(1));
        self.items.insert(0, (key, now, value));
    }

    fn clear(&mut self) {
        self.items.clear();
        self.items.shrink_to_fit();
    }

    fn len(&self) -> usize { self.items.len() }
}

/// What `reddit status` reports.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Status {
    pub configured: bool,
    pub authorized: bool,
    pub remaining: Option<f32>,
    pub used: Option<f32>,
    pub resets_in_seconds: Option<u64>,
    pub paused_for_seconds: Option<u64>,
    pub cached_pages: usize,
    /// Reading the public feeds (no app, or Reddit refused it).
    pub limited: bool,
    /// Limited mode's next request waits this long.
    pub limited_wait_seconds: Option<u64>,
}

/// The plugin's connection: one per shell, used by one worker at a time.
pub struct Session<T: Transport = Http> {
    transport: T,
    endpoints: Endpoints,
    agent: String,
    client_id: Option<Zeroizing<String>>,
    token: Option<Token>,
    /// Reddit refused this client id: read the public feeds until it changes.
    refused: bool,
    budget: Budget,
    pages: Recent<Listing>,
    rss_pages: Recent<Listing>,
    rss_threads: Recent<Thread>,
    /// A private queue (tests); the shell shares [`limited`].
    own_queue: Option<Box<parking_lot::Mutex<Limited>>>,
}

impl Default for Session<Http> {
    fn default() -> Self {
        let mut session = Self::new(Http::default(), Endpoints::from_env());
        session.own_queue = None;
        session
    }
}

impl<T: Transport> Session<T> {
    pub fn new(transport: T, endpoints: Endpoints) -> Self {
        Self {
            transport, endpoints, agent: user_agent(None), client_id: None, token: None, refused: false, budget: Budget::default(),
            pages: Recent::new(CACHED_PAGES, FRESH_FOR), rss_pages: Recent::new(CACHED_PAGES, RSS_FRESH_FOR), rss_threads: Recent::new(RSS_CACHED_THREADS, RSS_FRESH_FOR),
            own_queue: Some(Box::default()),
        }
    }

    fn queue(&self) -> &parking_lot::Mutex<Limited> { self.own_queue.as_deref().unwrap_or(limited()) }

    /// Whether requests go to the public feeds now.
    pub fn is_limited(&self) -> bool { self.client_id.is_none() || self.refused }

    /// Use this client id (and user agent). A new id drops the token and
    /// every cached reply.
    pub fn configure(&mut self, client_id: &str, username: Option<&str>) {
        let id = client_id.trim();
        let id = (!id.is_empty() && id.len() <= MAX_CLIENT_ID && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).then_some(id);
        if self.client_id.as_deref().map(String::as_str) != id {
            self.client_id = id.map(|id| Zeroizing::new(id.to_owned()));
            self.token = None;
            self.refused = false;
            self.release();
        }
        self.agent = user_agent(username);
    }

    /// Forget cached replies (the reader closed).
    pub fn release(&mut self) {
        self.pages.clear();
        self.rss_pages.clear();
        self.rss_threads.clear();
    }

    /// Also forget the token (the plugin was switched off).
    pub fn stop(&mut self) {
        self.release();
        self.token = None;
    }

    pub fn status(&self, now: Instant) -> Status {
        Status {
            configured: self.client_id.is_some(),
            authorized: self.token.as_ref().is_some_and(|token| token.expires > now),
            remaining: self.budget.remaining(now),
            used: self.budget.used(),
            resets_in_seconds: self.budget.resets_in(now).map(|wait| wait.as_secs()),
            paused_for_seconds: self.budget.wait(now).map(|wait| wait.as_secs().max(1)),
            cached_pages: self.pages.len() + self.rss_pages.len(),
            limited: self.is_limited(),
            limited_wait_seconds: self.queue().lock().pacer.wait(now).map(|wait| wait.as_secs().max(1)),
        }
    }

    /// Whether the official API can be used now: a token is at hand or
    /// renewed. A refused client id switches to limited mode for good (until
    /// it changes); a failed renewal uses limited mode for this request.
    fn oauth(&mut self, now: Instant) -> Result<bool, Error> {
        if self.is_limited() { return Ok(false); }
        match self.bearer(now) {
            Ok(_) => Ok(true),
            Err(Error::Rejected | Error::Setup) => { self.refused = true; Ok(false) }
            Err(Error::Wait(_) | Error::Failed(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// One feed request: never before the queue allows, and the reply's
    /// headers (or a 429) hold the queue for the rest of the window.
    fn feed(&mut self, path: &str, now: Instant) -> Result<Vec<u8>, Error> {
        {
            let mut queue = self.queue().lock();
            queue.active = true;
            if let Some(wait) = queue.pacer.wait(now) { return Err(Error::Wait(wait)); }
            crate::offline::check().map_err(Error::Offline)?;
            queue.pacer.sent(now);
        }
        let reply = self.transport.get(&format!("{}{path}", self.endpoints.site), None, &self.agent, MAX_REPLY_BYTES).map_err(Error::Failed)?;
        let mut queue = self.queue().lock();
        queue.pacer.observe(reply.status, reply.limits, now);
        match reply.status {
            200 => Ok(reply.body),
            403 => Err(Error::Missing { private: true }),
            429 => Err(Error::Wait(queue.pacer.wait(now).unwrap_or(super::rss::RSS_INTERVAL))),
            status if status == 404 || (300..400).contains(&status) => Err(Error::Missing { private: false }),
            status => Err(Error::Failed(tf("Reddit replied with HTTP {status}", &[("status", &status)]))),
        }
    }

    fn feed_listing(&mut self, query: &Query, after: Option<&str>, now: Instant) -> Result<Arc<Listing>, Error> {
        let path = super::rss::listing_path(query, after);
        if let Some(page) = self.rss_pages.get(&path, now) { return Ok(page); }
        let body = self.feed(&path, now)?;
        let page = Arc::new(super::rss::listing(&body).map_err(Error::Failed)?);
        self.rss_pages.put(path, page.clone(), now);
        Ok(page)
    }

    fn bearer(&mut self, now: Instant) -> Result<Zeroizing<String>, Error> {
        if let Some(token) = self.token.as_ref().filter(|token| token.expires > now + TOKEN_MARGIN) {
            return Ok(token.value.clone());
        }
        self.token = None;
        let client_id = self.client_id.clone().ok_or(Error::Setup)?;
        crate::offline::check().map_err(Error::Offline)?;
        let reply = self.transport.token(&self.endpoints.token, &client_id, &self.agent).map_err(Error::Failed)?;
        match reply.status {
            200 => {}
            400 | 401 | 403 => return Err(Error::Rejected),
            429 => {
                self.budget.observe(429, reply.limits, now);
                return Err(Error::Wait(self.budget.wait(now).unwrap_or_default()));
            }
            status => return Err(Error::Failed(tf("Reddit replied with HTTP {status}", &[("status", &status)]))),
        }
        let parsed: TokenReply = serde_json::from_slice(&reply.body).map_err(|_| Error::Rejected)?;
        let value = Zeroizing::new(parsed.access_token.filter(|token| !token.is_empty() && token.len() < MAX_TOKEN_BYTES).ok_or(Error::Rejected)?);
        let life = parsed.expires_in.filter(|secs| secs.is_finite() && *secs > 0.0).map_or(MAX_TOKEN_LIFE, |secs| Duration::from_secs_f64(secs.min(MAX_TOKEN_LIFE.as_secs_f64())));
        self.token = Some(Token { value: value.clone(), expires: now + life });
        Ok(value)
    }

    /// One API request with the budget, a single token renewal on 401 and
    /// the reply mapped to an [`Error`] the reader can explain.
    fn request(&mut self, path: &str, now: Instant) -> Result<Vec<u8>, Error> {
        if let Some(wait) = self.budget.wait(now) { return Err(Error::Wait(wait)); }
        let url = format!("{}{path}", self.endpoints.api);
        for attempt in 0..2 {
            let bearer = self.bearer(now)?;
            crate::offline::check().map_err(Error::Offline)?;
            let reply = self.transport.get(&url, Some(&bearer), &self.agent, MAX_REPLY_BYTES).map_err(Error::Failed)?;
            self.budget.observe(reply.status, reply.limits, now);
            match reply.status {
                200 => return Ok(reply.body),
                401 if attempt == 0 => self.token = None,
                401 => return Err(Error::Rejected),
                403 => return Err(Error::Missing { private: reason(&reply.body).as_deref() == Some("private") }),
                404 => return Err(Error::Missing { private: false }),
                429 => return Err(Error::Wait(self.budget.wait(now).unwrap_or_default())),
                status if (300..400).contains(&status) => return Err(Error::Missing { private: false }),
                status => return Err(Error::Failed(tf("Reddit replied with HTTP {status}", &[("status", &status)]))),
            }
        }
        Err(Error::Rejected)
    }

    /// A page of posts, from the API or (limited mode) the public feeds. A
    /// page read in the last two minutes (five in limited mode) is reused.
    pub fn listing(&mut self, query: &Query, after: Option<&str>, count: usize, want: Want, now: Instant) -> Result<Arc<Listing>, Error> {
        if self.oauth(now)? {
            self.queue().lock().active = false;
            match self.api_listing(query, after, count, want, now) {
                Err(Error::Rejected) => self.refused = true,
                other => return other,
            }
        }
        self.feed_listing(query, after, now)
    }

    fn api_listing(&mut self, query: &Query, after: Option<&str>, count: usize, want: Want, now: Instant) -> Result<Arc<Listing>, Error> {
        let path = query.path(after, count);
        let key = format!("{path}#{}x{}", want.thumb, want.media);
        if let Some(page) = self.pages.get(&key, now) { return Ok(page); }
        let body = self.request(&path, now)?;
        let page = Arc::new(super::listing(&body, want).map_err(Error::Failed)?);
        drop(body);
        self.pages.put(key, page.clone(), now);
        Ok(page)
    }

    /// A post and its comments. Not cached here: the reader keeps the open
    /// thread itself (already parsed), so a copy would only double it.
    /// In limited mode the comments feed is flat and kept five minutes.
    pub fn thread(&mut self, post: &str, permalink: Option<&str>, want: Want, now: Instant) -> Result<Thread, Error> {
        let post = super::post_id(post).ok_or(Error::Missing { private: false })?;
        if self.oauth(now)? {
            self.queue().lock().active = false;
            match self.request(&super::thread_path(post), now) {
                Ok(body) => return super::thread(&body, want).map_err(Error::Failed),
                Err(Error::Rejected) => self.refused = true,
                Err(error) => return Err(error),
            }
        }
        let path = super::rss::comments_path(post, permalink);
        if let Some(thread) = self.rss_threads.get(&path, now) { return Ok((*thread).clone()); }
        let body = self.feed(&path, now)?;
        let thread = super::rss::thread(&body).map_err(Error::Failed)?;
        self.rss_threads.put(path, Arc::new(thread.clone()), now);
        Ok(thread)
    }

    /// Hidden replies; `depth_of` gives the depth of comments already shown.
    pub fn more(&mut self, post: &str, children: &[Box<str>], depth_of: impl Fn(&str) -> Option<u8>, room: usize, now: Instant) -> Result<Vec<Node>, Error> {
        let post = super::post_id(post).ok_or(Error::Missing { private: false })?;
        let body = self.request(&super::more_path(post, children), now)?;
        super::more_children(&body, depth_of, room.min(MAX_COMMENTS)).map_err(Error::Failed)
    }

    /// The replies under `comment` that the thread page cut off.
    pub fn continue_thread(&mut self, post: &str, comment: &str, depth: u8, room: usize, now: Instant) -> Result<Vec<Node>, Error> {
        let post = super::post_id(post).ok_or(Error::Missing { private: false })?;
        let comment = comment.strip_prefix("t1_").unwrap_or(comment);
        if comment.is_empty() || !comment.bytes().all(|b| b.is_ascii_alphanumeric()) { return Err(Error::Missing { private: false }); }
        let body = self.request(&super::continue_path(post, comment), now)?;
        super::parse::continued(&body, comment, depth, room.min(MAX_COMMENTS)).map_err(Error::Failed)
    }

    /// A preview image from one of Reddit's image hosts (not rate limited).
    pub fn image(&self, url: &str) -> Result<Vec<u8>, String> {
        // Callers pass addresses the parser already filtered; check again so
        // no future caller can make this fetch an arbitrary host.
        let (Some(rest), Some(_)) = (url.strip_prefix("https://"), super::parse::image_url(url)) else {
            return Err(t("Only Reddit's own images are loaded").into());
        };
        // A loopback test server serves the pictures too, under `/media/<host>/…`.
        let url = if self.endpoints.api == OFFICIAL_API { url.to_owned() } else { format!("{}/media/{rest}", self.endpoints.api) };
        crate::offline::check()?;
        let reply = self.transport.get(&url, None, &self.agent, MAX_IMAGE_BYTES)?;
        if reply.status != 200 { return Err(tf("Reddit replied with HTTP {status}", &[("status", &reply.status)])); }
        Ok(reply.body)
    }
}

/// Reddit explains a 403 with `{"reason": "private"}` and similar.
fn reason(body: &[u8]) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Reason { reason: Option<String> }
    serde_json::from_slice::<Reason>(body).ok()?.reason
}
