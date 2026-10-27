use super::*;

#[test]
fn preview_shows_144_and_hides_when_overlay_off() {
    let mut p = Page::new(TALL);
    let (_, style, color) = find_text(p.frame(), "144").expect("preview number");
    assert!(style.mono && style.bold);
    assert_eq!(color, Rgba::from(p.settings.overlay.number_color));
    // Stats values are runs of their own, padded: " 138  ".
    assert!(texts(p.frame()).iter().any(|t| t.1.trim() == "138"));
    p.settings.overlay.visible = false;
    p.idle();
    assert!(find_text(p.frame(), "144").is_none());
    assert!(preview_box(p.frame()).is_none());
    // The stage itself stays.
    stage(p.frame());
}

#[test]
fn preview_without_stats_shows_only_the_counter() {
    let mut p = Page::new(TALL);
    p.settings.overlay.stats = StatsLayout::Off;
    p.idle();
    assert!(find_text(p.frame(), "144").is_some());
    assert!(find_text(p.frame(), "avg").is_none());
}

#[test]
fn preview_sits_in_the_chosen_corner_of_the_stage() {
    let mut p = Page::new(TALL);
    let s = stage(p.frame());
    let b = preview_box(p.frame()).unwrap();
    assert_eq!((b.x, b.y), (s.x + 8.0, s.y + 8.0));
    p.settings.overlay.location = Location::BottomRight;
    p.idle();
    let b = preview_box(p.frame()).unwrap();
    assert_eq!((b.right(), b.bottom()), (s.right() - 8.0, s.bottom() - 8.0));
}

#[test]
fn preview_scales_down_only_when_wider_than_the_stage() {
    let p = Page::new(TALL);
    let full = preview_box(p.frame()).unwrap();
    let s = stage(p.frame());
    assert!(full.w < s.w - 16.0, "fits at full size in a wide window");
    let number_size = find_text(p.frame(), "144").unwrap().1.size;
    assert_eq!(number_size, 18.0);

    let p = Page::new(Rect::new(0.0, 0.0, 400.0, 3000.0));
    let s = stage(p.frame());
    let b = preview_box(p.frame()).unwrap();
    assert!(full.w > s.w - 16.0, "too wide for the narrow stage");
    assert!(b.w <= s.w - 16.0 + 0.5, "scaled to fit: {b:?} in {s:?}");
    assert_eq!(b.x, s.x + 8.0);
    let k = b.w / full.w;
    let number_size = find_text(p.frame(), "144").unwrap().1.size;
    assert!((number_size - 18.0 * k).abs() < 0.1);
}

#[test]
fn stage_gradient_is_generated_once() {
    let mut p = Page::new(TALL);
    let image = |f: &Frame| {
        f.cmds
            .iter()
            .find_map(|c| match c {
                Cmd::Image { rect, rgba, .. } if rect.h == 92.0 => Some(rgba.clone()),
                _ => None,
            })
            .unwrap()
    };
    let first = image(p.frame());
    p.idle();
    assert!(std::rc::Rc::ptr_eq(&first, &image(p.frame())));
}

#[test]
fn preview_scales_down_to_fit_the_stage_height() {
    let mut p = Page::new(TALL);
    p.settings.overlay.counter_size = 72;
    p.settings.overlay.stats = StatsLayout::Stacked;
    p.idle();
    let s = stage(p.frame());
    let b = preview_box(p.frame()).unwrap();
    assert!(
        b.h <= s.h - 16.0 + 0.5,
        "fits the stage height: {b:?} in {s:?}"
    );
    assert!(b.w <= s.w - 16.0 + 0.5);
    assert_eq!((b.x, b.y), (s.x + 8.0, s.y + 8.0));
}
