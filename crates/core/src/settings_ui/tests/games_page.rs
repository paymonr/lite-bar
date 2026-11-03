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
    assert!(find_text(p.frame(), "Not a game").is_none());
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
