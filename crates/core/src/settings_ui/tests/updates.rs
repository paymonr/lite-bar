use super::*;

/// The status line's text and color.
fn status(p: &Page) -> (String, Rgba) {
    let want = match &p.update {
        UpdateState::Idle => format!("Lite Bar v{}", current_version()),
        s => s.status_text(&current_version()),
    };
    let (_, _, color) = find_text(p.frame(), &want).unwrap_or_else(|| panic!("no {want:?}"));
    (want, color)
}

fn with_update(state: UpdateState) -> Page {
    let mut p = Page::new(TALL);
    p.update = state;
    p.idle();
    p
}

#[test]
fn updates_section_sits_between_hotkeys_and_game_names() {
    let p = Page::new(TALL);
    let y = |t: &str| find_text(p.frame(), t).unwrap().0.y;
    assert!(y("HOTKEYS") < y("UPDATES"));
    assert!(y("UPDATES") < y("GAME NAMES"));
    // The sidebar lists it in the same place.
    let side = |t: &str| {
        texts(p.frame())
            .into_iter()
            .find(|x| x.1 == t && x.0.x < SIDEBAR_W)
            .unwrap()
            .0
            .y
    };
    assert!(side("Hotkeys") < side("Updates"));
    assert!(side("Updates") < side("Game names"));
    assert!(find_text(p.frame(), "Check for updates").is_some());
}

#[test]
fn toggle_emits_check_updates() {
    let mut p = Page::new(TALL);
    assert!(p.settings.updates.check);
    let title = find_text(p.frame(), "Check for updates").unwrap().0;
    // The toggle sits at the right end of the title's row.
    let toggle = fills(p.frame())
        .into_iter()
        .find(|(r, _, _)| r.w == theme::TOGGLE_W && (r.y - title.y).abs() < 20.0)
        .expect("the toggle")
        .0;
    let a = p.click(center(toggle));
    assert_eq!(changes(&a), vec![SettingsChange::CheckUpdates(false)]);
    assert!(!p.settings.updates.check);
}

#[test]
fn check_now_emits_its_action() {
    let mut p = Page::new(TALL);
    assert_eq!(p.click_text("Check now"), vec![UiAction::CheckForUpdates]);
    // Also with checks turned off.
    p.settings.updates.check = false;
    p.idle();
    assert_eq!(p.click_text("Check now"), vec![UiAction::CheckForUpdates]);
}

#[test]
fn status_line_shows_each_state_in_its_color() {
    let problem = Rgba(0xFF, 0x8A, 0x80, 255);
    let want = [
        (UpdateState::Idle, theme::MUTED),
        (UpdateState::Checking, theme::MUTED),
        (UpdateState::UpToDate, theme::MUTED),
        (UpdateState::Available(release()), theme::ACCENT),
        (UpdateState::Downloading(release()), theme::MUTED),
        (
            UpdateState::Failed {
                why: "download was corrupted".into(),
                retry: true,
            },
            problem,
        ),
        // Not a failure: the new version is in place.
        (UpdateState::RestartRequired, theme::ACCENT),
        (UpdateState::CheckFailed, theme::MUTED),
    ];
    for (state, color) in want {
        let p = with_update(state.clone());
        let (text, got) = status(&p);
        assert_eq!(got, color, "{state:?}: {text}");
    }
    let p = with_update(UpdateState::UpToDate);
    assert_eq!(
        status(&p).0,
        format!("You're up to date (v{})", current_version())
    );
}

#[test]
fn update_now_shows_only_when_available() {
    for state in all_update_states() {
        let p = with_update(state.clone());
        let available = matches!(state, UpdateState::Available(_));
        assert_eq!(
            find_text(p.frame(), "Update now").is_some(),
            available,
            "{state:?}"
        );
        // A failed install points to the release page too (download it by hand).
        let page = available || matches!(state, UpdateState::Failed { .. });
        assert_eq!(
            find_text(p.frame(), "Open release page").is_some(),
            page,
            "{state:?}"
        );
    }
    // Accent style: bold dark text on an accent fill.
    let p = with_update(UpdateState::Available(release()));
    let (pos, style, color) = find_text(p.frame(), "Update now").unwrap();
    assert!(style.bold);
    assert_eq!(color, theme::ON_ACCENT);
    assert!(fills(p.frame())
        .iter()
        .any(|(r, c, _)| *c == theme::ACCENT && r.contains(pos)));
}

#[test]
fn a_failed_install_offers_the_release_page() {
    let mut p = with_update(UpdateState::Failed {
        why: "Can't update in this folder — download it from GitHub".into(),
        retry: false,
    });
    assert_eq!(
        p.click_text("Open release page"),
        vec![UiAction::OpenReleasePage]
    );
    assert!(find_text(p.frame(), "Check now").is_some());
    assert!(find_text(p.frame(), "Try again").is_none(), "not retryable");
}

#[test]
fn try_again_shows_only_for_a_retryable_failure() {
    for state in all_update_states() {
        let p = with_update(state.clone());
        let retry = matches!(state, UpdateState::Failed { retry: true, .. });
        assert_eq!(
            find_text(p.frame(), "Try again").is_some(),
            retry,
            "{state:?}"
        );
    }
    let mut p = with_update(UpdateState::Failed {
        why: "the download was corrupted (checksum mismatch)".into(),
        retry: true,
    });
    assert_eq!(p.click_text("Try again"), vec![UiAction::InstallUpdate]);
    assert!(find_text(p.frame(), "Open release page").is_some());
}

#[test]
fn restart_now_shows_only_when_a_restart_is_required() {
    for state in all_update_states() {
        let p = with_update(state.clone());
        assert_eq!(
            find_text(p.frame(), "Restart now").is_some(),
            state == UpdateState::RestartRequired,
            "{state:?}"
        );
    }
    let mut p = with_update(UpdateState::RestartRequired);
    assert_eq!(p.click_text("Restart now"), vec![UiAction::RestartToUpdate]);
    assert!(find_text(p.frame(), "Update now").is_none());
}

#[test]
fn update_now_and_release_page_emit_their_actions() {
    let mut p = with_update(UpdateState::Available(release()));
    assert_eq!(p.click_text("Update now"), vec![UiAction::InstallUpdate]);
    assert_eq!(
        p.click_text("Open release page"),
        vec![UiAction::OpenReleasePage]
    );
}
