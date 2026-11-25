use super::*;

/// A config with Stash on and every default edge feature still on.
fn on() -> ShellConfig {
    let mut cfg = ShellConfig::default();
    cfg.plugins.enabled.push(STASH_PLUGIN_ID.into());
    cfg
}

fn set(cfg: &mut ShellConfig, key: &str, value: impl Into<toml::Value>) {
    cfg.plugins.settings.entry(STASH_PLUGIN_ID.into()).or_default().insert(key.into(), value.into());
}

/// A config where no edge feature is left: every edge is free.
fn free() -> ShellConfig {
    let mut cfg = on();
    cfg.sidebar.hover_reveal = false;
    cfg.edges.launcher_bottom = false;
    cfg.edges.session_right = false;
    cfg
}

fn features(cfg: &ShellConfig, edge: ScreenEdge, cx: EdgeContext) -> Vec<EdgeFeature> {
    edge_users(cfg, edge, cx).into_iter().map(|u| u.feature).collect()
}

#[test]
fn settings_default_to_no_catchers_and_parse_every_key() {
    let s = StashSettings::from_config(&on());
    assert_eq!(s, StashSettings::default());
    assert_eq!(s.edges, [false; 4], "catchers are off by default");
    assert_eq!(SHAKE_LEVELS[usize::from(s.shake)], "normal");
    let mut cfg = on();
    set(&mut cfg, "shake", "Firm");
    set(&mut cfg, "edge_left", true);
    set(&mut cfg, "width", 5000.0);
    set(&mut cfg, "height", 10.0);
    set(&mut cfg, "view", "stack");
    set(&mut cfg, "minimize_to", "both");
    set(&mut cfg, "copy_images", true);
    set(&mut cfg, "max_cache_mib", 1);
    let s = StashSettings::from_config(&cfg);
    assert_eq!(SHAKE_LEVELS[usize::from(s.shake)], "firm");
    assert!(s.edge_on(ScreenEdge::Left) && !s.edge_on(ScreenEdge::Right));
    assert_eq!((s.default_width, s.default_height), (SHELF_MAX_W, SHELF_MIN_H), "sizes are clamped");
    assert_eq!(s.default_view, ShelfView::Stack);
    assert!(s.minimize_to.bar() && s.minimize_to.tray());
    assert!(s.copy_images);
    assert_eq!(s.max_cache_mib, CACHE_MIN_MIB);
    // The borrowed readers agree.
    assert_eq!(minimize_to(&cfg), s.minimize_to);
    assert_eq!(edge_flags(&cfg), s.edges);
    // Junk falls back.
    set(&mut cfg, "shake", "wild");
    set(&mut cfg, "view", "carousel");
    let s = StashSettings::from_config(&cfg);
    assert_eq!(s.shake, 2);
    assert_eq!(s.default_view, ShelfView::Grid);
}

#[test]
fn every_default_edge_is_in_use() {
    let cfg = on();
    let cx = EdgeContext::default();
    assert_eq!(features(&cfg, ScreenEdge::Top, cx), [EdgeFeature::Bar]);
    assert_eq!(features(&cfg, ScreenEdge::Left, cx), [EdgeFeature::SidebarHover]);
    assert_eq!(features(&cfg, ScreenEdge::Bottom, cx), [EdgeFeature::LauncherHover]);
    assert_eq!(features(&cfg, ScreenEdge::Right, cx), [EdgeFeature::SessionDockHover]);
}

#[test]
fn each_feature_is_found_on_its_own_edge() {
    let cx = EdgeContext::default();
    // The bar follows its position; the launcher moves to the top then.
    let mut cfg = free();
    cfg.bar.position = BarPosition::Bottom;
    cfg.edges.launcher_bottom = true;
    assert_eq!(features(&cfg, ScreenEdge::Bottom, cx), [EdgeFeature::Bar]);
    assert_eq!(features(&cfg, ScreenEdge::Top, cx), [EdgeFeature::LauncherHover]);
    // A vertical bar holds its edge; the sidebar's hover strip is not drawn there.
    let mut cfg = free();
    cfg.bar.position = BarPosition::Left;
    cfg.sidebar.hover_reveal = true;
    assert_eq!(features(&cfg, ScreenEdge::Left, cx), [EdgeFeature::Bar]);
    // The sidebar on the right (and the session dock then on the left).
    let mut cfg = free();
    cfg.sidebar.side = crate::SidebarSide::Right;
    cfg.sidebar.hover_reveal = true;
    cfg.edges.session_right = true;
    assert_eq!(features(&cfg, ScreenEdge::Right, cx), [EdgeFeature::SidebarHover]);
    assert_eq!(features(&cfg, ScreenEdge::Left, cx), [EdgeFeature::SessionDockHover]);
    // Side bars, pinned or not.
    let mut cfg = free();
    cfg.side_bars.enabled = true;
    cfg.side_bars.left.enabled = true;
    cfg.side_bars.right.enabled = true;
    assert_eq!(features(&cfg, ScreenEdge::Left, cx), [EdgeFeature::SideBar]);
    assert_eq!(features(&cfg, ScreenEdge::Right, cx), [EdgeFeature::SideBar]);
    // The dock on the bottom, then moved to the right.
    let mut cfg = free();
    cfg.plugins.enabled.push(crate::workflow::DOCK_PLUGIN_ID.into());
    assert_eq!(features(&cfg, ScreenEdge::Bottom, cx), [EdgeFeature::Dock]);
    cfg.dock.position = DockPosition::Right;
    assert_eq!(features(&cfg, ScreenEdge::Right, cx), [EdgeFeature::Dock]);
    assert!(features(&cfg, ScreenEdge::Bottom, cx).is_empty());
    // A free config leaves every edge but the bar's free.
    let cfg = free();
    for edge in [ScreenEdge::Left, ScreenEdge::Right, ScreenEdge::Bottom] {
        assert!(edge_users(&cfg, edge, cx).is_empty(), "{edge:?}");
    }
}

#[test]
fn niri_has_no_session_dock_edge() {
    let cfg = on();
    assert!(features(&cfg, ScreenEdge::Right, EdgeContext::for_compositor("niri")).is_empty());
    assert_eq!(features(&cfg, ScreenEdge::Right, EdgeContext::for_compositor("hyprland")), [EdgeFeature::SessionDockHover]);
}

#[test]
fn fixes_point_at_real_settings_and_free_the_edge() {
    let cx = EdgeContext::default();
    let cfg = on();
    let mut fixed = serde_json::to_value(&cfg).unwrap();
    for edge in [ScreenEdge::Left, ScreenEdge::Bottom, ScreenEdge::Right] {
        for user in edge_users(&cfg, edge, cx) {
            let f = user.fixes.first().expect("a one-click fix");
            let slot = fixed.pointer_mut(f.path).unwrap_or_else(|| panic!("{} exists", f.path));
            *slot = serde_json::to_value(f.value).unwrap();
            assert!(fixed.pointer(user.setting).is_some(), "{} exists", user.setting);
        }
    }
    let fixed: ShellConfig = serde_json::from_value(fixed).unwrap();
    for edge in [ScreenEdge::Left, ScreenEdge::Bottom, ScreenEdge::Right] {
        assert!(edge_users(&fixed, edge, cx).is_empty(), "{edge:?} is free after its fix");
    }
    // The bar can only move away.
    let bar = &edge_users(&cfg, ScreenEdge::Top, cx)[0];
    assert_eq!(bar.fixes[0].path, "/bar/position");
}

#[test]
fn a_catcher_on_a_busy_edge_stays_blocked_until_the_edge_is_free() {
    let cx = EdgeContext::default();
    let mut cfg = on();
    set(&mut cfg, "edge_left", true);
    let states = catcher_states(&cfg, cx);
    match &states[ScreenEdge::Left.index()] {
        CatcherState::Blocked { by } => assert_eq!(by[0].feature, EdgeFeature::SidebarHover),
        other => panic!("blocked, not {other:?}"),
    }
    assert_eq!(states[ScreenEdge::Right.index()], CatcherState::Off);
    // The sidebar keeps its edge: nothing was changed behind its back.
    assert!(cfg.sidebar.hover_reveal);
    cfg.sidebar.hover_reveal = false;
    assert!(catcher_states(&cfg, cx)[ScreenEdge::Left.index()].is_active());
    // Stash off: nothing, whatever the settings say.
    cfg.plugins.enabled.clear();
    assert_eq!(catcher_states(&cfg, cx)[ScreenEdge::Left.index()], CatcherState::Off);
}

#[test]
fn turning_on_a_feature_over_an_active_catcher_is_reported() {
    let cx = EdgeContext::default();
    let mut before = free();
    set(&mut before, "edge_left", true);
    set(&mut before, "edge_bottom", true);
    assert!(catcher_states(&before, cx)[ScreenEdge::Left.index()].is_active());
    let mut after = before.clone();
    after.sidebar.hover_reveal = true;
    let hits = new_collisions(&before, &after, cx);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, ScreenEdge::Left);
    assert_eq!(hits[0].1[0].feature, EdgeFeature::SidebarHover);
    // The dock on the bottom collides too; unrelated changes don't.
    let mut dock = before.clone();
    dock.plugins.enabled.push(crate::workflow::DOCK_PLUGIN_ID.into());
    assert_eq!(new_collisions(&before, &dock, cx).iter().map(|h| h.0).collect::<Vec<_>>(), [ScreenEdge::Bottom]);
    let mut other = before.clone();
    other.bar.height += 2.0;
    assert!(new_collisions(&before, &other, cx).is_empty());
    // Switching the catcher itself off is not a collision.
    let mut off = after.clone();
    set(&mut off, "edge_left", false);
    assert!(new_collisions(&before, &off, cx).is_empty());
}

#[test]
fn corner_gaps_widen_for_hot_corners_and_compositor_corners() {
    let cx = EdgeContext::default();
    let mut cfg = free();
    assert_eq!(corner_gap(&cfg, ScreenCorner::TopLeft, cx), CATCHER_CORNER_GAP_PX);
    // The default config's top-left Overview corner, switched on.
    cfg.hot_corners.enabled = true;
    cfg.hot_corners.size = 24.0;
    assert_eq!(corner_gap(&cfg, ScreenCorner::TopLeft, cx), 24.0 + CATCHER_HOT_CORNER_MARGIN_PX);
    assert_eq!(corner_gap(&cfg, ScreenCorner::BottomLeft, cx), CATCHER_CORNER_GAP_PX);
    assert_eq!(corner_users(&cfg, cx), [CornerUse { corner: ScreenCorner::TopLeft, action: Some(CornerAction::Overview) }]);
    // Niri's own corner counts while Mandara's are off.
    cfg.hot_corners.enabled = false;
    let niri = EdgeContext::for_compositor("niri");
    assert_eq!(corner_users(&cfg, niri), [CornerUse { corner: ScreenCorner::TopLeft, action: None }]);
    assert!(corner_gap(&cfg, ScreenCorner::TopLeft, niri) > CATCHER_CORNER_GAP_PX);
}

#[test]
fn catcher_rects_are_hairlines_with_corner_gaps() {
    let out = Rect::new(0.0, 0.0, 2560.0, 1440.0);
    let rects = catcher_rects(out, &[out], [false, true, false, true], |c| if c == ScreenCorner::TopLeft { 48.0 } else { 32.0 });
    assert_eq!(rects.len(), 2);
    let (edge, right) = rects[0];
    assert_eq!(edge, ScreenEdge::Right);
    assert_eq!(right, Rect::new(2560.0 - CATCHER_THICKNESS_PX, 32.0, CATCHER_THICKNESS_PX, 1440.0 - 64.0));
    let (_, left) = rects[1];
    assert_eq!(left.y, 48.0, "the hot corner's gap is wider");
    assert_eq!(left.h, 1440.0 - 48.0 - 32.0);
    assert_eq!(left.w, CATCHER_THICKNESS_PX);
}

#[test]
fn edges_shared_with_another_output_get_no_catcher() {
    // Two outputs side by side, the right one shorter and lower.
    let a = Rect::new(0.0, 0.0, 2560.0, 1440.0);
    let b = Rect::new(2560.0, 400.0, 1920.0, 1080.0);
    let layout = [a, b];
    // a's right edge is outer above b only: 0..400.
    assert_eq!(outer_spans(a, &layout, ScreenEdge::Right).as_slice(), [(0.0, 400.0)]);
    // b's left edge is shared except the 40 px below a.
    assert_eq!(outer_spans(b, &layout, ScreenEdge::Left).as_slice(), [(1440.0, 1480.0)]);
    let rects = catcher_rects(a, &layout, [false, true, false, false], |_| 32.0);
    // 0..400 minus the top corner's gap; the inner end gets no gap.
    assert_eq!(rects.as_slice(), [(ScreenEdge::Right, Rect::new(2560.0 - CATCHER_THICKNESS_PX, 32.0, CATCHER_THICKNESS_PX, 368.0))]);
    // b's left: 40 px minus the bottom corner's gap is too short to keep.
    assert!(catcher_rects(b, &layout, [false, false, false, true], |_| 32.0).is_empty());
    // A fully shared edge has no outer stretch at all.
    let c = Rect::new(2560.0, 0.0, 1920.0, 1440.0);
    assert!(outer_spans(c, &[a, c], ScreenEdge::Left).is_empty());
    // A stretch too short after its gaps is dropped.
    let c = Rect::new(0.0, 0.0, 100.0, 60.0);
    assert!(catcher_rects(c, &[c], [true, false, false, false], |_| 32.0).is_empty());
}

#[test]
fn shelves_spring_from_their_edge_and_snap_gently() {
    // Left edge at y=700 of 1440: centred on the pointer, flush plus inset.
    assert_eq!(shelf_at_edge(ScreenEdge::Left, 700.0, 2560.0, 1440.0, 320.0, 380.0, 8.0), (8.0, 510.0));
    // Near a corner it stays on screen.
    assert_eq!(shelf_at_edge(ScreenEdge::Top, 10.0, 2560.0, 1440.0, 320.0, 380.0, 8.0), (8.0, 8.0));
    assert_eq!(shelf_at_edge(ScreenEdge::Right, 1430.0, 2560.0, 1440.0, 320.0, 380.0, 8.0), (2560.0 - 328.0, 1440.0 - 388.0));
    // Dropped 12 px from the left edge with a 16 px snap: settles at the inset.
    assert_eq!(snap_shelf(12.0, 500.0, 320.0, 380.0, 2560.0, 1440.0, 16.0, 8.0), (8.0, 500.0));
    // In the middle it stays put; off screen it is pulled back.
    assert_eq!(snap_shelf(900.0, 500.0, 320.0, 380.0, 2560.0, 1440.0, 16.0, 8.0), (900.0, 500.0));
    assert_eq!(snap_shelf(-400.0, 5000.0, 320.0, 380.0, 2560.0, 1440.0, 16.0, 8.0), (8.0, 1440.0 - 380.0 - 8.0));
}

#[test]
fn ids_round_trip() {
    for e in ScreenEdge::ALL {
        assert_eq!(ScreenEdge::parse(e.id()), Some(e));
        assert!(e.setting_path().ends_with(e.setting_key()));
    }
    for v in ShelfView::ALL {
        assert_eq!(ShelfView::parse(v.id()), Some(v));
    }
    assert_eq!(MinimizeTo::parse("TRAY"), Some(MinimizeTo::Tray));
}
