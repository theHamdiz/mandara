//! Reddit's JSON into compact, bounded posts and comment rows. Only the
//! fields the reader shows are kept; text is bounded on character
//! boundaries and control characters are dropped.

use std::collections::HashMap;

use mandara_i18n::t;
use serde::Deserialize;
use serde::de::IgnoredAny;

use super::{Comment, Image, Kind, Listing, MAX_COMMENTS, MAX_GALLERY, More, Node, Post, Thread, Want};

/// Reddit's own limits: titles, self text and comment bodies.
const MAX_TITLE: usize = 300;
const MAX_SELF_TEXT: usize = 40_000;
const MAX_BODY: usize = 10_000;
/// Names, flair and domains are short; longer values are not Reddit's.
const MAX_SHORT: usize = 64;
const MAX_URL: usize = 2048;
/// Hidden reply ids kept per stub; Reddit loads them in batches.
const MAX_MORE_IDS: usize = 500;
/// Only Reddit's own image hosts are fetched for previews.
const IMAGE_HOSTS: [&str; 7] = ["i.redd.it", "preview.redd.it", "external-preview.redd.it", "a.thumbs.redditmedia.com", "b.thumbs.redditmedia.com", "i.redditmedia.com", "g.redditmedia.com"];

#[derive(Deserialize)]
struct RawListing {
    #[serde(default)]
    data: RawListingData,
}

#[derive(Default, Deserialize)]
struct RawListingData {
    #[serde(default)]
    children: Vec<RawThing>,
    #[serde(default)]
    after: Option<Box<str>>,
}

#[derive(Deserialize)]
struct RawThing {
    kind: Box<str>,
    #[serde(default)]
    data: RawItem,
}

/// `replies` is an empty string when a comment has none. Read directly
/// (not as an untagged enum, which would buffer every subtree once per level).
#[derive(Default)]
enum Replies {
    Listing(Box<RawListing>),
    #[default]
    None,
}

impl<'de> Deserialize<'de> for Replies {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visit;
        impl<'de> serde::de::Visitor<'de> for Visit {
            type Value = Replies;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("a listing or an empty string") }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Replies, E> { Ok(Replies::None) }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Replies, E> { Ok(Replies::None) }
            fn visit_none<E: serde::de::Error>(self) -> Result<Replies, E> { Ok(Replies::None) }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<Replies, A::Error> {
                RawListing::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(|listing| Replies::Listing(Box::new(listing)))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, seq: A) -> Result<Replies, A::Error> {
                IgnoredAny::deserialize(serde::de::value::SeqAccessDeserializer::new(seq)).map(|_| Replies::None)
            }
        }
        deserializer.deserialize_any(Visit)
    }
}

/// Every field of posts, comments and "more" stubs the reader needs; the
/// rest of each object is skipped without being stored.
#[derive(Default, Deserialize)]
#[serde(default)]
struct RawItem {
    id: Box<str>,
    title: Box<str>,
    author: Box<str>,
    subreddit: Box<str>,
    link_flair_text: Option<Box<str>>,
    domain: Box<str>,
    permalink: Box<str>,
    url: Option<Box<str>>,
    selftext: Box<str>,
    body: Box<str>,
    score: f64,
    num_comments: f64,
    created_utc: f64,
    over_18: bool,
    spoiler: bool,
    stickied: bool,
    locked: bool,
    is_self: bool,
    is_video: bool,
    is_gallery: bool,
    post_hint: Option<Box<str>>,
    thumbnail: Option<Box<str>>,
    thumbnail_width: Option<f64>,
    thumbnail_height: Option<f64>,
    preview: Option<RawPreview>,
    gallery_data: Option<RawGallery>,
    media_metadata: Option<HashMap<Box<str>, RawMeta>>,
    replies: Replies,
    parent_id: Box<str>,
    count: f64,
    depth: f64,
    children: Vec<Box<str>>,
    is_submitter: bool,
    distinguished: Option<Box<str>>,
    edited: Edited,
    score_hidden: bool,
}

/// `edited` is `false` or the time of the edit.
#[derive(Default, Deserialize)]
#[serde(untagged)]
enum Edited {
    #[default]
    Never,
    Flag(bool),
    At(IgnoredAny),
}

impl Edited {
    fn yes(&self) -> bool { matches!(self, Self::Flag(true) | Self::At(_)) }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawPreview { images: Vec<RawPreviewImage> }

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawPreviewImage { source: Option<RawSize>, resolutions: Vec<RawSize> }

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct RawSize { url: Box<str>, width: f64, height: f64 }

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawGallery { items: Vec<RawGalleryItem> }

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawGalleryItem { media_id: Box<str> }

/// A gallery image: previews `p` and source `s`, with `u`, `x` and `y`.
#[derive(Default, Deserialize)]
#[serde(default)]
struct RawMeta { status: Box<str>, e: Box<str>, p: Vec<RawMetaSize>, s: Option<RawMetaSize> }

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct RawMetaSize { u: Option<Box<str>>, x: f64, y: f64 }

/// Keep text within `max` characters, without control characters other
/// than line breaks and tabs.
pub(super) fn bounded(text: &str, max: usize) -> Box<str> {
    let mut out = String::with_capacity(text.len().min(max * 4));
    let mut chars = text.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t');
    out.extend(chars.by_ref().take(max));
    if chars.next().is_some() { out.push('…'); }
    out.into_boxed_str()
}

/// One line: titles, names and flair never wrap on their own.
fn line(text: &str, max: usize) -> Box<str> {
    let text = bounded(text, max);
    if text.contains(['\n', '\t']) { text.replace(['\n', '\t'], " ").trim().into() } else { text.trim().into() }
}

/// An https address on one of Reddit's image hosts.
pub(super) fn image_url(url: &str) -> Option<Box<str>> {
    if url.len() > MAX_URL { return None; }
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next()?;
    IMAGE_HOSTS.iter().any(|known| host.eq_ignore_ascii_case(known)).then(|| format!("https://{rest}").into_boxed_str())
}

/// The smallest size at least `want` pixels wide, else the largest there is.
fn pick<'a>(sizes: impl Iterator<Item = (&'a str, f64, f64)>, want: u32) -> Option<Image> {
    let mut best: Option<(&str, f64, f64)> = None;
    for (url, width, height) in sizes {
        if width <= 0.0 || height <= 0.0 || image_url(url).is_none() { continue; }
        best = match best {
            None => Some((url, width, height)),
            Some(current) => {
                let fits = width >= f64::from(want);
                let current_fits = current.1 >= f64::from(want);
                let better = match (fits, current_fits) {
                    (true, true) => width < current.1,
                    (true, false) => true,
                    (false, true) => false,
                    (false, false) => width > current.1,
                };
                Some(if better { (url, width, height) } else { current })
            }
        };
    }
    best.and_then(|(url, width, height)| Some(Image { url: image_url(url)?, width: width as u32, height: height as u32 }))
}

fn preview(item: &RawItem, want: u32) -> Option<Image> {
    let image = item.preview.as_ref()?.images.first()?;
    pick(image.resolutions.iter().chain(&image.source).map(|size| (&*size.url, size.width, size.height)), want)
}

fn gallery(item: &RawItem, want: u32) -> Vec<Image> {
    let (Some(data), Some(meta)) = (&item.gallery_data, &item.media_metadata) else { return Vec::new() };
    data.items.iter().filter_map(|entry| {
        let meta = meta.get(&entry.media_id).filter(|meta| &*meta.status == "valid" && matches!(&*meta.e, "Image" | "AnimatedImage"))?;
        let sizes = meta.p.iter().chain(meta.s.as_ref().filter(|_| &*meta.e == "Image"));
        pick(sizes.filter_map(|size| Some((size.u.as_deref()?, size.x, size.y))), want)
    }).take(MAX_GALLERY).collect()
}

fn post(item: RawItem, want: Want, keep_text: bool) -> Option<Post> {
    let id = super::post_id(&item.id)?.into();
    let title = line(&item.title, MAX_TITLE);
    if title.is_empty() { return None; }
    let media = if item.is_gallery { gallery(&item, want.media) } else { preview(&item, want.media).into_iter().collect() };
    let kind = if item.is_gallery && !media.is_empty() {
        Kind::Gallery
    } else if item.is_video || item.post_hint.as_deref().is_some_and(|hint| hint.ends_with("video")) {
        Kind::Video
    } else if item.is_self {
        Kind::Text
    } else if item.post_hint.as_deref() == Some("image") && !media.is_empty() {
        Kind::Image
    } else {
        Kind::Link
    };
    let thumb = if item.is_gallery {
        gallery(&item, want.thumb).into_iter().next()
    } else {
        preview(&item, want.thumb)
    }.or_else(|| {
        let url = item.thumbnail.as_deref()?;
        pick(std::iter::once((url, item.thumbnail_width.unwrap_or(0.0), item.thumbnail_height.unwrap_or(0.0))), want.thumb)
    });
    let url = item.url.as_deref().filter(|url| url.len() <= MAX_URL && crate::web_api::web_url(url)).map(Box::from);
    let permalink = if item.permalink.starts_with('/') && item.permalink.len() <= MAX_URL { item.permalink.clone() } else { format!("/comments/{id}/").into() };
    Some(Post {
        id,
        subreddit: line(&item.subreddit, MAX_SHORT),
        title,
        author: line(&item.author, MAX_SHORT),
        flair: item.link_flair_text.as_deref().map(|flair| line(flair, MAX_SHORT)).filter(|flair| !flair.is_empty()),
        domain: line(&item.domain, MAX_SHORT),
        permalink,
        url,
        score: item.score as i64,
        comments: item.num_comments.max(0.0) as u64,
        created: item.created_utc as i64,
        kind,
        nsfw: item.over_18,
        spoiler: item.spoiler,
        stickied: item.stickied,
        locked: item.locked,
        limited: false,
        thumb,
        media,
        text: if keep_text { bounded(&item.selftext, MAX_SELF_TEXT) } else { Box::default() },
    })
}

fn comment(item: &RawItem, depth: u8) -> Option<Comment> {
    if item.id.is_empty() || !item.id.bytes().all(|b| b.is_ascii_alphanumeric()) { return None; }
    Some(Comment {
        id: item.id.clone(),
        author: line(&item.author, MAX_SHORT),
        body: bounded(&item.body, MAX_BODY),
        score: item.score as i64,
        score_hidden: item.score_hidden,
        created: item.created_utc as i64,
        depth,
        op: item.is_submitter,
        stickied: item.stickied,
        distinguished: item.distinguished.as_deref().is_some_and(|who| who == "moderator" || who == "admin"),
        edited: item.edited.yes(),
    })
}

fn more(item: &RawItem, depth: u8) -> More {
    More {
        parent: line(&item.parent_id, MAX_SHORT),
        children: item.children.iter().filter(|id| !id.is_empty() && id.len() <= MAX_SHORT && id.bytes().all(|b| b.is_ascii_alphanumeric())).take(MAX_MORE_IDS).cloned().collect(),
        count: item.count.max(0.0).min(f64::from(u32::MAX)) as u32,
        depth,
    }
}

/// Depth first, so a reply follows its parent. Stops at `limit` rows.
fn flatten(children: Vec<RawThing>, depth: u8, out: &mut Vec<Node>, limit: usize) -> bool {
    for child in children {
        if out.len() >= limit { return true; }
        match &*child.kind {
            "t1" => {
                let mut item = child.data;
                let replies = std::mem::take(&mut item.replies);
                if let Some(row) = comment(&item, depth) { out.push(Node::Comment(row)); } else { continue; }
                if let Replies::Listing(listing) = replies
                    && flatten(listing.data.children, depth.saturating_add(1), out, limit)
                {
                    return true;
                }
            }
            "more" => out.push(Node::More(more(&child.data, depth))),
            _ => {}
        }
    }
    false
}

fn invalid() -> String { t("Reddit sent a reply Mandara could not read").into() }

/// A page of posts. Repeated posts in one page are dropped.
pub fn listing(bytes: &[u8], want: Want) -> Result<Listing, String> {
    let raw: RawListing = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let mut posts: Vec<Post> = Vec::with_capacity(raw.data.children.len());
    for child in raw.data.children {
        if &*child.kind != "t3" { continue; }
        if let Some(post) = post(child.data, want, false)
            && !posts.iter().any(|seen| seen.id == post.id)
        {
            posts.push(post);
        }
    }
    let after = raw.data.after.filter(|after| after.len() <= MAX_SHORT && after.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
    Ok(Listing { posts, after, limited: false })
}

/// A post page: `[post listing, comment listing]`.
pub fn thread(bytes: &[u8], want: Want) -> Result<Thread, String> {
    let [first, second]: [RawListing; 2] = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let item = first.data.children.into_iter().find(|child| &*child.kind == "t3").ok_or_else(invalid)?;
    let post = post(item.data, want, true).ok_or_else(invalid)?;
    let mut nodes = Vec::new();
    let truncated = flatten(second.data.children, 0, &mut nodes, MAX_COMMENTS);
    nodes.shrink_to_fit();
    Ok(Thread { post, nodes, truncated, limited: false })
}

/// The comments of a "continue this thread" page, without the comment it
/// continues (the reader already shows it). Depths start at `depth`.
pub fn continued(bytes: &[u8], parent: &str, depth: u8, limit: usize) -> Result<Vec<Node>, String> {
    let [_, second]: [RawListing; 2] = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let mut nodes = Vec::new();
    for child in second.data.children {
        if &*child.kind != "t1" || &*child.data.id != parent { continue; }
        if let Replies::Listing(listing) = child.data.replies {
            flatten(listing.data.children, depth, &mut nodes, limit);
        }
    }
    Ok(nodes)
}

#[derive(Deserialize)]
struct RawMore { json: RawMoreJson }

#[derive(Deserialize)]
struct RawMoreJson {
    #[serde(default)]
    data: RawMoreData,
}

#[derive(Default, Deserialize)]
struct RawMoreData {
    #[serde(default)]
    things: Vec<RawThing>,
}

/// `morechildren` returns its comments flat, each with its parent. Depth
/// comes from the parent: `depth_of` knows the rows already shown, and a
/// reply to a comment in this batch sits one level below it. Rows are put
/// in reading order (a reply right after its parent).
pub fn more_children(bytes: &[u8], depth_of: impl Fn(&str) -> Option<u8>, limit: usize) -> Result<Vec<Node>, String> {
    let raw: RawMore = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let things: Vec<(Box<str>, Node)> = raw.json.data.things.into_iter().filter_map(|thing| {
        let parent = thing.data.parent_id.clone();
        let parent_id = parent.split_once('_').map_or(&*parent, |(_, id)| id);
        let node = match &*thing.kind {
            "t1" => Node::Comment(comment(&thing.data, 0)?),
            "more" => Node::More(more(&thing.data, 0)),
            _ => return None,
        };
        Some((parent_id.into(), node))
    }).collect();
    // Place each row after its parent's subtree in this batch, keeping
    // Reddit's order among siblings.
    let mut ordered: Vec<(Box<str>, Node)> = Vec::with_capacity(things.len());
    for (parent, mut node) in things {
        if ordered.len() >= limit { break; }
        let in_batch = ordered.iter().position(|(_, row)| matches!(row, Node::Comment(c) if *c.id == *parent));
        let depth = match in_batch {
            Some(at) => ordered[at].1.depth().saturating_add(1),
            None => depth_of(&parent).map_or(0, |depth| depth.saturating_add(1)),
        };
        match &mut node { Node::Comment(c) => c.depth = depth, Node::More(m) => m.depth = depth }
        let at = match in_batch {
            Some(at) => {
                let parent_depth = ordered[at].1.depth();
                let mut end = at + 1;
                while end < ordered.len() && ordered[end].1.depth() > parent_depth { end += 1; }
                end
            }
            None => ordered.len(),
        };
        ordered.insert(at, (parent, node));
    }
    Ok(ordered.into_iter().map(|(_, node)| node).collect())
}
