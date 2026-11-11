use super::*;

fn item(id: u64, kind: ItemKind) -> Item {
    Item { id, added: 0, kind }
}

fn blob_dir(name: &str) -> PathBuf {
    PathBuf::from("/cache/stash/7").join(name)
}

#[test]
fn drops_become_the_right_kind() {
    // A browser picture: its bytes win over the link and HTML it also offers.
    assert_eq!(plan_drop(&["text/x-moz-url", "text/uri-list", "text/html", "image/png"]), Some(DropPlan::Image("image/png".into())));
    // A link (Firefox gives a title with text/x-moz-url).
    assert_eq!(plan_drop(&["text/uri-list", "text/plain"]), Some(DropPlan::Links("text/uri-list".into())));
    assert_eq!(plan_drop(&["text/x-moz-url", "text/uri-list", "text/plain"]), Some(DropPlan::Links("text/x-moz-url".into())));
    // Rich text keeps its HTML.
    assert_eq!(plan_drop(&["text/html", "text/plain;charset=utf-8", "text/plain"]), Some(DropPlan::Text { text: "text/plain;charset=utf-8".into(), html: Some("text/html".into()) }));
    assert_eq!(plan_drop(&["UTF8_STRING", "TEXT", "STRING"]), Some(DropPlan::Text { text: "UTF8_STRING".into(), html: None }));
    assert_eq!(plan_drop(&["text/html"]), Some(DropPlan::Html("text/html".into())));
    // Any other picture type, then anything with a real MIME type.
    assert_eq!(plan_drop(&["image/x-xcf"]), Some(DropPlan::Image("image/x-xcf".into())));
    assert_eq!(plan_drop(&["application/x-moz-file-promise", "application/vnd.oasis.opendocument.text"]), Some(DropPlan::Data("application/vnd.oasis.opendocument.text".into())));
    // Only a program's private types: nothing to keep.
    assert_eq!(plan_drop(&["chromium/x-renderer-taint", "TARGETS"]), None);
    assert_eq!(plan_drop(&[] as &[&str]), None);
}

#[test]
fn links_parse_from_uri_lists_and_firefox_utf16() {
    let list = b"# a comment\r\nhttps://example.org/a\r\nhttps://example.org/b\r\n";
    assert_eq!(parse_links(URI_LIST, list), [Link { url: "https://example.org/a".into(), title: None }, Link { url: "https://example.org/b".into(), title: None }]);
    // Firefox's text/x-moz-url: UTF-16LE, URL then title.
    let moz: Vec<u8> = "https://example.org/page\nAn example".encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert_eq!(parse_links(MOZ_URL, &moz), [Link { url: "https://example.org/page".into(), title: Some("An example".into()) }]);
    assert_eq!(decode_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
    assert_eq!(decode_text(b"\xEF\xBB\xBFa\r\nb\0"), "a\nb");
}

#[test]
fn text_previews_and_html_are_bounded() {
    let long: String = "ب".repeat(PREVIEW_CHARS + 50);
    assert_eq!(preview_of(&long).chars().count(), PREVIEW_CHARS);
    assert_eq!(preview_of("short"), "short");
    assert_eq!(html_to_text("<p>Hello&nbsp;<b>world</b></p><script>var x = 1 < 2;</script><p>Tom &amp; Jerry &#x263A;</p>", 200), "Hello world Tom & Jerry ☺");
    assert_eq!(html_to_text("<div>a</div><div>b</div>", 200), "a b");
    assert_eq!(html_to_text(&"<i>x</i>".repeat(1000), 10).chars().count(), 10);
}

#[test]
fn links_and_hosts() {
    assert!(looks_like_image("https://cdn.example.org/cat.JPG?w=800"));
    assert!(!looks_like_image("https://example.org/article"));
    assert!(fetchable("https://example.org/a.png"));
    assert!(!fetchable("http://example.org/a.png"), "HTTPS only");
    assert!(!fetchable("file:///etc/passwd"));
    assert_eq!(single_link(" https://example.org/x "), Some("https://example.org/x"));
    assert_eq!(single_link("see https://example.org"), None);
    assert_eq!(host_of("https://user@sub.example.org:8443/p?q"), Some("sub.example.org"));
    assert_eq!(item(1, ItemKind::Url { url: "https://example.org/x".into(), title: None, icon: None }).title(), "example.org");
}

#[test]
fn file_uris_round_trip_with_odd_names() {
    for p in ["/home/u/Pictures/a b.png", "/tmp/ü%#?.txt", "/srv/مرحبا/ملف.pdf"] {
        let path = PathBuf::from(p);
        let uri = file_uri(&path);
        assert!(uri.starts_with("file:///"), "{uri}");
        assert!(!uri.contains(' ') && !uri.contains('#') && !uri.contains('?'), "{uri}");
        assert_eq!(file_uri_path(&uri), Some(path));
    }
    assert_eq!(file_uri_path("file://localhost/etc/hosts"), Some(PathBuf::from("/etc/hosts")));
    assert_eq!(file_uri_path("file://otherhost/etc/hosts"), None);
    assert_eq!(file_uri_path("https://example.org/"), None);
    assert_eq!(file_uri_path("file:///bad%zz"), None);
}

#[test]
fn drag_out_offers_the_right_types() {
    let file = item(1, ItemKind::File { path: "/home/u/a.pdf".into(), dir: false, mime: "application/pdf".into() });
    let image = item(2, ItemKind::Image { blob: "2.png".into(), mime: "image/png".into(), source: None });
    let text = item(3, ItemKind::Text { blob: "3.txt".into(), preview: "hi".into(), chars: 2, html: Some("3.html".into()) });
    let link = item(4, ItemKind::Url { url: "https://example.org/".into(), title: Some("Example".into()), icon: None });
    let types = |items: &[&Item]| drag_offers(items, blob_dir).into_iter().map(|(m, _)| m).collect::<Vec<_>>();
    // Files: a uri-list and their paths as text.
    assert_eq!(types(&[&file]), [URI_LIST, TEXT_UTF8, TEXT_PLAIN]);
    // A picture: its bytes first (from the stash file), then as a file.
    let offers = drag_offers(&[&image], blob_dir);
    assert_eq!(offers[0], ("image/png".into(), OfferContent::File(blob_dir("2.png"))));
    assert_eq!(offers[1], (URI_LIST.into(), OfferContent::Text("file:///cache/stash/7/2.png\r\n".into())));
    // Text: text and HTML, streamed from the stash files.
    assert_eq!(types(&[&text]), [TEXT_UTF8, HTML, TEXT_PLAIN, UTF8_STRING]);
    // Links: Firefox's moz-url (url + title), a uri-list and text.
    let offers = drag_offers(&[&link], blob_dir);
    assert_eq!(offers[0], (MOZ_URL.into(), OfferContent::Text("https://example.org/\nExample".into())));
    assert_eq!(types(&[&link]), [MOZ_URL, URI_LIST, TEXT_UTF8, TEXT_PLAIN]);
    // A mixed selection: one uri-list with every file and link, no bytes.
    let mixed = drag_offers(&[&file, &image, &link, &text], blob_dir);
    let uris = mixed.iter().find(|(m, _)| m == URI_LIST).map(|(_, c)| c.clone()).unwrap();
    assert_eq!(uris, OfferContent::Text("file:///home/u/a.pdf\r\nfile:///cache/stash/7/2.png\r\nhttps://example.org/\r\n".into()));
    assert!(!mixed.iter().any(|(m, _)| m == "image/png" || m == MOZ_URL));
}

#[test]
fn eviction_takes_oldest_closed_shelves_and_never_open_ones() {
    let mib = 1024 * 1024;
    // 600 MiB open, closed: #1 (100, oldest), #2 (300), #3 (200) — limit 1 GiB.
    let closed = [(2, 300 * mib, 20), (1, 100 * mib, 10), (3, 200 * mib, 30)];
    assert_eq!(plan_eviction(600 * mib, &closed, 1024 * mib), Eviction { evict: vec![1, 2], over_with_open: false });
    // Under the limit: nothing.
    assert_eq!(plan_eviction(10 * mib, &closed, 1024 * mib), Eviction::default());
    // Open shelves alone over: every closed one goes, and the user is asked.
    assert_eq!(plan_eviction(2048 * mib, &closed, 1024 * mib), Eviction { evict: vec![1, 2, 3], over_with_open: true });
}

#[test]
fn closing_and_restoring_keeps_items_and_bounds_the_list() {
    let mut s = StashState::default();
    let mut ids = Vec::new();
    for _ in 0..(CLOSED_KEEP + 3) {
        let id = s.take_id();
        let mut shelf = Shelf::new(id, 0, None, (10.0, 10.0), (320.0, 380.0), ShelfView::Grid);
        let item_id = s.take_id();
        shelf.items.push(item(item_id, ItemKind::Url { url: "https://example.org".into(), title: None, icon: None }));
        s.shelves.push(shelf);
        ids.push(id);
    }
    let mut dropped = Vec::new();
    for id in &ids {
        let (_, gone) = s.close(*id, false, *id).unwrap();
        dropped.extend(gone);
    }
    assert_eq!(s.closed.len(), CLOSED_KEEP);
    assert_eq!(dropped, ids[..3], "the oldest closed shelves fall off");
    let back = s.restore(None).unwrap();
    assert_eq!(back, *ids.last().unwrap());
    assert_eq!(s.shelf(back).unwrap().items.len(), 1);
    // Empty & close forgets at once; an empty shelf is not kept either.
    let (_, gone) = s.close(back, true, 99).unwrap();
    assert_eq!(gone, [back]);
    let empty = s.take_id();
    s.shelves.push(Shelf::new(empty, 0, None, (0.0, 0.0), (320.0, 380.0), ShelfView::Grid));
    assert_eq!(s.close(empty, false, 100).unwrap().1, [empty]);
}

#[test]
fn persistence_round_trips_and_repairs_bad_files() {
    let mut s = StashState::default();
    let id = s.take_id();
    let mut shelf = Shelf::new(id, 1_700_000_000, Some("DP-1".into()), (40.0, 60.0), (360.0, 400.0), ShelfView::Stack);
    shelf.minimized = true;
    for kind in [
        ItemKind::File { path: "/home/u/a b.txt".into(), dir: false, mime: "text/plain".into() },
        ItemKind::File { path: "/home/u/Music".into(), dir: true, mime: "inode/directory".into() },
        ItemKind::Image { blob: "3.png".into(), mime: "image/png".into(), source: Some("https://example.org/cat.png".into()) },
        ItemKind::Text { blob: "4.txt".into(), preview: "مرحبا".into(), chars: 5, html: None },
        ItemKind::Url { url: "https://example.org".into(), title: Some("Ex".into()), icon: None },
        ItemKind::Data { blob: "6.bin".into(), mime: "application/x-thing".into(), bytes: 12 },
    ] {
        let item_id = s.take_id();
        shelf.items.push(item(item_id, kind));
    }
    s.shelves.push(shelf);
    s.sanitize();
    let json = serde_json::to_string_pretty(&s).unwrap();
    let back: StashState = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
    assert!(json.contains("\"kind\": \"image\""));
    // A hand-edited file: duplicate shelf, absurd geometry, stale next id.
    let mut bad = back.clone();
    bad.next_id = 1;
    bad.shelves.push(bad.shelves[0].clone());
    bad.shelves[0].w = f32::NAN;
    bad.shelves[0].h = 1e9;
    bad.shelves[0].x = -50.0;
    bad.sanitize();
    assert_eq!(bad.shelves.len(), 1);
    assert_eq!(bad.shelves[0].w, mandara_config::stash::SHELF_DEFAULT_W);
    assert_eq!(bad.shelves[0].h, mandara_config::stash::SHELF_MAX_H);
    assert_eq!(bad.shelves[0].x, 0.0);
    assert!(bad.next_id > 7, "ids are never reused");
    // An unknown or empty file loads as an empty stash.
    assert_eq!(serde_json::from_str::<StashState>("{}").unwrap(), StashState::default());
}

#[test]
fn commands_pick_their_targets() {
    assert_eq!(parse_target(None), Ok(Target::Latest));
    assert_eq!(parse_target(Some("last")), Ok(Target::Latest));
    assert_eq!(parse_target(Some("all")), Ok(Target::All));
    assert_eq!(parse_target(Some("#4")), Ok(Target::One(4)));
    assert!(parse_target(Some("zero")).is_err());
    assert!(parse_target(Some("0")).is_err());
    let mut s = StashState::default();
    for minimized in [false, true] {
        let id = s.take_id();
        let mut shelf = Shelf::new(id, 0, None, (0.0, 0.0), (320.0, 380.0), ShelfView::Grid);
        shelf.minimized = minimized;
        s.shelves.push(shelf);
    }
    assert_eq!(resolve_target(&s, Target::Latest), [1], "the latest shown shelf");
    assert_eq!(resolve_target(&s, Target::All), [1, 2]);
    assert!(resolve_target(&s, Target::One(9)).is_empty());
    assert_eq!(parse_add("text:hello there"), AddArg::Text("hello there".into()));
    assert_eq!(parse_add("https://example.org/a"), AddArg::Link("https://example.org/a".into()));
    assert_eq!(parse_add("file:///tmp/a%20b"), AddArg::Path("/tmp/a b".into()));
    assert_eq!(parse_add("/tmp/x"), AddArg::Path("/tmp/x".into()));
}

#[test]
fn item_titles_and_blobs() {
    let t = item(1, ItemKind::Text { blob: "1.txt".into(), preview: "\n\n  First line here\nsecond".into(), chars: 30, html: Some("1.html".into()) });
    assert_eq!(t.title(), "First line here");
    assert_eq!(t.kind.blobs().collect::<Vec<_>>(), ["1.txt", "1.html"]);
    let f = item(2, ItemKind::File { path: "/a/b/report.pdf".into(), dir: false, mime: String::new() });
    assert_eq!(f.title(), "report.pdf");
    assert_eq!(f.kind.blobs().count(), 0);
    assert_eq!(extension_for("image/jpeg"), "jpg");
    assert_eq!(extension_for("text/plain;charset=utf-8"), "txt");
    assert_eq!(extension_for("application/x-thing"), "bin");
}
