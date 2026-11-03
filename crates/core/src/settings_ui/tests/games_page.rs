use super::*;
use crate::frame_stats::StatsSummary;
use crate::history::LocalTime;

/// The harness's "now": 2026-11-03 15:00.
fn today(hour: u8, minute: u8) -> LocalTime {
    LocalTime {
        year: 2026,
        month: 11,
        day: 3,
        hour,
        minute,
        second: 0,
    }
}

/// The page with the Games page shown.
fn games_page() -> Page {
    let mut p = Page::new(TALL);
    p.show_games();
    p
}

fn sidebar_text(p: &Page, text: &str) -> Option<Pt> {
    texts(p.frame())
        .into_iter()
        .find(|t| t.1 == text && t.0.x < SIDEBAR_W)
        .map(|t| t.0)
}

/// Clicks the game name `name` in the table (bold text right of the sidebar) and lets the
/// rename field take the focus.
fn start_rename(p: &mut Page, name: &str) {
    let pos = texts(p.frame())
        .into_iter()
        .filter(|t| t.1 == name && t.2.bold && t.0.x > SIDEBAR_W)
        .map(|t| t.0)
        .next_back()
        .unwrap_or_else(|| panic!("no name {name:?}"));
    p.click(Pt {
        x: pos.x + 2.0,
        y: pos.y + 4.0,
    });
    p.idle();
}

/// Clicks the button labelled `label` in the row whose text `title` is shown.
fn click_in_row(p: &mut Page, title: &str, label: &str) -> Vec<UiAction> {
    let row_y = texts(p.frame())
        .into_iter()
        .filter(|t| t.1 == title && t.0.x > SIDEBAR_W)
        .map(|t| t.0.y)
        .next_back()
        .expect("the row");
    let at = texts(p.frame())
        .into_iter()
        .filter(|t| t.1 == label)
        .min_by(|a, b| (a.0.y - row_y).abs().total_cmp(&(b.0.y - row_y).abs()))
        .expect("the button")
        .0;
    p.click(Pt {
        x: at.x + 2.0,
        y: at.y + 4.0,
    })
}

fn not_a_game(key: &str, name: Option<&str>) -> SettingsChange {
    SettingsChange::NotAGame {
        key: key.to_string(),
        name: name.map(str::to_string),
    }
}

fn rename(exe: &str, name: Option<&str>) -> SettingsChange {
    SettingsChange::GameName {
        exe_path: exe.to_string(),
        name: name.map(str::to_string),
    }
}

// ---- The page ----

#[test]
fn sidebar_lists_games_as_its_own_page_and_no_history() {
    let p = Page::new(TALL);
    let games = sidebar_text(&p, "Games").expect("a Games item");
    let updates = sidebar_text(&p, "Updates").unwrap();
    let mixer = sidebar_text(&p, "Sound mixer").unwrap();
    assert!(updates.y < games.y && games.y < mixer.y);
    assert!(games.x < updates.x, "top-level, not indented");
    assert!(sidebar_text(&p, "History").is_none());
    assert!(sidebar_text(&p, "Optimize").is_some());
    assert!(
        find_text(p.frame(), "GAMES").is_none(),
        "no Games section on Settings"
    );
}

#[test]
fn clicking_games_shows_the_page_and_emits_show_page() {
    let mut p = Page::new(TALL);
    let at = sidebar_text(&p, "Games").unwrap();
    let a = p.click(Pt {
        x: at.x + 4.0,
        y: at.y + 4.0,
    });
    assert_eq!(a, vec![UiAction::ShowPage(WindowPage::Games)]);
    assert_eq!(p.view.page, WindowPage::Games);
    p.idle();
    assert!(find_text(p.frame(), "Open stats folder").is_some());
    assert!(find_text(p.frame(), "OVERLAY").is_none());
    assert!(find_text(p.frame(), "Check for updates").is_none());
}

#[test]
fn clicking_a_settings_item_goes_back() {
    let mut p = games_page();
    let at = sidebar_text(&p, "Hotkeys").unwrap();
    let a = p.click(Pt {
        x: at.x + 4.0,
        y: at.y + 4.0,
    });
    assert_eq!(a, vec![UiAction::ShowPage(WindowPage::Settings)]);
    assert_eq!(p.view.page, WindowPage::Settings);
    assert_eq!(p.view.selected, NAV_HOTKEYS);
    p.idle();
    assert!(find_text(p.frame(), "HOTKEYS").is_some());
}

#[test]
fn switching_pages_commits_a_rename() {
    let mut p = games_page();
    start_rename(&mut p, "Minecraft");
    p.typing("Craft");
    let at = sidebar_text(&p, "Overlay").unwrap();
    let a = p.click(Pt {
        x: at.x + 4.0,
        y: at.y + 4.0,
    });
    assert_eq!(changes(&a), vec![rename("javaw.exe", Some("Craft"))]);
}

#[test]
fn open_stats_folder_button() {
    let mut p = games_page();
    assert_eq!(
        p.click_text("Open stats folder"),
        vec![UiAction::OpenStatsFolder]
    );
}

// ---- Tabs and rows ----

#[test]
fn tabs_show_counts_and_switch() {
    let mut p = games_page();
    assert!(find_text(p.frame(), "Games 2").is_some());
    assert!(find_text(p.frame(), "Other apps 0").is_some());
    click_in_row(&mut p, "Minecraft", "Not a game");
    p.idle();
    assert!(find_text(p.frame(), "Games 1").is_some());
    assert!(find_text(p.frame(), "Other apps 1").is_some());
    p.click_text("Other apps 1");
    p.idle();
    assert!(find_text(p.frame(), "It's a game").is_some());
    // Only the card on top still offers it.
    let n = texts(p.frame())
        .iter()
        .filter(|t| t.1 == "Not a game")
        .count();
    assert_eq!(n, 1);
}

#[test]
fn row_shows_played_last_avg_and_low() {
    let mut p = Page::new(Rect::new(0.0, 0.0, 1100.0, 3000.0));
    p.show_games();
    p.games = vec![GameRow {
        sessions: 14,
        play_secs: 49_906.0,
        last_played: today(12, 18),
        summary: Some(StatsSummary {
            frames: 1000,
            play_secs: 49_906.0,
            avg_fps: 135.8,
            low_1pct_fps: 66.0,
            high_1pct_fps: 258.0,
        }),
        ..game("minecraft.windows.exe", "Minecraft Bedrock", None)
    }];
    p.idle();
    for text in [
        "Minecraft Bedrock",
        "minecraft.windows.exe",
        "14",
        "13:51",
        "Today 12:18",
        "136",
        "66",
    ] {
        assert!(find_text(p.frame(), text).is_some(), "missing {text:?}");
    }
    for head in ["GAME", "SESSIONS", "PLAYED", "LAST", "AVG", "1% LOW"] {
        assert!(find_text(p.frame(), head).is_some(), "missing {head:?}");
    }
}

#[test]
fn rows_are_newest_first() {
    let mut p = games_page();
    p.games[0].last_played = today(9, 0);
    p.games[1].last_played = today(12, 0);
    p.idle();
    let y = |t: &str| find_text(p.frame(), t).unwrap().0.y;
    assert!(y("Elden Ring") < y("Minecraft"));
}

#[test]
fn not_a_game_and_its_a_game_emit_not_a_game_changes() {
    let mut p = games_page();
    let key = game_key_of("javaw.exe");
    let a = click_in_row(&mut p, "Minecraft", "Not a game");
    assert_eq!(changes(&a), vec![not_a_game(&key, Some("Minecraft"))]);
    p.idle();
    p.click_text("Other apps 1");
    p.idle();
    let a = click_in_row(&mut p, "Minecraft", "It's a game");
    assert_eq!(changes(&a), vec![not_a_game(&key, None)]);
}

#[test]
fn a_renamed_game_is_marked_under_its_shown_name() {
    let mut p = games_page();
    let a = click_in_row(&mut p, "Elden Ring", "Not a game");
    assert_eq!(
        changes(&a),
        vec![not_a_game(
            &game_key_of("eldenring.exe"),
            Some("Elden Ring")
        )]
    );
}

#[test]
fn unlisted_marked_keys_show_on_other_apps() {
    let mut p = games_page();
    p.settings
        .not_games
        .insert("gone-key".to_string(), "Old App".to_string());
    p.idle();
    p.click_text("Other apps 1");
    p.idle();
    assert!(
        find_text(p.frame(), "gone-key").is_some(),
        "the key is the detail"
    );
    let a = click_in_row(&mut p, "Old App", "It's a game");
    assert_eq!(changes(&a), vec![not_a_game("gone-key", None)]);
}

#[test]
fn other_apps_rows_cannot_be_renamed() {
    let mut p = games_page();
    click_in_row(&mut p, "Minecraft", "Not a game");
    p.idle();
    p.click_text("Other apps 1");
    p.idle();
    let at = text_center(p.frame(), "Minecraft");
    assert_eq!(p.click(at), vec![]);
    p.idle();
    assert_eq!(p.typing("x"), vec![]);
    assert_eq!(p.keys(vec![key(UiKey::Enter)]), vec![]);
}

#[test]
fn empty_states() {
    let mut p = games_page();
    p.games.clear();
    p.idle();
    assert!(find_text(p.frame(), GAMES_HINT).is_some());
    assert!(find_text(p.frame(), "Open stats folder").is_some());
    p.click_text("Other apps 0");
    p.idle();
    assert!(find_text(
        p.frame(),
        "No other apps. Use \"Not a game\" on the Games tab for anything that isn't a game."
    )
    .is_some());
}

#[test]
fn other_apps_explains_itself() {
    let mut p = games_page();
    p.click_text("Other apps 0");
    p.idle();
    assert!(find_text(
        p.frame(),
        "Not recorded, and left out of games.csv and sessions.csv."
    )
    .is_some());
    assert!(find_text(
        p.frame(),
        "Their history is kept, so \"It's a game\" puts everything back."
    )
    .is_some());
}

// ---- Rename ----

#[test]
fn clicking_a_name_starts_a_rename() {
    let mut p = games_page();
    start_rename(&mut p, "Minecraft");
    assert_eq!(p.typing("Craft"), vec![]);
    assert!(find_text(p.frame(), "Craft").is_some());
}

#[test]
fn enter_commits_the_rename() {
    let mut p = games_page();
    start_rename(&mut p, "Minecraft");
    p.typing("Craft");
    let a = p.keys(vec![key(UiKey::Enter)]);
    assert_eq!(changes(&a), vec![rename("javaw.exe", Some("Craft"))]);
}

#[test]
fn esc_cancels_the_rename() {
    let mut p = games_page();
    start_rename(&mut p, "Minecraft");
    p.typing("Craft");
    assert_eq!(p.keys(vec![key(UiKey::Esc)]), vec![]);
    p.idle();
    assert!(find_text(p.frame(), "Craft").is_none());
    assert!(find_text(p.frame(), "Minecraft").is_some());
}

#[test]
fn clearing_rename_emits_none() {
    let mut p = games_page();
    start_rename(&mut p, "Elden Ring");
    p.keys(vec![
        KeyPress {
            ctrl: true,
            ..key(UiKey::A)
        },
        key(UiKey::Backspace),
    ]);
    let a = p.keys(vec![key(UiKey::Enter)]);
    assert_eq!(changes(&a), vec![rename("eldenring.exe", None)]);
}

#[test]
fn rename_is_emitted_on_blur() {
    let mut p = games_page();
    start_rename(&mut p, "Minecraft");
    p.typing("  Craft ");
    let a = p.click(Pt {
        x: 300.0,
        y: 2990.0,
    });
    assert_eq!(changes(&a), vec![rename("javaw.exe", Some("Craft"))]);
}

#[test]
fn unchanged_rename_emits_nothing() {
    let mut p = games_page();
    start_rename(&mut p, "Elden Ring");
    assert_eq!(p.keys(vec![key(UiKey::Enter)]), vec![]);
}

#[test]
fn rename_is_emitted_when_the_field_disappears() {
    let mut p = games_page();
    start_rename(&mut p, "Minecraft");
    p.typing("Craft");
    p.games.retain(|g| g.exe_path != "javaw.exe");
    let a = p.idle();
    assert_eq!(changes(&a), vec![rename("javaw.exe", Some("Craft"))]);
    // Only once.
    assert_eq!(p.idle(), vec![]);
}

#[test]
fn a_list_refresh_keeps_a_rename_in_progress() {
    let mut p = games_page();
    start_rename(&mut p, "Minecraft");
    p.typing("Craft");
    p.games = p.games.clone();
    p.idle();
    assert!(find_text(p.frame(), "Craft").is_some());
    let a = p.keys(vec![key(UiKey::Enter)]);
    assert_eq!(changes(&a), vec![rename("javaw.exe", Some("Craft"))]);
}

#[test]
fn finish_edits_returns_a_pending_rename() {
    let mut p = games_page();
    start_rename(&mut p, "Minecraft");
    p.typing("Craft");
    assert_eq!(
        changes(&p.view.finish_edits()),
        vec![rename("javaw.exe", Some("Craft"))]
    );
    assert_eq!(p.view.finish_edits(), vec![]);
}

#[test]
fn shown_name_follows_the_settings() {
    let mut p = games_page();
    p.games[1].override_name = Some("Tarnished".to_string());
    p.idle();
    assert!(find_text(p.frame(), "Tarnished").is_some());
}

/// A page whose only game matches the file-name override `javaw.exe = "Minecraft Mod"`.
fn file_name_override_page() -> Page {
    let mut p = games_page();
    p.settings
        .game_names
        .insert("javaw.exe".to_string(), "Minecraft Mod".to_string());
    p.games = vec![GameRow {
        key: "java".to_string(),
        exe_path: r"C:\Java\bin\javaw.exe".to_string(),
        stored_name: "Java".to_string(),
        override_name: Some("Minecraft Mod".to_string()),
        override_key: Some("javaw.exe".to_string()),
        ..GameRow::default()
    }];
    p.idle();
    p.idle();
    p
}

#[test]
fn clearing_a_file_name_override_removes_it() {
    let mut p = file_name_override_page();
    start_rename(&mut p, "Minecraft Mod");
    p.keys(vec![
        KeyPress {
            ctrl: true,
            ..key(UiKey::A)
        },
        key(UiKey::Backspace),
    ]);
    let a = p.keys(vec![key(UiKey::Enter)]);
    assert_eq!(changes(&a), vec![rename("javaw.exe", None)]);
    assert!(
        p.settings.game_names.is_empty(),
        "{:?}",
        p.settings.game_names
    );
}

#[test]
fn renaming_a_file_name_override_edits_that_key() {
    let mut p = file_name_override_page();
    start_rename(&mut p, "Minecraft Mod");
    p.keys(vec![key(UiKey::End)]);
    p.typing("s");
    p.keys(vec![key(UiKey::Enter)]);
    assert_eq!(
        p.settings.game_names.iter().collect::<Vec<_>>(),
        vec![(&"javaw.exe".to_string(), &"Minecraft Mods".to_string())]
    );
}

// ---- The active game card ----

use crate::settings_ui::games_page::{active_card, Card};

fn summary(avg: f64, low: f64, secs: f64) -> StatsSummary {
    StatsSummary {
        frames: 1000,
        play_secs: secs,
        avg_fps: avg,
        low_1pct_fps: low,
        high_1pct_fps: avg * 2.0,
    }
}

fn live(exe: &str, name: &str) -> LiveGame {
    LiveGame {
        key: game_key_of(exe),
        name: name.to_string(),
        exe_path: exe.to_string(),
        session: Some(summary(141.0, 72.0, 2534.0)),
    }
}

fn played(exe: &str, name: &str, last: LocalTime) -> GameRow {
    GameRow {
        last_played: last,
        sessions: 14,
        play_secs: 49_906.0,
        summary: Some(summary(135.8, 66.0, 49_906.0)),
        last_session: Some(crate::history::LastSession {
            start: today(12, 11),
            end: last,
            play_secs: 391.0,
            avg_fps: 128.0,
            low_1pct_fps: 61.0,
            high_1pct_fps: 200.0,
        }),
        ..game(exe, name, None)
    }
}

#[test]
fn the_live_game_is_playing_now_with_its_row() {
    let games = [
        played("a.exe", "A", today(9, 0)),
        played("b.exe", "B", today(12, 0)),
    ];
    let l = live("a.exe", "A");
    match active_card(&games, &BTreeMap::new(), Some(&l)) {
        Some(Card::Playing { live, row }) => {
            assert_eq!(live.key, l.key);
            assert_eq!(row.map(|r| r.key.as_str()), Some(games[0].key.as_str()));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_first_session_plays_without_a_row() {
    let games = [played("b.exe", "B", today(12, 0))];
    let l = live("new.exe", "New");
    assert!(matches!(
        active_card(&games, &BTreeMap::new(), Some(&l)),
        Some(Card::Playing { row: None, .. })
    ));
}

#[test]
fn a_marked_live_game_is_not_playing_now() {
    let games = [
        played("a.exe", "A", today(12, 0)),
        played("b.exe", "B", today(9, 0)),
    ];
    let marked: BTreeMap<String, String> = [(game_key_of("a.exe"), "A".to_string())].into();
    let l = live("a.exe", "A");
    match active_card(&games, &marked, Some(&l)) {
        Some(Card::LastPlayed(row)) => assert_eq!(row.stored_name, "B"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn without_a_live_game_the_newest_unmarked_game_is_last_played() {
    let games = [
        played("a.exe", "A", today(9, 0)),
        played("b.exe", "B", today(12, 0)),
    ];
    match active_card(&games, &BTreeMap::new(), None) {
        Some(Card::LastPlayed(row)) => assert_eq!(row.stored_name, "B"),
        other => panic!("{other:?}"),
    }
    assert!(active_card(&[], &BTreeMap::new(), None).is_none());
}

/// The Games page with Minecraft (played today 12:18) and Elden Ring (earlier).
fn card_page(playing: bool) -> Page {
    let mut p = Page::new(Rect::new(0.0, 0.0, 1100.0, 3000.0));
    p.games = vec![
        played("javaw.exe", "Minecraft", today(12, 18)),
        GameRow {
            override_name: Some("Elden Ring".to_string()),
            ..played("eldenring.exe", "ELDEN RING", today(8, 0))
        },
    ];
    p.games[1].override_key = Some("eldenring.exe".to_string());
    if playing {
        p.live = Some(live("javaw.exe", "Minecraft"));
    }
    p.show_games();
    p
}

#[test]
fn playing_card_shows_this_session_and_all_time() {
    let p = card_page(true);
    for text in [
        "PLAYING NOW",
        "THIS SESSION",
        "ALL TIME",
        "141",
        "72",
        "0:42",
        "14 sessions",
    ] {
        assert!(find_text(p.frame(), text).is_some(), "missing {text:?}");
    }
    let card = find_text(p.frame(), "PLAYING NOW").unwrap().0.y;
    let tabs = find_text(p.frame(), "Games 2").unwrap().0.y;
    assert!(card < tabs, "the card sits above the tabs");
}

#[test]
fn playing_card_without_numbers_yet_shows_dashes() {
    let mut p = card_page(true);
    p.live.as_mut().unwrap().session = None;
    p.idle();
    let dashes = texts(p.frame()).iter().filter(|t| t.1 == "—").count();
    assert!(dashes >= 3, "avg, 1% low and time: {dashes}");
}

#[test]
fn last_played_card_shows_when_and_last_session() {
    let p = card_page(false);
    for text in [
        "LAST PLAYED · Today 12:18",
        "LAST SESSION",
        "ALL TIME",
        "128",
        "61",
        "0:06",
    ] {
        assert!(find_text(p.frame(), text).is_some(), "missing {text:?}");
    }
    assert!(find_text(p.frame(), "PLAYING NOW").is_none());
}

#[test]
fn no_card_without_games() {
    let mut p = games_page();
    p.games.clear();
    p.idle();
    assert!(find_text(p.frame(), "ALL TIME").is_none());
}

/// Clicks `label` nearest below `anchor` (the card's badge).
fn click_in_card(p: &mut Page, anchor: &str, label: &str) -> Vec<UiAction> {
    let y = find_text(p.frame(), anchor).unwrap().0.y;
    let at = texts(p.frame())
        .into_iter()
        .filter(|t| t.1 == label)
        .min_by(|a, b| (a.0.y - y).abs().total_cmp(&(b.0.y - y).abs()))
        .unwrap()
        .0;
    p.click(Pt {
        x: at.x + 2.0,
        y: at.y + 4.0,
    })
}

#[test]
fn card_buttons_rename_and_mark() {
    let mut p = card_page(true);
    let a = click_in_card(&mut p, "PLAYING NOW", "Not a game");
    assert_eq!(
        changes(&a),
        vec![not_a_game(&game_key_of("javaw.exe"), Some("Minecraft"))]
    );
    let mut p = card_page(false);
    p.click_text("Other apps 0");
    p.idle();
    click_in_card(&mut p, "LAST PLAYED · Today 12:18", "Rename");
    p.idle();
    p.typing("Craft");
    let a = p.keys(vec![key(UiKey::Enter)]);
    assert_eq!(changes(&a), vec![rename("javaw.exe", Some("Craft"))]);
}
