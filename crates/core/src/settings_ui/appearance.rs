//! Appearance section: size and opacity sliders, the color rows and the color mixer.

use std::ops::RangeInclusive;

use super::{end_card, row_line, setting_row, title_block, SettingsView, UiAction, HINT, ROW_H};
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
    setting_row(ui, ROW_H, row.title, row.hint, w, |ui| {
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
        setting_row(ui, h.max(ROW_H), title, None, w, |ui| {
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
    ui.row(30.0, |ui| {
        ui.column(|ui| {
            title_block(
                ui,
                30.0,
                "Color mixer",
                Some("Find any color, then use it or save it to your colors."),
            )
        })
    });
    ui.space(2.0);
    ui.label(
        "Saved colors show with a dashed edge in every row above.",
        HINT,
        theme::MUTED,
    );
    ui.space(10.0);
    ui.row(WHEEL, |ui| {
        ui.color_wheel(id("settings.wheel"), &mut view.mixer, WHEEL);
        ui.space(18.0);
        ui.column(|ui| {
            ui.space(15.0);
            brightness(ui, &mut view.mixer);
            ui.row(MIX_CHIP_H, |ui| chip_and_hex(ui, view));
            let color = view.mixer.to_rgb();
            ui.row(theme::CONTROL_HEIGHT, |ui| {
                let changes: [fn(Rgb) -> SettingsChange; 3] = [
                    SettingsChange::NumberColor,
                    SettingsChange::LabelColor,
                    SettingsChange::BackgroundColor,
                ];
                for (i, (text, change)) in USE_FOR.iter().zip(changes).enumerate() {
                    if ui.button(id_with(id("settings.use"), i), text) {
                        out.push(UiAction::Change(change(color)));
                    }
                }
            });
            ui.row(theme::CONTROL_HEIGHT, |ui| {
                if ui.button(id("settings.save_color"), SAVE) {
                    if let Some(list) = with_saved(saved, color) {
                        out.push(UiAction::Change(SettingsChange::SavedColors(list)));
                    }
                }
            });
        });
    });
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
        ui.next_width(70.0);
        ui.column(|ui| {
            ui.space(((theme::SLIDER_H - TextStyle::BODY.size) / 2.0).max(0.0));
            ui.label("Brightness", TextStyle::BODY, theme::MUTED);
        });
        let mut v = (mixer.v * 100.0).round() as i64;
        ui.next_width(SLIDER_W);
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
    ui.next_width(theme::HEX_W + 14.0);
    if ui.text_field(hex_id, &mut view.mixer_hex, "") == TextFieldEvent::Committed {
        if let Ok(c) = Rgb::parse_hex(view.mixer_hex.text.trim()) {
            view.mixer = Hsv::from_rgb(c);
        }
        view.mixer_hex = TextEdit::new(&view.mixer.to_rgb().to_hex());
    }
}
