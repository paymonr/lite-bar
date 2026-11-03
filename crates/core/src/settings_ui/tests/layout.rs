//! Layout checks at the minimum and a roomy window size, with a conservative text measure.

use super::*;
use crate::history::LocalTime;

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
    // The Settings page scrolls under its preview; the Games page has none.
    let top = find_stage(f).map_or(0.0, |s| s.bottom() + 13.0);
    let viewport = Rect::new(SIDEBAR_W + 1.0, top, w, h - top);
    for (text, r) in text_rects(f) {
        // Page text reaching into the scrolling area is clipped to it (the header's text
        // ends above it).
        let r = if r.x >= SIDEBAR_W && r.bottom() > top {
            r.intersect(viewport)
        } else {
            r
        };
        if r.w * r.h <= 0.0 {
            continue;
        }
        // Text inside a field is clipped by it, so only its start must be inside (probed
        // within the visible part, which can be a sliver at the viewport's edge).
        let in_field = |h: &Rect| {
            h.contains(Pt {
                x: r.x,
                y: r.y + (r.h / 2.0).min(0.5),
            })
        };
        if r.x >= SIDEBAR_W && !hits.iter().any(in_field) {
            assert!(
                r.right() <= content_right,
                "{text:?} {r:?} runs past the content (w {w})"
            );
        }
        // What shows of text that starts in a field (a long placeholder) ends at its edge.
        let r = hits
            .iter()
            .find(|h| in_field(h))
            .map_or(r, |f| r.intersect(*f));
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

/// Texts on the page (right of the sidebar, inside the window) never overlap each other.
fn check_text_apart(f: &Frame, w: f32, h: f32, scroll: f32) {
    let window = Rect::new(SIDEBAR_W + 1.0, 0.0, w, h);
    let rects: Vec<(String, Rect)> = text_rects(f)
        .into_iter()
        .map(|(t, r)| (t, r.intersect(window)))
        .filter(|(_, r)| r.w * r.h > 0.0)
        .collect();
    for (i, (a, ra)) in rects.iter().enumerate() {
        for (b, rb) in &rects[i + 1..] {
            assert!(
                area(*ra, *rb) < 0.5,
                "{a:?} {ra:?} overlaps {b:?} {rb:?} (w {w}, scroll {scroll})"
            );
        }
    }
}

/// Scrolls through the whole page, checking every frame.
fn check_size(w: f32, h: f32) {
    for state in all_update_states() {
        check_size_with(w, h, state, StartupState::Off, true);
    }
    for state in all_startup_states() {
        for elevated in [true, false] {
            check_size_with(w, h, UpdateState::Idle, state.clone(), elevated);
        }
    }
}

fn check_size_with(w: f32, h: f32, update: UpdateState, startup: StartupState, elevated: bool) {
    let mut p = Page::with_measure(Rect::new(0.0, 0.0, w, h), &CONSERVATIVE);
    p.update = update;
    p.startup = startup;
    p.elevated = elevated;
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
    let mut seen_updates = false;
    let mut seen_startup = false;
    while scroll <= max {
        p.st.scroll.insert(id("settings.scroll"), scroll);
        p.idle();
        let f = p.frame();
        assert!(!f.hits.is_empty());
        check(f, w, h, scroll);
        seen_restore |= find_text(f, "Restore defaults").is_some();
        seen_updates |= find_text(f, "Check now").is_some();
        seen_startup |= find_text(f, "Start with Windows").is_some();
        scroll += 60.0;
    }
    assert!(seen_restore);
    assert!(seen_updates);
    assert!(seen_startup);
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

#[test]
fn the_install_row_lays_out_cleanly() {
    for w in [MIN_WINDOW_DIP.0, 1100.0] {
        for status in [
            MoveStatus::Idle,
            MoveStatus::Moving,
            MoveStatus::Done(MoveOutcome::OtherAccount),
            MoveStatus::Done(MoveOutcome::MovedNotStarted("x".repeat(80))),
        ] {
            let mut p = Page::with_measure(Rect::new(0.0, 0.0, w, 3000.0), &CONSERVATIVE);
            p.games.push(game(
                "C:\\Games\\Some Very Long Game Folder\\a-game-with-a-really-long-name.exe",
                "A Game With A Really Quite Long Name",
                None,
            ));
            p.install_offer = true;
            p.move_status = status;
            p.elevated = false;
            p.idle();
            p.idle();
            check(p.frame(), w, 3000.0, 0.0);
        }
    }
}

/// The Games page at `w`×`h` with 20 long-named games, on `tab` ("Games 20" or "Other
/// apps 5"), scrolled through.
fn check_games_page(w: f32, h: f32, other_apps: bool) {
    let mut p = Page::with_measure(Rect::new(0.0, 0.0, w, h), &CONSERVATIVE);
    // "Yesterday 21:04": the widest the Last column gets.
    let yesterday = LocalTime {
        day: p.games_now.day - 1,
        hour: 21,
        minute: 4,
        ..p.games_now
    };
    p.games = (0..20)
        .map(|i| {
            let exe = format!(
                "C:\\Games\\Some Very Long Game Folder {i}\\a-game-with-a-really-long-name-{i}.exe"
            );
            GameRow {
                sessions: 1234,
                play_secs: 3_600_000.0,
                last_played: yesterday,
                summary: Some(crate::frame_stats::StatsSummary {
                    frames: 1000,
                    play_secs: 3_600_000.0,
                    avg_fps: 1234.0,
                    low_1pct_fps: 999.0,
                    high_1pct_fps: 2000.0,
                }),
                ..game(
                    &exe,
                    &format!("A Game With A Really Quite Long Name {i}"),
                    None,
                )
            }
        })
        .collect();
    for g in p.games.iter().take(5) {
        p.settings
            .not_games
            .insert(g.key.clone(), g.stored_name.clone());
    }
    p.show_games();
    if other_apps {
        p.click_text("Other apps 5");
        p.idle();
    }
    p.idle();
    let max = (p.view.games_content_h() - (h - 60.0)).max(0.0) + 200.0;
    let mut scroll = 0.0;
    while scroll <= max {
        p.st.scroll.insert(id("games.scroll"), scroll);
        p.idle();
        check(p.frame(), w, h, scroll);
        check_text_apart(p.frame(), w, h, scroll);
        scroll += 60.0;
    }
}

#[test]
fn games_page_lays_out_cleanly() {
    for (w, h) in [MIN_WINDOW_DIP, (1100.0, 800.0)] {
        for other_apps in [false, true] {
            check_games_page(w, h, other_apps);
        }
    }
}

#[test]
fn narrow_table_drops_sessions_and_last() {
    for (w, wide) in [(MIN_WINDOW_DIP.0, false), (1100.0, true)] {
        let mut p = Page::with_measure(Rect::new(0.0, 0.0, w, 800.0), &CONSERVATIVE);
        p.show_games();
        assert_eq!(find_text(p.frame(), "SESSIONS").is_some(), wide, "w {w}");
        assert_eq!(find_text(p.frame(), "LAST").is_some(), wide, "w {w}");
        assert!(find_text(p.frame(), "PLAYED").is_some());
    }
}

#[test]
fn games_page_card_states_lay_out_cleanly() {
    let summary = crate::frame_stats::StatsSummary {
        frames: 1000,
        play_secs: 360_000.0,
        avg_fps: 1234.0,
        low_1pct_fps: 999.0,
        high_1pct_fps: 2000.0,
    };
    for (w, h) in [MIN_WINDOW_DIP, (1100.0, 800.0)] {
        for playing in [false, true] {
            let mut p = Page::with_measure(Rect::new(0.0, 0.0, w, h), &CONSERVATIVE);
            p.games = vec![GameRow {
                sessions: 1234,
                play_secs: 3_600_000.0,
                summary: Some(summary),
                ..game(
                    "C:\\Games\\Long\\a-game-with-a-really-long-name.exe",
                    "A Game With A Really Quite Long Name That Goes On",
                    None,
                )
            }];
            if playing {
                p.live = Some(LiveGame {
                    key: "live".to_string(),
                    name: "Another Game With A Really Quite Long Name".to_string(),
                    exe_path: "C:\\x.exe".to_string(),
                    session: Some(summary),
                });
            }
            p.show_games();
            check(p.frame(), w, h, 0.0);
            check_text_apart(p.frame(), w, h, 0.0);
        }
    }
}
