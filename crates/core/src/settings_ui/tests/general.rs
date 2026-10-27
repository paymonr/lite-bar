use super::*;
use crate::startup_task::StartupState;

const HINT_TEXT: &str = "Starts as administrator at sign-in, with no prompt.";
const ASKS: &str = "Turning this on asks for admin once.";

fn with_startup(state: StartupState, elevated: bool) -> Page {
    let mut p = Page::new(TALL);
    p.startup = state;
    p.elevated = elevated;
    p.idle();
    p
}

/// The Start with Windows toggle's rect.
fn toggle(p: &Page) -> Rect {
    let title = find_text(p.frame(), "Start with Windows").unwrap().0;
    fills(p.frame())
        .into_iter()
        .find(|(r, _, _)| r.w == theme::TOGGLE_W && (r.y - title.y).abs() < 20.0)
        .expect("the toggle")
        .0
}

fn toggle_on(p: &Page) -> bool {
    let t = toggle(p);
    fills(p.frame())
        .iter()
        .any(|(r, c, _)| *r == t && *c == theme::ACCENT)
}

/// Neither on nor off: the knob sits in the middle of the track.
fn toggle_mixed(p: &Page) -> bool {
    let t = toggle(p);
    let knob = fills(p.frame())
        .into_iter()
        .find(|(r, _, _)| r.w == theme::TOGGLE_KNOB && t.contains(center(*r)))
        .expect("the knob")
        .0;
    (center(knob).x - center(t).x).abs() < 0.5 && !toggle_on(p)
}

fn err(msg: &str, actual: Option<bool>) -> StartupState {
    StartupState::Error {
        msg: msg.into(),
        actual,
    }
}

fn click_toggle(p: &mut Page) -> Vec<UiAction> {
    let t = toggle(p);
    p.click(center(t))
}

#[test]
fn general_section_sits_between_appearance_and_hotkeys() {
    let p = Page::new(TALL);
    let y = |t: &str| find_text(p.frame(), t).unwrap().0.y;
    assert!(y("APPEARANCE") < y("GENERAL"));
    assert!(y("GENERAL") < y("HOTKEYS"));
    let side = |t: &str| {
        texts(p.frame())
            .into_iter()
            .find(|x| x.1 == t && x.0.x < SIDEBAR_W)
            .unwrap()
            .0
            .y
    };
    assert!(side("Appearance") < side("General"));
    assert!(side("General") < side("Hotkeys"));
    assert!(y("GENERAL") < y("Start with Windows"));
    assert!(y("Start with Windows") < y("HOTKEYS"));
}

#[test]
fn toggle_shows_the_real_state_and_emits_set_startup() {
    let mut p = with_startup(StartupState::Off, true);
    assert!(!toggle_on(&p));
    assert_eq!(click_toggle(&mut p), vec![UiAction::SetStartup(true)]);

    let mut p = with_startup(StartupState::On, true);
    assert!(toggle_on(&p));
    assert_eq!(click_toggle(&mut p), vec![UiAction::SetStartup(false)]);
}

#[test]
fn a_task_for_another_copy_shows_it_and_turning_on_recreates() {
    let mut p = with_startup(StartupState::OnOtherCopy, true);
    assert!(find_text(p.frame(), "On (points to another copy)").is_some());
    assert!(!toggle_on(&p));
    assert_eq!(click_toggle(&mut p), vec![UiAction::SetStartup(true)]);
}

#[test]
fn no_clicks_while_unknown_or_busy() {
    for state in [StartupState::Unknown, StartupState::Busy] {
        let mut p = with_startup(state.clone(), true);
        assert!(click_toggle(&mut p).is_empty(), "{state:?}");
    }
    assert!(find_text(
        with_startup(StartupState::Unknown, true).frame(),
        "Checking…"
    )
    .is_some());
    assert!(find_text(with_startup(StartupState::Busy, true).frame(), "Working…").is_some());
}

#[test]
fn busy_shows_the_value_being_set() {
    let mut p = with_startup(StartupState::Off, true);
    click_toggle(&mut p);
    p.startup = StartupState::Busy;
    p.idle();
    assert!(toggle_on(&p));
    // A failed change shows the state checked afterwards.
    p.startup = err("Couldn't change it", Some(false));
    p.idle();
    assert!(!toggle_on(&p) && !toggle_mixed(&p));
}

#[test]
fn an_unverified_state_is_neither_on_nor_off() {
    for state in [
        StartupState::Unknown,
        StartupState::Busy,
        err("Couldn't check the startup task", None),
    ] {
        // A fresh window: nothing was verified or clicked yet.
        let mut p = with_startup(state.clone(), true);
        assert!(toggle_mixed(&p), "{state:?}");
        assert!(click_toggle(&mut p).is_empty(), "{state:?}");
    }
    // Shown as soon as it is verified.
    let mut p = with_startup(StartupState::Unknown, true);
    p.startup = StartupState::On;
    p.idle();
    assert!(toggle_on(&p) && !toggle_mixed(&p));
}

#[test]
fn hint_and_admin_note() {
    for state in [
        StartupState::On,
        StartupState::Off,
        StartupState::OnOtherCopy,
        StartupState::Unknown,
        err("failed", None),
    ] {
        for elevated in [true, false] {
            let p = with_startup(state.clone(), elevated);
            let (_, _, color) = find_text(p.frame(), HINT_TEXT).expect("the hint");
            assert_eq!(color, theme::MUTED);
            let asks = !elevated && state == StartupState::Off;
            assert_eq!(
                find_text(p.frame(), ASKS).is_some(),
                asks,
                "{state:?} elevated {elevated}"
            );
        }
    }
}

#[test]
fn errors_show_in_the_problem_color() {
    let p = with_startup(err("Couldn't create the task", Some(false)), true);
    let (_, _, color) = find_text(p.frame(), "Couldn't create the task").unwrap();
    assert_eq!(color, Rgba(0xFF, 0x8A, 0x80, 255));
    assert!(find_text(p.frame(), HINT_TEXT).is_some());
}

#[test]
fn an_error_with_a_verified_state_shows_it_and_takes_clicks() {
    let mut p = with_startup(err("failed", Some(true)), true);
    assert!(toggle_on(&p));
    assert_eq!(click_toggle(&mut p), vec![UiAction::SetStartup(false)]);
    let mut p = with_startup(err("failed", Some(false)), true);
    assert!(!toggle_on(&p) && !toggle_mixed(&p));
    assert_eq!(click_toggle(&mut p), vec![UiAction::SetStartup(true)]);
}
