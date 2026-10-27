//! Overlay section: show toggle, location and stats layout.

use super::{end_card, row_line, setting_row, UiAction, ROW_H};
use crate::settings::{OverlaySettings, StatsLayout};
use crate::settings_edit::SettingsChange;
use crate::ui::{id, theme, TextStyle, Ui};

const STATS: [(StatsLayout, &str); 3] = [
    (StatsLayout::Off, "Off"),
    (StatsLayout::Inline, "Inline"),
    (StatsLayout::Stacked, "Stacked"),
];

pub(super) fn section(ui: &mut Ui, s: &OverlaySettings, out: &mut Vec<UiAction>) {
    let mut emit = |c: SettingsChange| out.push(UiAction::Change(c));
    ui.card(|ui| {
        setting_row(
            ui,
            ROW_H,
            Some("Show overlay"),
            &[],
            theme::TOGGLE_W,
            |ui| {
                let mut v = s.visible;
                if ui.toggle(id("settings.visible"), &mut v) {
                    emit(SettingsChange::Visible(v));
                }
            },
        );
        row_line(ui);
        setting_row(
            ui,
            theme::SCREEN_H,
            Some("Location"),
            &["Where the counter sits on screen"],
            theme::SCREEN_W,
            |ui| {
                let mut loc = s.location;
                if ui.location_picker(id("settings.location"), &mut loc) {
                    emit(SettingsChange::Location(loc));
                }
            },
        );
        row_line(ui);
        let labels = STATS.map(|(_, l)| l);
        let m = ui.text_measure();
        let w: f32 = labels
            .iter()
            .map(|l| m.measure(l, TextStyle::BODY).0 + 2.0 * theme::SEGMENT_PAD_X)
            .sum();
        setting_row(
            ui,
            ROW_H,
            Some("Stats"),
            &["avg, 1% low and 1% high"],
            w,
            |ui| {
                let mut sel = STATS.iter().position(|(l, _)| *l == s.stats).unwrap_or(0);
                if ui.segmented(id("settings.stats"), &mut sel, &labels) {
                    emit(SettingsChange::Stats(STATS[sel].0));
                }
            },
        );
    });
    end_card(ui);
}
