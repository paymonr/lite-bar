//! The uninstall confirmation page, in the style of the welcome popup. Pure: the host (a
//! small Win32 window) runs the frame, applies the returned actions and feeds back the
//! uninstall's status.

use crate::popup::{
    footer_band, paragraph, right_buttons, wrapped, FOOTER_H, HEADING_STYLE, PAD, PATH_STYLE,
};
use crate::settings_ui::PROBLEM_TEXT;
use crate::ui::{id, theme, Rect, TextStyle, Ui};

/// Window client size in DIPs.
pub const UNINSTALL_SIZE_DIP: (f32, f32) = (440.0, 280.0);
/// The window's title bar text.
pub const UNINSTALL_TITLE: &str = "Lite Bar";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UninstallStatus {
    Confirm,
    Working,
    Done(String),
    Failed(String),
}

pub struct UninstallInput<'a> {
    /// An installed copy (Program Files); otherwise a portable one that only loses its
    /// startup entry and data.
    pub installed: bool,
    /// The settings and stats folder, shown under the toggle.
    pub data_dir: &'a str,
    pub delete_data: bool,
    pub status: &'a UninstallStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UninstallAction {
    Uninstall,
    Cancel,
    Close,
    SetDeleteData(bool),
}

const ROW_H: f32 = 36.0;
const UNINSTALL: &str = "Uninstall";
const REMOVE: &str = "Remove";
const CANCEL: &str = "Cancel";
const CLOSE: &str = "Close";
const WORKING: &str = "Uninstalling…";

/// Draws the page in the Ui's area and returns what the user chose: the question, a
/// toggle for deleting settings and stats (with where they live), then a footer band
/// with the buttons. While working only the status shows; afterwards the message and
/// Close.
pub fn uninstall_page(ui: &mut Ui, input: &UninstallInput) -> Vec<UninstallAction> {
    let mut out = Vec::new();
    let a = ui.available();
    let footer = Rect::new(a.x, a.bottom() - FOOTER_H, a.w, FOOTER_H);
    let inner = Rect::new(a.x + PAD, a.y + PAD, a.w - 2.0 * PAD, footer.y - a.y - PAD);
    ui.region(inner, |ui| match input.status {
        UninstallStatus::Confirm => confirm_body(ui, input, &mut out),
        UninstallStatus::Working => ui.label(WORKING, TextStyle::BODY, theme::MUTED),
        UninstallStatus::Done(m) => paragraph(ui, m, theme::TEXT),
        UninstallStatus::Failed(m) => paragraph(ui, m, PROBLEM_TEXT),
    });
    footer_band(ui, footer, |ui| match input.status {
        UninstallStatus::Confirm => {
            let main = if input.installed { UNINSTALL } else { REMOVE };
            let picked = right_buttons(
                ui,
                &[
                    (id("uninstall.confirm"), main, true),
                    (id("uninstall.cancel"), CANCEL, false),
                ],
            );
            out.extend(picked.map(|i| [UninstallAction::Uninstall, UninstallAction::Cancel][i]));
        }
        UninstallStatus::Working => {}
        UninstallStatus::Done(_) | UninstallStatus::Failed(_) => {
            if right_buttons(ui, &[(id("uninstall.close"), CLOSE, false)]).is_some() {
                out.push(UninstallAction::Close);
            }
        }
    });
    out
}

fn confirm_body(ui: &mut Ui, input: &UninstallInput, out: &mut Vec<UninstallAction>) {
    let heading = if input.installed {
        "Uninstall Lite Bar?"
    } else {
        "Remove Lite Bar's startup entry and data?"
    };
    wrapped(ui, heading, HEADING_STYLE, theme::TEXT);
    ui.space(16.0);
    ui.row(ROW_H, |ui| {
        ui.right_aligned(theme::TOGGLE_W, |ui| {
            let mut v = input.delete_data;
            if ui.toggle(id("uninstall.delete_data"), &mut v) {
                out.push(UninstallAction::SetDeleteData(v));
            }
        });
        ui.label(
            "Also delete my settings and stats",
            TextStyle::BODY,
            theme::TEXT,
        );
    });
    ui.label(input.data_dir, PATH_STYLE, theme::MUTED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::popup::{FOOTER_H, PAD};
    use crate::settings_ui::PROBLEM_TEXT;
    use crate::ui::{
        theme, Cmd, Frame, Input, Pt, Rect, Rgba, TextMeasure, TextStyle, Ui, UiState,
    };
    use crate::uninstall::{finish_message, UninstallStep, QUIT_FAILED};

    struct Fake {
        char_w: f32,
        line: f32,
    }
    impl TextMeasure for Fake {
        fn measure(&self, text: &str, style: TextStyle) -> (f32, f32) {
            (
                self.char_w * text.chars().count() as f32,
                self.line * style.size,
            )
        }
        fn baseline(&self, style: TextStyle) -> f32 {
            0.8 * style.size
        }
    }
    const FAKE: Fake = Fake {
        char_w: 7.0,
        line: 1.0,
    };
    const DIR: &str = r"C:\Users\someone\AppData\Roaming\LiteBar";
    const UNINSTALL: &str = "Uninstall";
    const CANCEL: &str = "Cancel";

    fn area() -> Rect {
        Rect::new(0.0, 0.0, UNINSTALL_SIZE_DIP.0, UNINSTALL_SIZE_DIP.1)
    }

    fn run(
        st: &mut UiState,
        m: &Fake,
        installed: bool,
        delete_data: bool,
        status: &UninstallStatus,
        input: Input,
    ) -> (Vec<UninstallAction>, Frame) {
        let mut ui = Ui::new(m, &input, st, area());
        let actions = uninstall_page(
            &mut ui,
            &UninstallInput {
                installed,
                data_dir: DIR,
                delete_data,
                status,
            },
        );
        (actions, ui.finish())
    }

    fn texts(f: &Frame) -> Vec<(Pt, String, TextStyle, Rgba)> {
        f.cmds
            .iter()
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

    fn find_text(f: &Frame, text: &str) -> Option<(Pt, TextStyle, Rgba)> {
        texts(f)
            .into_iter()
            .find(|t| t.1 == text)
            .map(|t| (t.0, t.2, t.3))
    }

    fn center_of(f: &Frame, text: &str) -> Pt {
        let (pos, style, _) = find_text(f, text).unwrap_or_else(|| panic!("no text {text:?}"));
        Pt {
            x: pos.x + 3.5 * text.chars().count() as f32,
            y: pos.y + style.size / 2.0,
        }
    }

    fn frame(installed: bool, delete_data: bool, status: &UninstallStatus) -> Frame {
        run(
            &mut UiState::default(),
            &FAKE,
            installed,
            delete_data,
            status,
            Input::default(),
        )
        .1
    }

    /// Settles the layout, then hovers, presses and releases at `p`.
    fn click_at(
        installed: bool,
        delete_data: bool,
        status: &UninstallStatus,
        p: Pt,
    ) -> Vec<UninstallAction> {
        let mut st = UiState::default();
        let mut go = |input| run(&mut st, &FAKE, installed, delete_data, status, input).0;
        go(Input::default());
        let at = |down, pressed, released| Input {
            pointer: Some(p),
            down,
            pressed,
            released,
            ..Input::default()
        };
        let mut out = go(at(false, false, false));
        out.extend(go(at(true, true, false)));
        out.extend(go(at(false, false, true)));
        out
    }

    /// The lines drawn in `color`, joined with spaces (a wrapped message reads as one).
    fn joined(f: &Frame, color: Rgba) -> String {
        let mut v: Vec<_> = texts(f).into_iter().filter(|t| t.3 == color).collect();
        v.sort_by(|a, b| a.0.y.total_cmp(&b.0.y));
        v.into_iter().map(|t| t.1).collect::<Vec<_>>().join(" ")
    }

    fn toggle_track(f: &Frame) -> Rect {
        f.cmds
            .iter()
            .find_map(|c| match c {
                Cmd::Fill { rect, .. } if rect.w == theme::TOGGLE_W => Some(*rect),
                _ => None,
            })
            .expect("toggle")
    }

    #[test]
    fn confirm_installed_shows_heading_toggle_path_and_buttons() {
        let f = frame(true, false, &UninstallStatus::Confirm);
        let (_, heading, _) = find_text(&f, "Uninstall Lite Bar?").expect("heading");
        assert!(heading.bold && heading.size > TextStyle::BODY.size);
        let (label, _, _) = find_text(&f, "Also delete my settings and stats").expect("label");
        let track = toggle_track(&f);
        assert!(label.x < track.x, "label left of the toggle");
        assert!(
            (track.right() - (UNINSTALL_SIZE_DIP.0 - PAD)).abs() < 1.0,
            "toggle at the right padding"
        );
        let (dir, style, color) = find_text(&f, DIR).expect("data dir");
        assert!(style.mono && color == theme::MUTED);
        assert!(dir.y > label.y, "path under the toggle row");

        let (u, ustyle, _) = find_text(&f, UNINSTALL).unwrap();
        let (c, cstyle, _) = find_text(&f, CANCEL).unwrap();
        assert!(ustyle.bold, "accent button is bold");
        assert!(u.x < c.x, "Uninstall before Cancel");
        assert!(
            u.y > UNINSTALL_SIZE_DIP.1 - FOOTER_H,
            "buttons in the footer"
        );
        let right = c.x + FAKE.measure(CANCEL, cstyle).0 + theme::BUTTON_PAD_X;
        assert!((right - (UNINSTALL_SIZE_DIP.0 - PAD)).abs() < 1.0);
    }

    #[test]
    fn confirm_portable_wording() {
        let f = frame(false, false, &UninstallStatus::Confirm);
        assert!(find_text(&f, "Remove Lite Bar's startup entry and data?").is_some());
        assert!(find_text(&f, "Remove").is_some());
        assert!(find_text(&f, UNINSTALL).is_none());
        assert!(find_text(&f, "Uninstall Lite Bar?").is_none());
    }

    #[test]
    fn clicks_emit_actions() {
        let s = UninstallStatus::Confirm;
        let f = frame(true, false, &s);
        assert_eq!(
            click_at(true, false, &s, center_of(&f, UNINSTALL)),
            [UninstallAction::Uninstall]
        );
        assert_eq!(
            click_at(true, false, &s, center_of(&f, CANCEL)),
            [UninstallAction::Cancel]
        );
        let t = toggle_track(&f);
        let at = Pt {
            x: t.x + t.w / 2.0,
            y: t.y + t.h / 2.0,
        };
        assert_eq!(
            click_at(true, false, &s, at),
            [UninstallAction::SetDeleteData(true)]
        );
        assert_eq!(
            click_at(true, true, &s, at),
            [UninstallAction::SetDeleteData(false)]
        );
    }

    #[test]
    fn working_shows_status_and_no_buttons() {
        let f = frame(true, false, &UninstallStatus::Working);
        let (_, _, color) = find_text(&f, "Uninstalling…").expect("status");
        assert_eq!(color, theme::MUTED);
        for b in [UNINSTALL, CANCEL, "Close"] {
            assert!(find_text(&f, b).is_none(), "{b} drawn");
        }
    }

    #[test]
    fn done_shows_message_and_close() {
        let s = UninstallStatus::Done("Lite Bar was uninstalled.".into());
        let f = frame(true, false, &s);
        let (_, _, color) = find_text(&f, "Lite Bar was uninstalled.").expect("message");
        assert_eq!(color, theme::TEXT);
        for b in [UNINSTALL, CANCEL] {
            assert!(find_text(&f, b).is_none(), "{b} drawn");
        }
        assert_eq!(
            click_at(true, false, &s, center_of(&f, "Close")),
            [UninstallAction::Close]
        );
    }

    #[test]
    fn failed_uses_problem_color() {
        let s = UninstallStatus::Failed(QUIT_FAILED.into());
        let f = frame(true, false, &s);
        assert_eq!(joined(&f, PROBLEM_TEXT), QUIT_FAILED);
        assert!(find_text(&f, "Close").is_some());
        for b in [UNINSTALL, CANCEL] {
            assert!(find_text(&f, b).is_none(), "{b} drawn");
        }
        assert_eq!(
            click_at(true, false, &s, center_of(&f, "Close")),
            [UninstallAction::Close]
        );
    }

    #[test]
    fn fits_uninstall_size() {
        let wide = Fake {
            char_w: 7.5,
            line: 1.35,
        };
        use UninstallStep::*;
        let longest = finish_message(&[
            RemoveStartup,
            RemoveShortcut,
            RemoveData,
            RemoveProgram,
            RemoveEntry,
        ]);
        for installed in [true, false] {
            for s in [
                UninstallStatus::Confirm,
                UninstallStatus::Working,
                UninstallStatus::Done(longest.clone()),
                UninstallStatus::Failed(QUIT_FAILED.into()),
            ] {
                let f = run(
                    &mut UiState::default(),
                    &wide,
                    installed,
                    true,
                    &s,
                    Input::default(),
                )
                .1;
                let w = UNINSTALL_SIZE_DIP.0;
                let h = UNINSTALL_SIZE_DIP.1;
                for (pos, text, style, _) in texts(&f) {
                    let (tw, th) = wide.measure(&text, style);
                    assert!(pos.x + tw <= w - PAD, "{text:?} runs off right");
                    assert!(pos.y + th <= h, "{text:?} runs off bottom");
                    let in_footer = [UNINSTALL, CANCEL, "Remove", "Close"].contains(&text.as_str());
                    assert!(
                        in_footer || pos.y + th <= h - FOOTER_H - theme::GAP,
                        "{text:?} runs into the footer"
                    );
                }
                for c in &f.cmds {
                    if let Cmd::Fill { rect, .. } | Cmd::Stroke { rect, .. } = c {
                        assert!(rect.right() <= w && rect.bottom() <= h, "{rect:?} outside");
                    }
                }
            }
        }
    }
}
