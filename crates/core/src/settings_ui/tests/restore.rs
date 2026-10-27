use super::*;

#[test]
fn restore_defaults_needs_confirm_then_emits_restore() {
    let mut p = Page::new(TALL);
    p.settings.overlay.counter_size = 40;
    p.idle();
    assert_eq!(p.click_text("Restore defaults"), vec![]);
    assert!(find_text(p.frame(), "Click again to confirm").is_some());
    let a = p.click_text("Click again to confirm");
    assert_eq!(a, vec![UiAction::Change(SettingsChange::RestoreDefaults)]);
    assert_eq!(p.settings.overlay.counter_size, 18);
    p.idle();
    assert!(find_text(p.frame(), "Defaults restored.").is_some());
}

#[test]
fn restore_confirm_expires_after_3_seconds() {
    let mut p = Page::new(TALL);
    p.click_text("Restore defaults");
    p.now = 3000;
    p.idle();
    assert_eq!(p.click_text("Restore defaults"), vec![]);
}

#[test]
fn restore_resyncs_the_hex_fields() {
    let mut p = Page::new(TALL);
    p.settings.overlay.number_color = rgb("#123456");
    p.idle();
    assert!(find_text(p.frame(), "#123456").is_some());
    p.click_text("Restore defaults");
    p.click_text("Click again to confirm");
    p.idle();
    assert!(find_text(p.frame(), "#123456").is_none());
    assert!(find_text(p.frame(), "#7CFC7C").is_some());
}

#[test]
fn focused_hex_shows_the_default_after_restore() {
    let mut p = Page::new(TALL);
    p.settings.overlay.number_color = rgb("#123456");
    p.idle();
    click_field(&mut p, "#123456");
    p.click_text("Restore defaults");
    p.click_text("Click again to confirm");
    p.idle();
    assert!(find_text(p.frame(), "#123456").is_none());
    assert!(find_text(p.frame(), "#7CFC7C").is_some());
}

#[test]
fn armed_restore_is_an_accent_button() {
    let mut p = Page::new(TALL);
    p.click_text("Restore defaults");
    let at = text_center(p.frame(), "Click again to confirm");
    assert!(fills(p.frame())
        .iter()
        .any(|(r, c, _)| *c == theme::ACCENT && r.contains(at)));
}
