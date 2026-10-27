//! The pinned header: the page title and the live overlay preview over a game-like stage.

use std::rc::Rc;

use super::{SettingsView, PAGE_PAD_X};
use crate::frame_stats::StatsSummary;
use crate::layout::{self, overlay_origin, stats_text, Size};
use crate::overlay_draw::overlay_cmds;
use crate::settings::OverlaySettings;
use crate::ui::{theme, Cmd, Pt, Rect, Rgba, TextStyle, Ui};

const STAGE_H: f32 = 92.0;
/// Overlay margin inside the stage, as the real overlay keeps from the screen edge.
const STAGE_MARGIN: i32 = 8;
const TITLE: TextStyle = TextStyle {
    size: 18.0,
    bold: true,
    mono: false,
};
const CAPTION: TextStyle = TextStyle {
    size: 11.0,
    bold: false,
    mono: false,
};
const CAPTION_COLOR: Rgba = Rgba(255, 255, 255, 140);
/// Gradient buffer size; the painter stretches it smoothly over the stage.
const GRAD_W: u32 = 96;
const GRAD_H: u32 = 24;
/// The mockup's stage: 135° through blue-grey, brown, sage and dark green.
const GRAD_STOPS: [(f32, [u8; 3]); 4] = [
    (0.0, [0x2B, 0x3A, 0x4A]),
    (0.45, [0x6D, 0x5A, 0x3B]),
    (0.70, [0x9F, 0xAE, 0x8F]),
    (1.0, [0x1D, 0x2A, 0x1D]),
];

/// Draws the header across `main`'s top and returns its height (bottom border included).
pub(super) fn header(ui: &mut Ui, view: &mut SettingsView, s: &OverlaySettings, main: Rect) -> f32 {
    let m = ui.text_measure();
    let x = main.x + PAGE_PAD_X;
    let y = main.y + 16.0;
    ui.paint(Cmd::Text {
        pos: Pt { x, y },
        text: "Settings".to_string(),
        style: TITLE,
        color: theme::TEXT,
    });
    let title_h = m.measure("Settings", TITLE).1;
    let stage = Rect::new(
        x,
        y + title_h + 10.0,
        (main.w - 2.0 * PAGE_PAD_X).max(0.0),
        STAGE_H,
    );
    let bg = view
        .stage_bg
        .get_or_insert_with(|| Rc::new(gradient()))
        .clone();
    ui.paint(Cmd::Image {
        rect: stage,
        width_px: GRAD_W,
        height_px: GRAD_H,
        rgba: bg,
    });
    let caption = "Preview over a game";
    let (cw, ch) = m.measure(caption, CAPTION);
    ui.paint(Cmd::Text {
        pos: Pt {
            x: stage.right() - 10.0 - cw,
            y: stage.bottom() - 8.0 - ch,
        },
        text: caption.to_string(),
        style: CAPTION,
        color: CAPTION_COLOR,
    });
    if s.visible {
        ui.clip(stage, |ui| overlay_preview(ui, s, stage));
    }
    let h = stage.bottom() + 12.0 - main.y;
    ui.paint(Cmd::Fill {
        rect: Rect::new(main.x, main.y + h, main.w, 1.0),
        color: theme::BORDER,
        radius: 0.0,
    });
    h + 1.0
}

/// The overlay as `overlay_cmds` draws it, showing 144 FPS and fixed sample stats, placed
/// in `stage` by the real overlay's rule and scaled down only if it would not fit (in width
/// or height).
fn overlay_preview(ui: &mut Ui, s: &OverlaySettings, stage: Rect) {
    let sample = StatsSummary {
        frames: 0,
        play_secs: 0.0,
        avg_fps: 138.0,
        low_1pct_fps: 92.0,
        high_1pct_fps: 160.0,
    };
    let stats = stats_text(Some(&sample));
    let (size, cmds) = overlay_cmds(ui.text_measure(), s, "144", Some(&stats));
    let room_w = stage.w - 2.0 * STAGE_MARGIN as f32;
    let room_h = stage.h - 2.0 * STAGE_MARGIN as f32;
    let fit = |room: f32, len: i32| {
        if len > 0 {
            (room / len as f32).max(0.0)
        } else {
            1.0
        }
    };
    let k = fit(room_w, size.w).min(fit(room_h, size.h)).min(1.0);
    let scaled = Size {
        w: (size.w as f32 * k).round() as i32,
        h: (size.h as f32 * k).round() as i32,
    };
    let area = layout::Rect {
        left: stage.x.round() as i32,
        top: stage.y.round() as i32,
        right: stage.right().round() as i32,
        bottom: stage.bottom().round() as i32,
    };
    let (ox, oy) = overlay_origin(s.location, area, scaled, STAGE_MARGIN);
    let origin = Pt {
        x: ox as f32,
        y: oy as f32,
    };
    for cmd in cmds {
        ui.paint(place(cmd, origin, k));
    }
}

/// Moves an overlay command (drawn at 0,0) to `origin`, scaled by `k`.
fn place(cmd: Cmd, o: Pt, k: f32) -> Cmd {
    let at = |p: Pt| Pt {
        x: o.x + p.x * k,
        y: o.y + p.y * k,
    };
    match cmd {
        Cmd::Fill {
            rect,
            color,
            radius,
        } => {
            let p = at(Pt {
                x: rect.x,
                y: rect.y,
            });
            Cmd::Fill {
                rect: Rect::new(p.x, p.y, rect.w * k, rect.h * k),
                color,
                radius: radius * k,
            }
        }
        Cmd::Text {
            pos,
            text,
            style,
            color,
        } => Cmd::Text {
            pos: at(pos),
            text,
            style: TextStyle {
                size: style.size * k,
                ..style
            },
            color,
        },
        // `overlay_cmds` draws only fills and text.
        other => other,
    }
}

/// Opaque RGBA pixels of the stage gradient.
fn gradient() -> Vec<u8> {
    let mut out = Vec::with_capacity((GRAD_W * GRAD_H * 4) as usize);
    for y in 0..GRAD_H {
        for x in 0..GRAD_W {
            let t = ((x as f32 + 0.5) / GRAD_W as f32 + (y as f32 + 0.5) / GRAD_H as f32) / 2.0;
            let i = GRAD_STOPS
                .windows(2)
                .position(|w| t <= w[1].0)
                .unwrap_or(GRAD_STOPS.len() - 2);
            let ((t0, c0), (t1, c1)) = (GRAD_STOPS[i], GRAD_STOPS[i + 1]);
            let f = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
            for ch in 0..3 {
                let v = f32::from(c0[ch]) + (f32::from(c1[ch]) - f32::from(c0[ch])) * f;
                out.push(v.round() as u8);
            }
            out.push(255);
        }
    }
    out
}
