//! Pieces shared by the small popup windows (welcome, uninstall): the footer band with
//! right-aligned buttons, wrapped paragraphs and the monospace path box.

use crate::ui::{theme, Cmd, Id, Pt, Rect, Rgba, TextMeasure, TextStyle, Ui};

pub(crate) const PAD: f32 = 24.0;
pub(crate) const FOOTER_H: f32 = 64.0;
/// Between the lines of a wrapped paragraph.
pub(crate) const LINE_GAP: f32 = 2.0;
pub(crate) const HEADING_STYLE: TextStyle = TextStyle {
    size: 20.0,
    bold: true,
    mono: false,
};
pub(crate) const PATH_STYLE: TextStyle = TextStyle {
    size: 12.0,
    bold: false,
    mono: true,
};

/// The target folder in a rounded box, in the monospace font.
pub(crate) fn path_pill(ui: &mut Ui, dir: &str) {
    let (tw, th) = ui.text_measure().measure(dir, PATH_STYLE);
    let h = th + 12.0;
    let w = (tw + 20.0).min(ui.available().w);
    ui.next_width(w);
    ui.row(h, |ui| {
        let r = ui.available();
        let rect = Rect::new(r.x, r.y, w, h);
        ui.paint(Cmd::Fill {
            rect,
            color: theme::SURFACE,
            radius: theme::BUTTON_RADIUS,
        });
        ui.paint(Cmd::Stroke {
            rect,
            color: theme::BORDER,
            radius: theme::BUTTON_RADIUS,
            width: 1.0,
        });
        ui.paint(Cmd::Text {
            pos: Pt {
                x: r.x + 10.0,
                y: r.y + (h - th) / 2.0,
            },
            text: dir.to_string(),
            style: PATH_STYLE,
            color: theme::TEXT,
        });
    });
}

/// The bottom band: `content` runs inside a control-high row of the padded strip, so it
/// can put a status on the left or call [`right_buttons`].
pub(crate) fn footer_band(ui: &mut Ui, rect: Rect, content: impl FnOnce(&mut Ui)) {
    ui.paint(Cmd::Fill {
        rect,
        color: theme::SURFACE,
        radius: 0.0,
    });
    ui.paint(Cmd::Line {
        from: Pt {
            x: rect.x,
            y: rect.y,
        },
        to: Pt {
            x: rect.right(),
            y: rect.y,
        },
        color: theme::BORDER,
        width: 1.0,
    });
    let strip = Rect::new(
        rect.x + PAD,
        rect.y + (rect.h - theme::CONTROL_HEIGHT) / 2.0,
        rect.w - 2.0 * PAD,
        theme::CONTROL_HEIGHT,
    );
    ui.place(strip, |ui| ui.row(theme::CONTROL_HEIGHT, content));
}

/// Right-aligned buttons in the current row, in order (the accent one is bold); returns
/// the index of the one clicked.
pub(crate) fn right_buttons(ui: &mut Ui, buttons: &[(Id, &str, bool)]) -> Option<usize> {
    let m = ui.text_measure();
    let widths: Vec<f32> = buttons
        .iter()
        .map(|&(_, label, accent)| {
            let style = if accent {
                TextStyle::BOLD
            } else {
                TextStyle::BODY
            };
            m.measure(label, style).0 + 2.0 * theme::BUTTON_PAD_X
        })
        .collect();
    let total = widths.iter().sum::<f32>() + theme::GAP * buttons.len().saturating_sub(1) as f32;
    let mut clicked = None;
    ui.right_aligned(total, |ui| {
        for (i, (&(id, label, accent), w)) in buttons.iter().zip(&widths).enumerate() {
            ui.next_width(*w);
            let hit = if accent {
                ui.button_accent(id, label)
            } else {
                ui.button(id, label)
            };
            if hit {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// Body text wrapped at word boundaries to the column's width (the toolkit has no wrapping).
pub(crate) fn paragraph(ui: &mut Ui, text: &str, color: Rgba) {
    wrapped(ui, text, TextStyle::BODY, color);
}

/// `text` in `style`, wrapped like [`paragraph`].
pub(crate) fn wrapped(ui: &mut Ui, text: &str, style: TextStyle, color: Rgba) {
    let max = ui.available().w;
    for (i, line) in wrap(ui.text_measure(), text, style, max).iter().enumerate() {
        if i > 0 {
            ui.space(LINE_GAP);
        }
        ui.label(line, style, color);
    }
}

fn wrap(m: &dyn TextMeasure, text: &str, style: TextStyle, max: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if m.measure(&format!("{l} {word}"), style).0 <= max => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}
