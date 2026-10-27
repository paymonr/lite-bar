//! The overlay's look as draw commands, shared by the real overlay and the settings preview.

use crate::layout::{Size, StatsText};
use crate::settings::{OverlaySettings, StatsLayout};
use crate::ui::{Cmd, Pt, Rect, Rgba, TextMeasure, TextStyle};

const LABEL_TEXT: &str = "FPS";
/// Widest number the box always has room for, so it doesn't jump between 99 and 100.
const MIN_NUMBER_TEXT: &str = "000";
const PAD_X: f32 = 8.0;
const PAD_Y: f32 = 4.0;
const GAP: f32 = 4.0;
const RADIUS: f32 = 4.0;
/// Inline stats sit this many gaps to the right of the `FPS` label.
const INLINE_STATS_GAP_FACTOR: f32 = 3.0;

/// The overlay box in DIPs and its draw commands: a rounded background, the FPS number
/// (Consolas bold at `counter_size`), the `FPS` label and the stats (Consolas at
/// `text_size`). The label and inline stats share the number's baseline; stacked stats go
/// on a second line, left-aligned with the number. Stats are ignored when the layout is
/// `Off`. Each stats part is its own `Cmd::Text` run, so labels and values get their colors.
pub fn overlay_cmds(
    measure: &dyn TextMeasure,
    s: &OverlaySettings,
    fps_text: &str,
    stats: Option<&StatsText>,
) -> (Size, Vec<Cmd>) {
    let number_style = TextStyle {
        size: f32::from(s.counter_size),
        bold: true,
        mono: true,
    };
    let label_style = TextStyle {
        size: f32::from(s.text_size),
        bold: false,
        mono: true,
    };
    let number_color = Rgba::from(s.number_color);
    let label_color = Rgba::from(s.label_color);

    let (number_w, number_h) = measure.measure(fps_text, number_style);
    let (min_number_w, _) = measure.measure(MIN_NUMBER_TEXT, number_style);
    let (label_w, label_h) = measure.measure(LABEL_TEXT, label_style);
    let number_box = number_w.max(min_number_w).ceil();
    // Label-styled text placed this far below the number's top shares its baseline.
    let label_dy = measure.baseline(number_style) - measure.baseline(label_style);
    let content_h = number_h.max(label_dy + label_h);
    let label_x = PAD_X + number_box + GAP;
    let label_right = label_x + label_w.ceil();

    let mut width = label_right + PAD_X;
    let mut height = PAD_Y * 2.0 + content_h;
    let mut texts = vec![
        text(
            Pt { x: PAD_X, y: PAD_Y },
            fps_text,
            number_style,
            number_color,
        ),
        text(
            Pt {
                x: label_x,
                y: PAD_Y + label_dy,
            },
            LABEL_TEXT,
            label_style,
            label_color,
        ),
    ];

    if let Some(st) = stats.filter(|_| s.stats != StatsLayout::Off) {
        let (stats_w, stats_h) = measure.measure(&st.text, label_style);
        let mut pos = if s.stats == StatsLayout::Stacked {
            width = width.max(PAD_X * 2.0 + stats_w.ceil());
            height += stats_h.ceil();
            Pt {
                x: PAD_X,
                y: PAD_Y + content_h,
            }
        } else {
            let x = label_right + GAP * INLINE_STATS_GAP_FACTOR;
            width = x + stats_w.ceil() + PAD_X;
            height = height.max(PAD_Y * 2.0 + label_dy + stats_h);
            Pt {
                x,
                y: PAD_Y + label_dy,
            }
        };
        for (part, is_label) in stats_runs(st) {
            let color = if is_label { label_color } else { number_color };
            texts.push(text(pos, part, label_style, color));
            pos.x += measure.measure(part, label_style).0;
        }
    }

    let size = Size {
        w: width.ceil() as i32,
        h: height.ceil() as i32,
    };
    let opacity = f32::from(s.background_opacity.min(100)) / 100.0;
    let bg = Rgba::from(s.background_color);
    let mut cmds = vec![Cmd::Fill {
        rect: Rect::new(0.0, 0.0, size.w as f32, size.h as f32),
        color: Rgba(bg.0, bg.1, bg.2, (opacity * 255.0).round() as u8),
        radius: RADIUS,
    }];
    cmds.extend(texts);
    (size, cmds)
}

fn text(pos: Pt, text: &str, style: TextStyle, color: Rgba) -> Cmd {
    Cmd::Text {
        pos,
        text: text.to_owned(),
        style,
        color,
    }
}

/// Splits the stats line into consecutive non-empty parts, flagging the label ranges.
fn stats_runs(st: &StatsText) -> Vec<(&str, bool)> {
    // UTF-16 offset to byte offset (offsets past the end clamp to it).
    let byte_at = |u16_pos: usize| {
        let mut units = 0;
        for (i, c) in st.text.char_indices() {
            if units >= u16_pos {
                return i;
            }
            units += c.len_utf16();
        }
        st.text.len()
    };
    let mut runs = Vec::new();
    let mut at = 0;
    for &(start, len) in &st.label_ranges {
        let (start, end) = (byte_at(start).max(at), byte_at(start + len).max(at));
        runs.push((&st.text[at..start], false));
        runs.push((&st.text[start..end], true));
        at = end;
    }
    runs.push((&st.text[at..], false));
    runs.retain(|r| !r.0.is_empty());
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;
    use crate::layout::stats_text;
    use crate::settings::StatsLayout;
    use crate::ui::{Pt, Rect, Rgba, TextStyle};

    /// Monospace-like: each char is 0.6 × size wide, lines are 1.2 × size tall,
    /// and the baseline sits at 0.8 × size.
    struct Fake;
    impl TextMeasure for Fake {
        fn measure(&self, text: &str, style: TextStyle) -> (f32, f32) {
            (
                0.6 * style.size * text.chars().count() as f32,
                1.2 * style.size,
            )
        }
        fn baseline(&self, style: TextStyle) -> f32 {
            0.8 * style.size
        }
    }

    fn settings(layout: StatsLayout) -> OverlaySettings {
        OverlaySettings {
            stats: layout,
            ..OverlaySettings::default()
        }
    }

    fn draw(s: &OverlaySettings, stats: bool) -> (Size, Vec<Cmd>) {
        let text = stats_text(None);
        overlay_cmds(&Fake, s, "144", stats.then_some(&text))
    }

    /// (pos, text, style, color) of every text command, in order.
    fn texts(cmds: &[Cmd]) -> Vec<(Pt, String, TextStyle, Rgba)> {
        cmds.iter()
            .filter_map(|c| match c {
                Cmd::Text {
                    pos,
                    text,
                    style,
                    color,
                } => Some((*pos, text.clone(), *style, *color)),
                _ => None,
            })
            .collect()
    }

    fn number_style(size: u16) -> TextStyle {
        TextStyle {
            size: f32::from(size),
            bold: true,
            mono: true,
        }
    }

    fn label_style(size: u16) -> TextStyle {
        TextStyle {
            size: f32::from(size),
            bold: false,
            mono: true,
        }
    }

    #[test]
    fn off_layout_is_number_plus_label() {
        let s = settings(StatsLayout::Off);
        // Stats handed in with layout Off are ignored.
        let (size, cmds) = draw(&s, true);
        // pad 8 + "000" at 18 (32.4 → 33) + gap 4 + "FPS" at 11 (19.8 → 20) + pad 8.
        // Height: pad 4 + number line 21.6 + pad 4 = 29.6 → 30.
        assert_eq!(size, Size { w: 73, h: 30 });
        let bg = Rgba(0, 0, 0, 140);
        assert_eq!(
            cmds[0],
            Cmd::Fill {
                rect: Rect::new(0.0, 0.0, 73.0, 30.0),
                color: bg,
                radius: 4.0
            }
        );
        let t = texts(&cmds);
        assert_eq!(cmds.len(), 3);
        assert_eq!(t[0].0, Pt { x: 8.0, y: 4.0 });
        assert_eq!(t[0].1, "144");
        assert_eq!(t[0].2, number_style(18));
        assert_eq!(t[1].1, "FPS");
        assert_eq!(t[1].2, label_style(11));
        assert_eq!(t[1].0.x, 8.0 + 33.0 + 4.0);
    }

    #[test]
    fn label_sits_on_the_number_baseline() {
        let (_, cmds) = draw(&settings(StatsLayout::Inline), true);
        let t = texts(&cmds);
        let number_base = t[0].0.y + 0.8 * t[0].2.size;
        for (pos, text, style, _) in &t[1..] {
            let base = pos.y + 0.8 * style.size;
            assert!(
                (base - number_base).abs() < 1e-4,
                "{text}: {base} vs {number_base}"
            );
        }
    }

    #[test]
    fn number_box_has_room_for_three_digits() {
        let s = settings(StatsLayout::Off);
        let (two, _) = overlay_cmds(&Fake, &s, "99", None);
        let (three, _) = overlay_cmds(&Fake, &s, "100", None);
        assert_eq!(two, three);
        let (four, _) = overlay_cmds(&Fake, &s, "1000", None);
        assert!(four.w > three.w);
    }

    #[test]
    fn inline_is_wider_than_off() {
        let (off, _) = draw(&settings(StatsLayout::Off), false);
        let (inline, cmds) = draw(&settings(StatsLayout::Inline), true);
        assert!(inline.w > off.w, "{off:?} -> {inline:?}");
        assert!(inline.h >= off.h, "{off:?} -> {inline:?}");
        // Stats start 3 gaps after the label: 8 + 33 + 4 + 20 + 12.
        let t = texts(&cmds);
        assert_eq!(t[2].1, "avg");
        assert_eq!(t[2].0.x, 77.0);
        // The stats runs end one pad before the right edge.
        let stats_w = 0.6 * 11.0 * stats_text(None).text.chars().count() as f32;
        assert_eq!(inline.w, (77.0 + stats_w).ceil() as i32 + 8);
    }

    #[test]
    fn stacked_is_taller_not_wider_than_inline() {
        let (off, _) = draw(&settings(StatsLayout::Stacked), false);
        let (stacked, cmds) = draw(&settings(StatsLayout::Stacked), true);
        let (inline, _) = draw(&settings(StatsLayout::Inline), true);
        assert_eq!(off, draw(&settings(StatsLayout::Off), false).0);
        assert!(stacked.h > off.h && stacked.h > inline.h);
        assert!(
            stacked.w < inline.w,
            "stacked {stacked:?} inline {inline:?}"
        );
        // Second line, left-aligned with the number, under the first line.
        let t = texts(&cmds);
        assert_eq!(t[2].1, "avg");
        assert_eq!(
            t[2].0,
            Pt {
                x: 8.0,
                y: 4.0 + 21.6
            }
        );
    }

    #[test]
    fn stats_runs_are_contiguous_and_rebuild_the_text() {
        let (_, cmds) = draw(&settings(StatsLayout::Inline), true);
        let t = texts(&cmds);
        let runs = &t[2..];
        let joined: String = runs.iter().map(|r| r.1.as_str()).collect();
        assert_eq!(joined, stats_text(None).text);
        for w in runs.windows(2) {
            let width = 0.6 * w[0].2.size * w[0].1.chars().count() as f32;
            assert!((w[1].0.x - (w[0].0.x + width)).abs() < 1e-3);
            assert_eq!(w[1].0.y, w[0].0.y);
        }
    }

    #[test]
    fn counter_size_scales_number_only() {
        let mut s = settings(StatsLayout::Inline);
        let (small, a) = draw(&s, true);
        s.counter_size = 36;
        let (big, b) = draw(&s, true);
        assert!(big.w > small.w && big.h > small.h);
        let (a, b) = (texts(&a), texts(&b));
        assert_eq!(b[0].2, number_style(36));
        for (x, y) in a[1..].iter().zip(&b[1..]) {
            assert_eq!(x.2, y.2, "{} keeps its style", x.1);
        }
    }

    #[test]
    fn text_size_scales_label_and_stats_only() {
        let mut s = settings(StatsLayout::Inline);
        let (small, a) = draw(&s, true);
        s.text_size = 30;
        let (big, b) = draw(&s, true);
        assert!(big.w > small.w);
        let (a, b) = (texts(&a), texts(&b));
        assert_eq!(a[0].2, b[0].2, "number keeps its style");
        for r in &b[1..] {
            assert_eq!(r.2, label_style(30), "{}", r.1);
        }
    }

    #[test]
    fn colors_map_to_the_right_parts() {
        let mut s = settings(StatsLayout::Inline);
        s.number_color = Rgb { r: 1, g: 2, b: 3 };
        s.label_color = Rgb { r: 4, g: 5, b: 6 };
        s.background_color = Rgb { r: 7, g: 8, b: 9 };
        s.background_opacity = 40;
        let (_, cmds) = draw(&s, true);
        match &cmds[0] {
            Cmd::Fill { color, .. } => assert_eq!(*color, Rgba(7, 8, 9, 102)),
            c => panic!("background first, got {c:?}"),
        }
        let number = Rgba(1, 2, 3, 255);
        let label = Rgba(4, 5, 6, 255);
        let t = texts(&cmds);
        assert_eq!(t[0].3, number, "the FPS number");
        for (_, text, _, color) in &t[1..] {
            let is_label = ["FPS", "avg", "1%L", "1%H"].contains(&text.as_str());
            let want = if is_label { label } else { number };
            assert_eq!(*color, want, "{text:?}");
        }
        assert!(t.iter().filter(|r| r.3 == label).count() == 4);
    }
}
