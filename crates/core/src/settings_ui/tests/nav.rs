use super::*;

#[test]
fn sidebar_switches_page() {
    let mut p = Page::new(MOCKUP);
    assert_eq!(p.view.selected, NAV_OVERLAY);
    p.click_text("Hotkeys");
    assert_eq!(p.view.selected, NAV_HOTKEYS);
    // Disabled future pages cannot be selected.
    p.click_text("History");
    assert_eq!(p.view.selected, NAV_HOTKEYS);
}

#[test]
fn appearance_nav_scrolls_to_heading() {
    let mut p = Page::new(MOCKUP);
    let top = find_text(p.frame(), "OVERLAY").unwrap().0.y;
    p.click_text("Appearance");
    p.idle();
    assert_eq!(find_text(p.frame(), "APPEARANCE").unwrap().0.y, top);
}

#[test]
fn overlay_nav_scrolls_to_top() {
    let mut p = Page::new(MOCKUP);
    let top = find_text(p.frame(), "OVERLAY").unwrap().0.y;
    p.click_text("Hotkeys");
    p.idle();
    assert!(find_text(p.frame(), "OVERLAY").unwrap().0.y < top);
    p.click_text("Overlay");
    p.idle();
    assert_eq!(p.st.scroll.get(&id("settings.scroll")), Some(&0.0));
    assert_eq!(find_text(p.frame(), "OVERLAY").unwrap().0.y, top);
}

#[test]
fn content_height_change_asks_for_a_redraw() {
    let mut p = Page::new(TALL);
    p.idle();
    assert_eq!(p.frame().redraw_after_ms, None);
    p.games.clear();
    p.idle();
    assert_eq!(p.frame().redraw_after_ms, Some(p.now));
}

#[test]
fn overlay_reclick_scrolls_back_to_top() {
    let mut p = Page::new(MOCKUP);
    assert_eq!(p.view.selected, NAV_OVERLAY);
    p.wheel(-2.0);
    assert!(p.scroll() > 0.0);
    p.click_text("Overlay");
    assert_eq!(p.scroll(), 0.0);
}

#[test]
fn appearance_reclick_scrolls_back_to_its_heading() {
    let mut p = Page::new(MOCKUP);
    let top = find_text(p.frame(), "OVERLAY").unwrap().0.y;
    p.click_text("Appearance");
    p.wheel(-1.0);
    assert_eq!(p.view.selected, NAV_APPEARANCE);
    p.click_text("Appearance");
    p.idle();
    assert_eq!(find_text(p.frame(), "APPEARANCE").unwrap().0.y, top);
}

#[test]
fn highlight_follows_the_scroll() {
    // Short enough that the Updates heading can reach the top.
    let mut p = Page::new(Rect::new(0.0, 0.0, 900.0, MIN_WINDOW_DIP.1));
    let h = p.view.headings;
    for (scroll, want) in [
        (h[1] - 1.0, NAV_OVERLAY),
        (h[1], NAV_APPEARANCE),
        (h[2], NAV_HOTKEYS),
        (h[NAV_UPDATES], NAV_UPDATES),
        (0.0, NAV_OVERLAY),
    ] {
        p.st.scroll.insert(id("settings.scroll"), scroll);
        p.idle();
        assert_eq!(p.view.selected, want, "at scroll {scroll}");
    }
}

#[test]
fn wheel_scrolling_moves_the_highlight() {
    let mut p = Page::new(MOCKUP);
    while p.scroll() < p.view.headings[1] {
        p.wheel(-1.0);
    }
    assert_eq!(p.view.selected, NAV_APPEARANCE);
}

#[test]
fn clicked_last_section_stays_selected_at_the_bottom() {
    let mut p = Page::new(MOCKUP);
    p.click_text("Game names");
    p.idle();
    // The page cannot scroll far enough to bring the heading to the top.
    assert!(p.scroll() < p.view.headings[NAV_GAMES]);
    assert_eq!(p.view.selected, NAV_GAMES);
    p.idle();
    assert_eq!(p.view.selected, NAV_GAMES);
}

#[test]
fn settings_header_is_static() {
    let mut p = Page::new(MOCKUP);
    p.click_text("Hotkeys");
    let scroll = p.scroll();
    p.click_text("Settings");
    assert_eq!(p.view.selected, NAV_HOTKEYS);
    assert_eq!(p.scroll(), scroll);
    // Drawn in the selected look: surface fill with an accent bar.
    let at = text_center(p.frame(), "Settings");
    assert!(fills(p.frame())
        .iter()
        .any(|(r, c, _)| *c == theme::ACCENT && r.w == 3.0 && r.y <= at.y && r.bottom() >= at.y));
}

#[test]
fn scrolling_to_the_bottom_highlights_the_last_section() {
    let mut p = Page::new(MOCKUP);
    let mut last = -1.0;
    while p.scroll() != last {
        last = p.scroll();
        p.wheel(-5.0);
    }
    assert!(p.scroll() < p.view.headings[NAV_GAMES]);
    assert_eq!(p.view.selected, NAV_GAMES);
    // Scrolling back up a little leaves the bottom.
    p.wheel(1.0);
    assert_ne!(p.view.selected, NAV_GAMES);
}

#[test]
fn updates_nav_scrolls_to_its_heading() {
    // Short enough that the Updates heading can reach the top.
    let mut p = Page::new(Rect::new(0.0, 0.0, 900.0, MIN_WINDOW_DIP.1));
    let top = find_text(p.frame(), "OVERLAY").unwrap().0.y;
    p.click_text("Updates");
    p.idle();
    assert_eq!(p.view.selected, NAV_UPDATES);
    assert_eq!(find_text(p.frame(), "UPDATES").unwrap().0.y, top);
}
