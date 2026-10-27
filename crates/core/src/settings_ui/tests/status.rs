use super::*;

const SAVE_RED: Rgba = Rgba(0xFF, 0x8A, 0x80, 255);
const FIRST: &str = "Can't save: the settings file has an error.";
const SECOND: &str = "Fix it, then choose Reload settings.";

#[test]
fn save_error_is_pinned_above_the_sections() {
    let mut p = Page::new(MOCKUP);
    assert!(find_text(p.frame(), FIRST).is_none());
    p.save_error = Some(format!("{FIRST}\n{SECOND}"));
    p.idle();
    p.idle();
    let (pos, _, color) = find_text(p.frame(), FIRST).expect("first line");
    assert_eq!(color, SAVE_RED);
    let (pos2, _, color2) = find_text(p.frame(), SECOND).expect("second line");
    assert_eq!(color2, SAVE_RED);
    assert!(pos2.y > pos.y);
    let heading = find_text(p.frame(), "OVERLAY").unwrap().0;
    assert!(heading.y > pos2.y, "the sections start under it");
    // Pinned: scrolling the page leaves it in place.
    p.wheel(-3.0);
    p.idle();
    assert!(p.scroll() > 0.0);
    assert_eq!(find_text(p.frame(), FIRST).unwrap().0, pos);

    p.save_error = None;
    p.idle();
    assert!(find_text(p.frame(), FIRST).is_none());
}
