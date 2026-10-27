use super::*;

#[test]
fn clicking_stacked_emits_stats_change() {
    let mut p = Page::new(TALL);
    let a = p.click_text("Stacked");
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::Stats(
            StatsLayout::Stacked
        ))]
    );
}

#[test]
fn show_overlay_toggle_emits_visible() {
    let mut p = Page::new(TALL);
    let toggle = fills(p.frame())
        .into_iter()
        .find(|(r, _, _)| r.w == theme::TOGGLE_W && r.h == theme::TOGGLE_H)
        .unwrap()
        .0;
    let a = p.click(center(toggle));
    assert_eq!(a, vec![UiAction::Change(SettingsChange::Visible(false))]);
}

#[test]
fn location_click_emits_change() {
    let mut p = Page::new(TALL);
    let screen = fills(p.frame())
        .into_iter()
        .find(|(r, _, _)| r.w == theme::SCREEN_W && r.h == theme::SCREEN_H)
        .unwrap()
        .0;
    // Bottom-right button: inset 8, steps of 65 across and 30 down, 14 wide.
    let a = p.click(Pt {
        x: screen.x + 8.0 + 130.0 + 7.0,
        y: screen.y + 8.0 + 60.0 + 7.0,
    });
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::Location(
            Location::BottomRight
        ))]
    );
}
