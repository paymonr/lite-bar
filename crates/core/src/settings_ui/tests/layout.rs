//! Layout checks at the minimum and a roomy window size, with a conservative text measure.

use super::*;

fn area(r: Rect, o: Rect) -> f32 {
    let i = r.intersect(o);
    i.w * i.h
}

fn text_rects(f: &Frame) -> Vec<(String, Rect)> {
    texts(f)
        .into_iter()
        .map(|(pos, text, style, _)| {
            let (w, h) = CONSERVATIVE.measure(&text, style);
            (text, Rect::new(pos.x, pos.y, w, h))
        })
        .collect()
}

/// Checks one frame: hit targets never overlap, text never runs under a control it does
/// not belong to, and everything stays inside the sidebar or the padded content column.
fn check(f: &Frame, w: f32, h: f32, scroll: f32) {
    let hits: Vec<Rect> = f.hits.iter().copied().filter(|r| r.w * r.h > 0.0).collect();
    let content_left = SIDEBAR_W + 1.0 + PAGE_PAD_X - 0.5;
    let content_right = w - PAGE_PAD_X + 0.5;
    for (i, a) in hits.iter().enumerate() {
        let in_sidebar = a.right() <= SIDEBAR_W;
        let in_content = a.x >= content_left && a.right() <= content_right;
        assert!(
            in_sidebar || in_content,
            "hit {a:?} outside the content (w {w}, scroll {scroll})"
        );
        for b in &hits[i + 1..] {
            assert!(
                area(*a, *b) < 0.01,
                "hits {a:?} and {b:?} overlap (w {w}, scroll {scroll})"
            );
        }
    }
    // Scrolling content is clipped to the viewport under the header.
    let s = stage(f);
    let top = s.bottom() + 13.0;
    let viewport = Rect::new(SIDEBAR_W + 1.0, top, w, h - top);
    for (text, r) in text_rects(f) {
        let r = if r.x >= SIDEBAR_W && r.y >= s.bottom() {
            r.intersect(viewport)
        } else {
            r
        };
        if r.w * r.h <= 0.0 {
            continue;
        }
        // Text inside a field is clipped by it, so only its start must be inside.
        let in_field = |h: &Rect| {
            h.contains(Pt {
                x: r.x,
                y: r.y + 0.5,
            })
        };
        if r.x >= SIDEBAR_W && !hits.iter().any(in_field) {
            assert!(
                r.right() <= content_right,
                "{text:?} {r:?} runs past the content (w {w})"
            );
        }
        for h in &hits {
            if area(r, *h) > 0.01 {
                let inside = r.x >= h.x - 0.5
                    && r.right() <= h.right() + 0.5
                    && r.y >= h.y - 0.5
                    && r.bottom() <= h.bottom() + 0.5;
                assert!(
                    inside || in_field(h),
                    "{text:?} {r:?} overlaps control {h:?} (w {w}, scroll {scroll})"
                );
            }
        }
    }
}

/// Scrolls through the whole page, checking every frame.
fn check_size(w: f32, h: f32) {
    let mut p = Page::with_measure(Rect::new(0.0, 0.0, w, h), &CONSERVATIVE);
    p.games.push(game(
        "C:\\Games\\Some Very Long Game Folder\\a-game-with-a-really-long-name.exe",
        "A Game With A Really Quite Long Name",
        None,
    ));
    p.idle();
    p.idle();
    let max = (p.view.content_h - (h - 150.0)).max(0.0) + 200.0;
    let mut scroll = 0.0;
    let mut seen_restore = false;
    while scroll <= max {
        p.st.scroll.insert(id("settings.scroll"), scroll);
        p.idle();
        let f = p.frame();
        assert!(!f.hits.is_empty());
        check(f, w, h, scroll);
        seen_restore |= find_text(f, "Restore defaults").is_some();
        scroll += 60.0;
    }
    assert!(seen_restore);
}

#[test]
fn minimum_window_lays_out_cleanly() {
    check_size(MIN_WINDOW_DIP.0, MIN_WINDOW_DIP.1);
}

#[test]
fn roomy_window_lays_out_cleanly() {
    check_size(1100.0, 800.0);
}

#[test]
fn minimum_window_with_a_waiting_hotkey_and_status() {
    let w = MIN_WINDOW_DIP.0;
    let mut p = Page::with_measure(Rect::new(0.0, 0.0, w, 3000.0), &CONSERVATIVE);
    p.view.hotkey = HotkeyCapture::Error("Already used by another app".to_string());
    p.status = Some("Already used by another app".to_string());
    p.settings.saved_colors = (0..16).map(|i| Rgb { r: i, g: 0, b: 0 }).collect();
    p.idle();
    p.idle();
    check(p.frame(), w, 3000.0, 0.0);
}

#[test]
fn minimum_window_with_the_restore_button_armed() {
    let w = MIN_WINDOW_DIP.0;
    let mut p = Page::with_measure(Rect::new(0.0, 0.0, w, 3000.0), &CONSERVATIVE);
    p.click_text("Restore defaults");
    p.idle();
    check(p.frame(), w, 3000.0, 0.0);
}

#[test]
fn mixer_buttons_wrap_at_the_minimum_width() {
    let w = MIN_WINDOW_DIP.0;
    let p = Page::with_measure(Rect::new(0.0, 0.0, w, 3000.0), &CONSERVATIVE);
    let f = p.frame();
    let y = |t: &str| find_text(f, t).unwrap().0.y;
    // Not all four buttons fit on one line beside the wheel.
    assert!(y("+ Save to my colors") > y("Use for number"));
}
