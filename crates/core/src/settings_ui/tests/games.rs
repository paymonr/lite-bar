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
