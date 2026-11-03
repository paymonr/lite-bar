use super::*;

#[test]
fn game_rename_commit_emits_game_name() {
    let mut p = Page::new(TALL);
    click_field(&mut p, "Minecraft");
    // Typing alone keeps the edit in the view; nothing is saved per keystroke.
    assert_eq!(p.typing("Craft"), vec![]);
    assert!(find_text(p.frame(), "Craft").is_some());
    let a = p.keys(vec![key(UiKey::Enter)]);
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::GameName {
            exe_path: "javaw.exe".to_string(),
            name: Some("Craft".to_string()),
        })]
    );
}

#[test]
fn clearing_rename_emits_none() {
    let mut p = Page::new(TALL);
    click_field(&mut p, "Elden Ring");
    p.keys(vec![
        KeyPress {
            ctrl: true,
            ..key(UiKey::A)
        },
        key(UiKey::Backspace),
    ]);
    let a = p.keys(vec![key(UiKey::Enter)]);
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::GameName {
            exe_path: "eldenring.exe".to_string(),
            name: None,
        })]
    );
}

#[test]
fn rename_is_emitted_on_blur() {
    let mut p = Page::new(TALL);
    click_field(&mut p, "Minecraft");
    p.typing("  Craft ");
    let a = p.click(Pt {
        x: 300.0,
        y: 2990.0,
    });
    assert_eq!(
        changes(&a),
        vec![SettingsChange::GameName {
            exe_path: "javaw.exe".to_string(),
            name: Some("Craft".to_string()),
        }]
    );
}

#[test]
fn unchanged_rename_emits_nothing() {
    let mut p = Page::new(TALL);
    click_field(&mut p, "Elden Ring");
    assert_eq!(p.keys(vec![key(UiKey::Enter)]), vec![]);
}

#[test]
fn rename_is_emitted_when_the_field_disappears() {
    let mut p = Page::new(TALL);
    click_field(&mut p, "Minecraft");
    p.typing("Craft");
    p.games.retain(|g| g.exe_path != "javaw.exe");
    let a = p.idle();
    assert_eq!(
        changes(&a),
        vec![SettingsChange::GameName {
            exe_path: "javaw.exe".to_string(),
            name: Some("Craft".to_string()),
        }]
    );
    // Only once.
    assert_eq!(p.idle(), vec![]);
}

#[test]
fn finish_edits_returns_a_pending_rename() {
    let mut p = Page::new(TALL);
    click_field(&mut p, "Minecraft");
    p.typing("Craft");
    assert_eq!(
        changes(&p.view.finish_edits()),
        vec![SettingsChange::GameName {
            exe_path: "javaw.exe".to_string(),
            name: Some("Craft".to_string()),
        }]
    );
    assert_eq!(p.view.finish_edits(), vec![]);
}

#[test]
fn rename_field_follows_the_settings_when_idle() {
    let mut p = Page::new(TALL);
    p.games[1].override_name = Some("Tarnished".to_string());
    p.idle();
    assert!(find_text(p.frame(), "Tarnished").is_some());
}

#[test]
fn open_stats_folder_button() {
    let mut p = Page::new(TALL);
    assert_eq!(
        p.click_text("Open stats folder"),
        vec![UiAction::OpenStatsFolder]
    );
}

#[test]
fn empty_games_shows_hint() {
    let mut p = Page::new(TALL);
    assert!(find_text(p.frame(), GAMES_HINT).is_none());
    p.games.clear();
    p.idle();
    assert!(find_text(p.frame(), GAMES_HINT).is_some());
    assert!(find_text(p.frame(), "Open stats folder").is_none());
}

/// A page whose only game matches the file-name override `javaw.exe = "Minecraft Mod"`.
fn file_name_override_page() -> Page {
    let mut p = Page::new(TALL);
    p.settings
        .game_names
        .insert("javaw.exe".to_string(), "Minecraft Mod".to_string());
    p.games = vec![GameRow {
        key: "java".to_string(),
        exe_path: r"C:\Java\bin\javaw.exe".to_string(),
        stored_name: "Java".to_string(),
        override_name: Some("Minecraft Mod".to_string()),
        override_key: Some("javaw.exe".to_string()),
    }];
    p.idle();
    p.idle();
    p
}

#[test]
fn clearing_a_file_name_override_removes_it() {
    let mut p = file_name_override_page();
    click_field(&mut p, "Minecraft Mod");
    p.keys(vec![
        KeyPress {
            ctrl: true,
            ..key(UiKey::A)
        },
        key(UiKey::Backspace),
    ]);
    let a = p.keys(vec![key(UiKey::Enter)]);
    assert_eq!(
        changes(&a),
        vec![SettingsChange::GameName {
            exe_path: "javaw.exe".to_string(),
            name: None,
        }]
    );
    assert!(
        p.settings.game_names.is_empty(),
        "{:?}",
        p.settings.game_names
    );
}

#[test]
fn renaming_a_file_name_override_edits_that_key() {
    let mut p = file_name_override_page();
    click_field(&mut p, "Minecraft Mod");
    p.keys(vec![key(UiKey::End)]);
    p.typing("s");
    p.keys(vec![key(UiKey::Enter)]);
    assert_eq!(
        p.settings.game_names.iter().collect::<Vec<_>>(),
        vec![(&"javaw.exe".to_string(), &"Minecraft Mods".to_string())]
    );
}

/// Clicks the button labelled `label` in the row titled `title`.
fn click_in_row(p: &mut Page, title: &str, label: &str) -> Vec<UiAction> {
    let row_y = texts(p.frame())
        .into_iter()
        .filter(|t| t.1 == title)
        .map(|t| t.0.y)
        .next()
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

#[test]
fn not_a_game_moves_a_game_under_not_games_and_back() {
    let mut p = Page::new(TALL);
    assert!(
        find_text(p.frame(), "Not games").is_none(),
        "nothing marked yet"
    );
    let key = game_key_of("javaw.exe");
    let a = click_in_row(&mut p, "Minecraft", "Not a game");
    assert_eq!(changes(&a), vec![not_a_game(&key, Some("Minecraft"))]);

    p.idle();
    let y = |p: &Page, t: &str| find_text(p.frame(), t).map(|f| f.0.y);
    let heading = y(&p, "Not games").expect("the Not games list");
    let row = texts(p.frame())
        .into_iter()
        .filter(|t| t.1 == "Minecraft")
        .map(|t| t.0.y)
        .fold(f32::MIN, f32::max);
    assert!(heading < row, "Minecraft is listed under Not games");
    assert_eq!(
        texts(p.frame())
            .iter()
            .filter(|t| t.1 == "Not a game")
            .count(),
        1,
        "only Elden Ring still has the button"
    );

    let a = click_in_row(&mut p, "Minecraft", "It's a game");
    assert_eq!(changes(&a), vec![not_a_game(&key, None)]);
    p.idle();
    assert!(y(&p, "Not games").is_none());
    assert_eq!(
        texts(p.frame())
            .iter()
            .filter(|t| t.1 == "Not a game")
            .count(),
        2
    );
}

#[test]
fn a_renamed_game_is_marked_under_its_shown_name() {
    let mut p = Page::new(TALL);
    let a = click_in_row(&mut p, "ELDEN RING", "Not a game");
    assert_eq!(
        changes(&a),
        vec![not_a_game(
            &game_key_of("eldenring.exe"),
            Some("Elden Ring")
        )]
    );
}

#[test]
fn a_not_game_without_stats_can_still_be_made_a_game() {
    let mut p = Page::new(TALL);
    p.settings
        .not_games
        .insert("gone-key".to_string(), "Old App".to_string());
    p.idle();
    assert!(find_text(p.frame(), "Not games").is_some());
    let a = click_in_row(&mut p, "Old App", "It's a game");
    assert_eq!(changes(&a), vec![not_a_game("gone-key", None)]);
}

#[test]
fn games_section_is_called_games() {
    let p = Page::new(TALL);
    assert!(find_text(p.frame(), "GAMES").is_some());
    assert!(find_text(p.frame(), "GAME NAMES").is_none());
}
