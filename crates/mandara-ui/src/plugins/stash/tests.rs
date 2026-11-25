use super::*;

#[test]
fn file_type_icons_go_from_specific_to_generic() {
    assert_eq!(icon_names(true, "inode/directory"), ["folder", "inode-directory"]);
    assert_eq!(icon_names(false, "image/png"), ["image-png", "image-x-generic", "application-x-generic", "text-x-generic"]);
    assert_eq!(icon_names(false, "application/pdf"), ["application-pdf", "application-x-generic", "text-x-generic"]);
}

#[test]
fn minimized_shelves_go_where_the_setting_says() {
    assert_eq!(minimize_targets(MinimizeTo::Bar), (true, false));
    assert_eq!(minimize_targets(MinimizeTo::Tray), (false, true));
    assert_eq!(minimize_targets(MinimizeTo::Both), (true, true));
}

#[test]
fn listing_reports_shelves_items_and_closed_ones() {
    let mut s = StashState::default();
    let id = s.take_id();
    let mut shelf = Shelf::new(id, 0, Some("DP-1".into()), (1.0, 2.0), (320.0, 380.0), ShelfView::List);
    let item = s.take_id();
    shelf.items.push(Item { id: item, added: 0, kind: ItemKind::Url { url: "https://example.org/a".into(), title: Some("A page".into()), icon: None } });
    s.shelves.push(shelf);
    let closed = s.take_id();
    let mut c = Shelf::new(closed, 0, None, (0.0, 0.0), (320.0, 380.0), ShelfView::Grid);
    let other = s.take_id();
    c.items.push(Item { id: other, added: 0, kind: ItemKind::Text { blob: "t.txt".into(), preview: "hello".into(), chars: 5, html: None } });
    s.shelves.push(c);
    s.close(closed, false, 42);
    let j = list_json(&s);
    assert_eq!(j["shelves"][0]["view"], "list");
    assert_eq!(j["shelves"][0]["items"][0]["kind"], "url");
    assert_eq!(j["shelves"][0]["items"][0]["title"], "A page");
    assert_eq!(j["closed"][0]["id"], closed);
    assert_eq!(j["closed"][0]["items"], 1);
}
