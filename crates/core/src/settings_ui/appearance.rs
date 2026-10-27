//! Appearance section: size and opacity sliders, the color rows and the color mixer.

use std::ops::RangeInclusive;

use super::{
    end_card, row_line, setting_row, text_block, text_size, SettingsView, UiAction, ROW_H,
};
use crate::color::Rgb;
use crate::settings::{OverlaySettings, Settings, MAX_SAVED_COLORS};
use crate::settings_edit::SettingsChange;
use crate::ui::{
    color_picker_size, id, id_with, theme, Cmd, ColorPickerEvent, Hsv, Rect, Rgba, TextEdit,
    TextFieldEvent, TextStyle, Ui,
};

const VALUE_STYLE: TextStyle = TextStyle {
    size: 12.0,
    bold: false,
    mono: true,
};
const CHIP_STYLE: TextStyle = TextStyle {
    size: 11.0,
    bold: false,
    mono: false,
};
const SLIDER_W: f32 = 190.0;
const VALUE_W: f32 = 44.0;
/// Room kept for a color row's title.
const COLOR_TITLE_W: f32 = 150.0;
const SWATCHES_MAX_W: f32 = 440.0;
const WHEEL: f32 = 170.0;
const WHEEL_GAP: f32 = 18.0;
const MIX_HEX_W: f32 = theme::HEX_W + 14.0;
const BRIGHTNESS: &str = "Brightness";
const MIX_CHIP_W: f32 = 56.0;
const MIX_CHIP_H: f32 = 40.0;
const USE_FOR: [&str; 3] = ["Use for number", "Use for text", "Use for background"];
const SAVE: &str = "+ Save to my colors";

/// A color row: title, current color and the change it makes.
type ColorRow = (&'static str, Rgb, fn(Rgb) -> SettingsChange);

/// One slider row's setting.
struct SizeRow {
    key: &'static str,
    title: &'static str,
    hint: Option<&'static str>,
    value: u16,
    default: u16,
    range: RangeInclusive<i64>,
    unit: &'static str,
    change: fn(u16) -> SettingsChange,
}

pub(super) fn section(
    ui: &mut Ui,
    view: &mut SettingsView,
    settings: &Settings,
    out: &mut Vec<UiAction>,
) {
    let o = &settings.overlay;
    let d = OverlaySettings::default();
    let rows = [
        SizeRow {
            key: "counter",
            title: "FPS counter size",
            hint: Some("The big number"),
            value: o.counter_size,
            default: d.counter_size,
            range: 8..=72,
            unit: "px",
            change: SettingsChange::CounterSize,
        },
        SizeRow {
            key: "text",
            title: "Text size",
            hint: Some("\"FPS\" and the stats"),
            value: o.text_size,
            default: d.text_size,
            range: 6..=48,
            unit: "px",
            change: SettingsChange::TextSize,
        },
        SizeRow {
            key: "opacity",
            title: "Background opacity",
            hint: None,
            value: u16::from(o.background_opacity),
            default: u16::from(d.background_opacity),
            range: 0..=100,
            unit: "%",
            change: |v| SettingsChange::BackgroundOpacity(v as u8),
        },
    ];
    ui.card(|ui| {
        for (i, row) in rows.iter().enumerate() {
            if i > 0 {
                row_line(ui);
            }
            size_row(ui, row, out);
        }
    });
    end_card(ui);
    ui.card(|ui| color_rows(ui, view, settings, out));
    end_card(ui);
    ui.card(|ui| mixer(ui, view, &settings.saved_colors, out));
    end_card(ui);
}

fn size_row(ui: &mut Ui, row: &SizeRow, out: &mut Vec<UiAction>) {
    let chip = format!("Default {}{}", row.default, row.unit);
    let chip_w = ui.text_measure().measure(&chip, CHIP_STYLE).0 + 2.0 * theme::CHIP_PAD_X;
    let w = SLIDER_W + theme::GAP + VALUE_W + theme::GAP + chip_w;
    setting_row(ui, ROW_H, Some(row.title), row.hint.as_slice(), w, |ui| {
        let key = id(&format!("settings.size.{}", row.key));
        let mut v = i64::from(row.value);
        ui.next_width(SLIDER_W);
        if ui.slider(key, &mut v, row.range.clone()) {
            out.push(UiAction::Change((row.change)(v as u16)));
        }
        let text = format!("{v}{}", row.unit);
        let tw = ui.text_measure().measure(&text, VALUE_STYLE).0;
        ui.space(theme::GAP + (VALUE_W - tw).max(0.0));
        ui.label(&text, VALUE_STYLE, theme::SEGMENT_TEXT);
        if ui.default_chip(id_with(key, 1), &chip, v == i64::from(row.default)) {
            out.push(UiAction::Change((row.change)(row.default)));
        }
    });
}

fn color_rows(ui: &mut Ui, view: &mut SettingsView, settings: &Settings, out: &mut Vec<UiAction>) {
    let o = &settings.overlay;
    let saved = &settings.saved_colors;
    let rows: [ColorRow; 3] = [
        ("Number color", o.number_color, SettingsChange::NumberColor),
        ("Text color", o.label_color, SettingsChange::LabelColor),
        (
            "Background color",
            o.background_color,
            SettingsChange::BackgroundColor,
        ),
    ];
    let avail = (ui.available().w - COLOR_TITLE_W).clamp(0.0, SWATCHES_MAX_W);
    let (w, h) = color_picker_size(saved.len(), avail);
    for (i, ((title, color, change), hex)) in rows.into_iter().zip(&mut view.hex).enumerate() {
        if i > 0 {
            row_line(ui);
        }
        setting_row(ui, h.max(ROW_H), Some(title), &[], w, |ui| {
            let mut c = color;
            ui.next_width(w);
            match ui.color_picker(id_with(id("settings.color"), i), &mut c, saved, hex) {
                ColorPickerEvent::None => {}
                ColorPickerEvent::Changed => out.push(UiAction::Change(change(c))),
                ColorPickerEvent::RemoveSaved(s) => {
                    let mut kept = saved.clone();
                    kept.remove(s);
                    out.push(UiAction::Change(SettingsChange::SavedColors(kept)));
                }
            }
        });
    }
}

fn mixer(ui: &mut Ui, view: &mut SettingsView, saved: &[Rgb], out: &mut Vec<UiAction>) {
    let lines = [
        "Find any color, then use it or save it to your colors.",
        "Saved colors show with a dashed edge in every row above.",
    ];
    let inner_w = ui.available().w;
    let (_, text_h) = text_size(ui, Some("Color mixer"), &lines);
    text_block(ui, text_h, Some("Color mixer"), &lines, inner_w);
    ui.space(10.0);
    // The controls fit beside the wheel down to `MIN_WINDOW_DIP`; they wrap their
    // buttons onto more lines as needed.
    let side_w = (inner_w - WHEEL - WHEEL_GAP).max(0.0);
    let col_h = controls_height(ui, side_w);
    let h = WHEEL.max(col_h);
    ui.row(h, |ui| {
        ui.color_wheel(id("settings.wheel"), &mut view.mixer, WHEEL);
        ui.space(WHEEL_GAP);
        ui.column(|ui| {
            ui.space((h - col_h) / 2.0);
            controls(ui, view, saved, side_w, out);
        });
    });
}

/// Labels and styles of the mixer's buttons, in order; the first is the accent one.
fn mixer_buttons() -> [(&'static str, TextStyle); 4] {
    [
        (USE_FOR[0], TextStyle::BOLD),
        (USE_FOR[1], TextStyle::BODY),
        (USE_FOR[2], TextStyle::BODY),
        (SAVE, TextStyle::BODY),
    ]
}

/// The buttons split into lines that fit `w`, as indices.
fn button_lines(ui: &Ui, w: f32) -> Vec<Vec<usize>> {
    let m = ui.text_measure();
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut x = 0.0;
    for (i, (t, st)) in mixer_buttons().iter().enumerate() {
        let bw = m.measure(t, *st).0 + 2.0 * theme::BUTTON_PAD_X;
        match lines.last_mut() {
            Some(line) if x + theme::GAP + bw <= w => {
                line.push(i);
                x += theme::GAP + bw;
            }
            _ => {
                lines.push(vec![i]);
                x = bw;
            }
        }
    }
    lines
}

fn controls_height(ui: &Ui, w: f32) -> f32 {
    let n = button_lines(ui, w).len() as f32;
    theme::SLIDER_H
        + theme::GAP
        + MIX_CHIP_H
        + theme::GAP
        + n * theme::CONTROL_HEIGHT
        + (n - 1.0) * theme::GAP
}

/// Brightness, the chip with the hex box, and the buttons, wrapped to `w`.
fn controls(ui: &mut Ui, view: &mut SettingsView, saved: &[Rgb], w: f32, out: &mut Vec<UiAction>) {
    brightness(ui, &mut view.mixer);
    ui.row(MIX_CHIP_H, |ui| chip_and_hex(ui, view));
    let color = view.mixer.to_rgb();
    let changes: [fn(Rgb) -> SettingsChange; 3] = [
        SettingsChange::NumberColor,
        SettingsChange::LabelColor,
        SettingsChange::BackgroundColor,
    ];
    for line in button_lines(ui, w) {
        ui.row(theme::CONTROL_HEIGHT, |ui| {
            for i in line {
                if i < USE_FOR.len() {
                    let bid = id_with(id("settings.use"), i);
                    let clicked = if i == 0 {
                        ui.button_accent(bid, USE_FOR[i])
                    } else {
                        ui.button(bid, USE_FOR[i])
                    };
                    if clicked {
                        out.push(UiAction::Change(changes[i](color)));
                    }
                } else if ui.button(id("settings.save_color"), SAVE) {
                    if let Some(list) = with_saved(saved, color) {
                        out.push(UiAction::Change(SettingsChange::SavedColors(list)));
                    }
                }
            }
        });
    }
}

/// `saved` with `color` appended (oldest dropped past the cap), or `None` if it is
/// already saved.
fn with_saved(saved: &[Rgb], color: Rgb) -> Option<Vec<Rgb>> {
    if saved.contains(&color) {
        return None;
    }
    let mut list = saved.to_vec();
    list.push(color);
    let extra = list.len().saturating_sub(MAX_SAVED_COLORS);
    list.drain(..extra);
    Some(list)
}

fn brightness(ui: &mut Ui, mixer: &mut Hsv) {
    ui.row(theme::SLIDER_H, |ui| {
        let (lw, lh) = ui.text_measure().measure(BRIGHTNESS, TextStyle::BODY);
        ui.next_width(lw);
        ui.column(|ui| {
            ui.space(((theme::SLIDER_H - lh) / 2.0).max(0.0));
            ui.label(BRIGHTNESS, TextStyle::BODY, theme::MUTED);
        });
        let mut v = (mixer.v * 100.0).round() as i64;
        ui.next_width(SLIDER_W.min(ui.available().w));
        if ui.slider(id("settings.brightness"), &mut v, 0..=100) {
            mixer.v = v as f32 / 100.0;
        }
    });
}

fn chip_and_hex(ui: &mut Ui, view: &mut SettingsView) {
    let color = view.mixer.to_rgb();
    let a = ui.available();
    let chip = Rect::new(a.x, a.y + (a.h - MIX_CHIP_H) / 2.0, MIX_CHIP_W, MIX_CHIP_H);
    ui.paint(Cmd::Fill {
        rect: chip,
        color: Rgba::from(color),
        radius: 6.0,
    });
    ui.paint(Cmd::Stroke {
        rect: chip,
        color: theme::BORDER,
        radius: 6.0,
        width: 1.0,
    });
    ui.space(MIX_CHIP_W + 10.0);
    let hex_id = id("settings.mixer_hex");
    let current = color.to_hex();
    if !ui.is_focused(hex_id) && view.mixer_hex.text != current {
        view.mixer_hex = TextEdit::new(&current);
    }
    ui.next_width(MIX_HEX_W);
    if ui.text_field(hex_id, &mut view.mixer_hex, "") == TextFieldEvent::Committed {
        if let Ok(c) = Rgb::parse_hex(view.mixer_hex.text.trim()) {
            view.mixer = Hsv::from_rgb(c);
        }
        view.mixer_hex = TextEdit::new(&view.mixer.to_rgb().to_hex());
    }
}
