//! Color swatch rows with a hex field, and the HSV color wheel.

use std::rc::Rc;

use super::super::theme::{self, Rgba};
use super::super::{
    id_with, wheel_pixels, Cmd, Hsv, Id, Rect, TextEdit, TextFieldEvent, TextStyle, Ui, WheelCache,
};
use crate::color::Rgb;

const fn rgb(hex: u32) -> Rgb {
    Rgb {
        r: (hex >> 16) as u8,
        g: (hex >> 8) as u8,
        b: hex as u8,
    }
}

/// The preset swatches shown in every color row.
pub const PRESET_COLORS: [Rgb; 12] = [
    rgb(0x7CFC7C),
    rgb(0xFFFFFF),
    rgb(0x000000),
    rgb(0xFF5555),
    rgb(0xFFB86C),
    rgb(0xF1FA8C),
    rgb(0x50FA7B),
    rgb(0x8BE9FD),
    rgb(0x5FA8FF),
    rgb(0xBD93F9),
    rgb(0xFF79C6),
    rgb(0x9AA3AD),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorPickerEvent {
    None,
    /// `value` was changed by a swatch or a committed hex entry.
    Changed,
    /// The saved color at this index was right-clicked.
    RemoveSaved(usize),
}

const HEX_STYLE: TextStyle = TextStyle {
    size: 12.0,
    bold: false,
    mono: true,
};

/// Lays out `n` swatches and then the hex box left to right, wrapping at `avail`.
/// Returns the item rects relative to the widget's top-left and its total size.
fn flow(n: usize, avail: f32) -> (Vec<Rect>, f32, f32) {
    let pitch = theme::HEX_H + theme::SWATCH_GAP;
    let (mut x, mut line, mut w) = (0.0f32, 0usize, 0.0f32);
    let mut rects = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let (iw, ih) = if i < n {
            (theme::SWATCH, theme::SWATCH)
        } else {
            (theme::HEX_W, theme::HEX_H)
        };
        if x > 0.0 && x + iw > avail {
            x = 0.0;
            line += 1;
        }
        let y = line as f32 * pitch + (theme::HEX_H - ih) / 2.0;
        rects.push(Rect::new(x, y, iw, ih));
        w = w.max(x + iw);
        x += iw + theme::SWATCH_GAP;
    }
    let h = (line + 1) as f32 * pitch - theme::SWATCH_GAP;
    (rects, w, h)
}

/// Size of a `color_picker` with `saved` saved colors given `avail` width.
pub fn color_picker_size(saved: usize, avail: f32) -> (f32, f32) {
    let (_, w, h) = flow(PRESET_COLORS.len() + saved, avail);
    (w, h)
}

impl Ui<'_> {
    /// Preset swatches, then `saved` (dashed border), then a hex field. Swatches wrap
    /// to more lines when the row is too narrow. `hex` holds the field's text; it
    /// follows `value` while not focused, and a committed entry that is not a valid
    /// `#RRGGBB` reverts to `value`.
    pub fn color_picker(
        &mut self,
        id: Id,
        value: &mut Rgb,
        saved: &[Rgb],
        hex: &mut TextEdit,
    ) -> ColorPickerEvent {
        let colors: Vec<Rgb> = PRESET_COLORS.iter().chain(saved).copied().collect();
        let (rects, w, h) = flow(colors.len(), self.avail_width());
        let area = self.alloc(Some(w), h);
        let at = |r: &Rect| Rect::new(area.x + r.x, area.y + r.y, r.w, r.h);
        let mut ev = ColorPickerEvent::None;
        for (i, (c, r)) in colors.iter().zip(&rects).enumerate() {
            let rect = at(r);
            let (hot, clicked) = self.interact(id_with(id, i), rect);
            let saved_index = i.checked_sub(PRESET_COLORS.len());
            if clicked && *value != *c {
                *value = *c;
                ev = ColorPickerEvent::Changed;
            }
            if let (true, true, Some(s)) = (hot, self.input.right_released, saved_index) {
                ev = ColorPickerEvent::RemoveSaved(s);
            }
            self.cmds.push(Cmd::Fill {
                rect,
                color: Rgba::from(*c),
                radius: theme::SWATCH_RADIUS,
            });
            self.cmds.push(if saved_index.is_some() {
                Cmd::Dashed {
                    rect,
                    color: theme::MUTED,
                    radius: theme::SWATCH_RADIUS,
                }
            } else {
                Cmd::Stroke {
                    rect,
                    color: theme::BORDER,
                    radius: theme::SWATCH_RADIUS,
                    width: 1.0,
                }
            });
            if *c == *value {
                self.cmds.push(Cmd::Stroke {
                    rect: rect.shrink(-2.0),
                    color: theme::ACCENT,
                    radius: theme::SWATCH_RADIUS + 2.0,
                    width: 2.0,
                });
            }
        }
        let hex_id = id_with(id, usize::MAX);
        let current = value.to_hex();
        if self.state.focused != Some(hex_id) && hex.text != current {
            *hex = TextEdit::new(&current);
        }
        let hex_rect = at(&rects[colors.len()]);
        if self.text_input(hex_id, hex_rect, hex, "", HEX_STYLE) == TextFieldEvent::Committed {
            if let Ok(c) = Rgb::parse_hex(&hex.text) {
                if c != *value {
                    *value = c;
                    ev = ColorPickerEvent::Changed;
                }
            }
            *hex = TextEdit::new(&value.to_hex());
        }
        ev
    }

    /// A hue/saturation disc `diameter` DIPs wide at `hsv.v`. Click or drag sets the
    /// hue from the angle (0 at the top, clockwise) and saturation from the distance,
    /// clamped to the rim.
    pub fn color_wheel(&mut self, id: Id, hsv: &mut Hsv, diameter: f32) -> bool {
        let rect = self.alloc(Some(diameter), diameter);
        let r = diameter / 2.0;
        let (cx, cy) = (rect.x + r, rect.y + r);
        let in_disc = self
            .input
            .pointer
            .is_some_and(|p| (p.x - cx).hypot(p.y - cy) <= r);
        self.record_hit(rect);
        if in_disc {
            self.hover_unrecorded(id, rect);
        }
        let mut changed = false;
        if self.state.active == Some(id) {
            if let Some(p) = self.input.pointer {
                let (dx, dy) = (p.x - cx, p.y - cy);
                let h = dx.atan2(-dy).to_degrees().rem_euclid(360.0);
                let s = if r > 0.0 {
                    (dx.hypot(dy) / r).min(1.0)
                } else {
                    0.0
                };
                if h != hsv.h || s != hsv.s {
                    hsv.h = h;
                    hsv.s = s;
                    changed = true;
                }
            }
        }
        let rgba = self.wheel_image(diameter, hsv.v);
        let px = rgba.1;
        self.cmds.push(Cmd::Image {
            rect,
            width_px: px,
            height_px: px,
            rgba: rgba.0,
        });
        let a = hsv.h.to_radians();
        let d = hsv.s.clamp(0.0, 1.0) * r;
        let (mx, my) = (cx + a.sin() * d, cy - a.cos() * d);
        for (size, color, width) in [
            (17.0, Rgba(0, 0, 0, 255), 1.0),
            (14.0, Rgba(255, 255, 255, 255), 2.0),
        ] {
            self.cmds.push(Cmd::Stroke {
                rect: Rect::new(mx - size / 2.0, my - size / 2.0, size, size),
                color,
                radius: size / 2.0,
                width,
            });
        }
        changed
    }

    /// The wheel's pixels for this size and brightness, regenerated only when either
    /// changes (brightness in 1/256 steps).
    fn wheel_image(&mut self, diameter: f32, v: f32) -> (Rc<Vec<u8>>, u32) {
        let diameter_px = (diameter * self.scale).round().max(1.0) as u32;
        let v_step = (v.clamp(0.0, 1.0) * 256.0).round() as u16;
        match &self.state.wheel_cache {
            Some(c) if c.diameter_px == diameter_px && c.v_step == v_step => {}
            _ => {
                let rgba = wheel_pixels(diameter_px, f32::from(v_step) / 256.0);
                self.state.wheel_cache = Some(WheelCache {
                    diameter_px,
                    v_step,
                    rgba: Rc::new(rgba),
                });
            }
        }
        let c = self
            .state
            .wheel_cache
            .as_ref()
            .expect("cache was just filled");
        (c.rgba.clone(), diameter_px)
    }
}
