//! Hotkeys section: the show / hide capture field and the host's status line.

use super::{end_card, setting_row, SettingsView, UiAction, HINT};
use crate::settings::Settings;
use crate::ui::{id, theme, HotkeyFieldEvent, Rgba, Ui};

/// Status text for a hotkey another app already holds.
const TAKEN: Rgba = Rgba(0xFF, 0x8A, 0x80, 255);

pub(super) fn section(
    ui: &mut Ui,
    view: &mut SettingsView,
    settings: &Settings,
    status: Option<&str>,
    out: &mut Vec<UiAction>,
) {
    let current = &settings.hotkeys.toggle_overlay;
    ui.card(|ui| {
        let w = ui.hotkey_field_width(current, &view.hotkey);
        setting_row(
            ui,
            40.0,
            "Show / hide overlay",
            Some("Click, then press the new keys. Esc cancels."),
            w,
            |ui| match ui.hotkey_field(id("settings.hotkey"), current, &mut view.hotkey) {
                HotkeyFieldEvent::Proposed(h) => out.push(UiAction::ProposeHotkey(h)),
                HotkeyFieldEvent::Cancelled => out.push(UiAction::CancelHotkeyCapture),
                HotkeyFieldEvent::None | HotkeyFieldEvent::Invalid(_) => {}
            },
        );
        if ui.hotkey_field_width(current, &view.hotkey) != w {
            // The field's text changed with its state; lay it out again right away.
            ui.request_redraw();
        }
        if let Some(status) = status {
            let color = if status.starts_with("Already") {
                TAKEN
            } else {
                theme::MUTED
            };
            let tw = ui.text_measure().measure(status, HINT).0;
            ui.row(HINT.size + 4.0, |ui| {
                ui.right_aligned(tw, |ui| ui.label(status, HINT, color));
            });
        }
    });
    end_card(ui);
}
