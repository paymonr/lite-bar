//! The welcome popup page: offers to move lite-bar.exe to `<Program Files>\Lite Bar`.
//! Pure, like the Settings page: the host (a small Win32 window) runs the frame, applies
//! the returned actions and feeds back the move's status.

use crate::install::{MoveOutcome, MoveStatus};
use crate::settings_ui::PROBLEM_TEXT;
use crate::ui::{id, theme, Cmd, Pt, Rect, Rgba, TextMeasure, TextStyle, Ui};

/// Popup client size in DIPs.
pub const POPUP_SIZE_DIP: (f32, f32) = (460.0, 460.0);
/// The popup's title bar text.
pub const TITLE: &str = "Lite Bar";
/// The heading at the top of the page.
pub const HEADING: &str = "Welcome to Lite Bar";

const PAD: f32 = 24.0;
const FOOTER_H: f32 = 64.0;
const ROW_H: f32 = 36.0;
const CAP_H: f32 = 22.0;
const CAP_PAD_X: f32 = 7.0;
const CAP_GAP: f32 = 4.0;
/// Between the lines of a wrapped paragraph.
const LINE_GAP: f32 = 2.0;
const HEADING_STYLE: TextStyle = TextStyle {
    size: 20.0,
    bold: true,
    mono: false,
};
const CAP_STYLE: TextStyle = TextStyle {
    size: 12.0,
    bold: true,
    mono: false,
};
const PATH_STYLE: TextStyle = TextStyle {
    size: 12.0,
    bold: false,
    mono: true,
};
const MOVE: &str = "Move to Program Files";
const LATER: &str = "Ask me later";
const NO_THANKS: &str = "No thanks";
const CLOSE: &str = "Close";
const MOVING: &str = "Moving…";

pub struct MovePromptInput<'a> {
    /// The toggle hotkey as shown to the user, e.g. "Ctrl+Shift+F".
    pub hotkey: &'a str,
    /// The folder the exe would move to.
    pub target_dir: &'a str,
    pub status: &'a MoveStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovePromptAction {
    Move,
    Later,
    NoThanks,
}

/// Draws the popup in the Ui's area and returns what the user chose: a heading, a card
/// with the two things to know (hotkey, tray menu), why to move with the target folder,
/// then a footer band with the buttons.
pub fn move_prompt_frame(ui: &mut Ui, input: &MovePromptInput) -> Vec<MovePromptAction> {
    let mut out = Vec::new();
    let a = ui.available();
    let footer = Rect::new(a.x, a.bottom() - FOOTER_H, a.w, FOOTER_H);
    let inner = Rect::new(a.x + PAD, a.y + PAD, a.w - 2.0 * PAD, footer.y - a.y - PAD);
    ui.region(inner, |ui| {
        ui.label(HEADING, HEADING_STYLE, theme::TEXT);
        ui.space(4.0);
        paragraph(
            ui,
            "A tiny FPS overlay with 1% lows and per-game stats.",
            theme::MUTED,
        );
        ui.space(16.0);
        ui.card(|ui| {
            let keys: Vec<&str> = input.hotkey.split('+').collect();
            card_row(
                ui,
                "Show or hide the overlay",
                caps_width(ui, &keys),
                |ui| key_caps(ui, &keys),
            );
            divider(ui);
            let tray = "In the tray icon menu";
            let w = ui.text_measure().measure(tray, TextStyle::BODY).0;
            card_row(ui, "Settings and Quit", w, |ui| {
                ui.label(tray, TextStyle::BODY, theme::MUTED)
            });
        });
        ui.space(20.0);
        ui.label(
            "Keep it safe in Program Files",
            TextStyle::BOLD,
            theme::TEXT,
        );
        ui.space(4.0);
        paragraph(
            ui,
            "Lite Bar runs as administrator, and only administrators can change files in \
             Program Files, so nothing else can tamper with it there.",
            theme::MUTED,
        );
        ui.space(10.0);
        path_pill(ui, input.target_dir);
        let (text, color) = status_line(input.status);
        if !text.is_empty() && !matches!(input.status, MoveStatus::Moving) {
            ui.space(12.0);
            paragraph(ui, &text, color);
        }
    });
    footer_band(ui, footer, input.status, &mut out);
    out
}

/// A card row: `title` on the left, a `control_w` wide control on the right.
fn card_row<'a>(ui: &mut Ui<'a>, title: &str, control_w: f32, control: impl FnOnce(&mut Ui<'a>)) {
    ui.row(ROW_H, |ui| {
        ui.right_aligned(control_w, control);
        ui.label(title, TextStyle::BODY, theme::TEXT);
    });
}

/// A hairline in the middle of the gap before the next item.
fn divider(ui: &mut Ui) {
    let r = ui.available();
    let y = r.y + theme::GAP / 2.0;
    ui.paint(Cmd::Line {
        from: Pt { x: r.x, y },
        to: Pt { x: r.right(), y },
        color: theme::BORDER,
        width: 1.0,
    });
}

fn cap_width(ui: &Ui, key: &str) -> f32 {
    ui.text_measure().measure(key, CAP_STYLE).0 + 2.0 * CAP_PAD_X
}

fn caps_width(ui: &Ui, keys: &[&str]) -> f32 {
    let w: f32 = keys.iter().map(|k| cap_width(ui, k)).sum();
    w + CAP_GAP * keys.len().saturating_sub(1) as f32
}

/// Each key of the hotkey in its own small outlined cap, vertically centred in the row.
fn key_caps(ui: &mut Ui, keys: &[&str]) {
    let r = ui.available();
    let y = r.y + (r.h - CAP_H) / 2.0;
    let mut x = r.x;
    for key in keys {
        let w = cap_width(ui, key);
        let rect = Rect::new(x, y, w, CAP_H);
        let (tw, th) = ui.text_measure().measure(key, CAP_STYLE);
        ui.paint(Cmd::Fill {
            rect,
            color: theme::BUTTON_BG,
            radius: theme::BUTTON_RADIUS,
        });
        ui.paint(Cmd::Stroke {
            rect,
            color: theme::BUTTON_BORDER,
            radius: theme::BUTTON_RADIUS,
            width: 1.0,
        });
        ui.paint(Cmd::Text {
            pos: Pt {
                x: x + (w - tw) / 2.0,
                y: y + (CAP_H - th) / 2.0,
            },
            text: key.to_string(),
            style: CAP_STYLE,
            color: theme::TEXT,
        });
        x += w + CAP_GAP;
    }
}

/// The target folder in a rounded box, in the monospace font.
fn path_pill(ui: &mut Ui, dir: &str) {
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

/// The bottom band: "Moving…" on the left while a move runs, otherwise the buttons the
/// state offers, right-aligned.
fn footer_band(ui: &mut Ui, rect: Rect, status: &MoveStatus, out: &mut Vec<MovePromptAction>) {
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
    ui.place(strip, |ui| buttons(ui, status, out));
}

/// The status line and its color; empty when there is nothing to say.
fn status_line(status: &MoveStatus) -> (String, Rgba) {
    match status {
        MoveStatus::Idle | MoveStatus::Done(MoveOutcome::Moved) => (String::new(), theme::MUTED),
        MoveStatus::Moving => (MOVING.into(), theme::MUTED),
        MoveStatus::Done(o) => match o {
            MoveOutcome::Declined => (
                "Not moved: Windows didn't get permission.".into(),
                theme::MUTED,
            ),
            MoveOutcome::OtherAccount => (
                "Moving needs your own administrator account. You can move lite-bar.exe \
                 to Program Files by hand."
                    .into(),
                PROBLEM_TEXT,
            ),
            MoveOutcome::Failed(e) => (format!("Couldn't move Lite Bar: {e}"), PROBLEM_TEXT),
            MoveOutcome::MovedNotStarted(e) => (
                format!("Moved, but couldn't start the new copy: {e}"),
                PROBLEM_TEXT,
            ),
            MoveOutcome::Moved => (String::new(), theme::MUTED),
        },
    }
}

/// The buttons the state offers, right-aligned; "Moving…" instead while moving, only
/// Close once the file has moved.
fn buttons(ui: &mut Ui, status: &MoveStatus, out: &mut Vec<MovePromptAction>) {
    let m = ui.text_measure();
    let w = |label: &str, style| m.measure(label, style).0 + 2.0 * theme::BUTTON_PAD_X;
    ui.row(theme::CONTROL_HEIGHT, |ui| match status {
        MoveStatus::Moving => ui.label(MOVING, TextStyle::BODY, theme::MUTED),
        MoveStatus::Done(MoveOutcome::MovedNotStarted(_)) => {
            let close = w(CLOSE, TextStyle::BODY);
            ui.right_aligned(close, |ui| {
                ui.next_width(close);
                if ui.button(id("move_prompt.close"), CLOSE) {
                    out.push(MovePromptAction::Later);
                }
            });
        }
        _ => {
            let (mw, lw, nw) = (
                w(MOVE, TextStyle::BOLD),
                w(LATER, TextStyle::BODY),
                w(NO_THANKS, TextStyle::BODY),
            );
            ui.right_aligned(mw + lw + nw + 2.0 * theme::GAP, |ui| {
                ui.next_width(mw);
                if ui.button_accent(id("move_prompt.move"), MOVE) {
                    out.push(MovePromptAction::Move);
                }
                ui.next_width(lw);
                if ui.button(id("move_prompt.later"), LATER) {
                    out.push(MovePromptAction::Later);
                }
                ui.next_width(nw);
                if ui.button(id("move_prompt.no_thanks"), NO_THANKS) {
                    out.push(MovePromptAction::NoThanks);
                }
            });
        }
    });
}

/// Body text wrapped at word boundaries to the column's width (the toolkit has no wrapping).
fn paragraph(ui: &mut Ui, text: &str, color: Rgba) {
    let style = TextStyle::BODY;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{MoveOutcome, MoveStatus};
    use crate::settings_ui::PROBLEM_TEXT;
    use crate::ui::{
        theme, Cmd, Frame, Input, Pt, Rect, Rgba, TextMeasure, TextStyle, Ui, UiState,
    };

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
    const DIR: &str = r"C:\Program Files\Lite Bar";

    fn area() -> Rect {
        Rect::new(0.0, 0.0, POPUP_SIZE_DIP.0, POPUP_SIZE_DIP.1)
    }

    /// Runs one frame on `st`; returns the actions and the frame.
    fn run(
        st: &mut UiState,
        m: &Fake,
        status: &MoveStatus,
        input: Input,
    ) -> (Vec<MovePromptAction>, Frame) {
        let mut ui = Ui::new(m, &input, st, area());
        let actions = move_prompt_frame(
            &mut ui,
            &MovePromptInput {
                hotkey: "Ctrl+Shift+F",
                target_dir: DIR,
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

    /// Settles the layout, then hovers, presses and releases at `p`.
    fn click_at(status: &MoveStatus, p: Pt) -> Vec<MovePromptAction> {
        let mut st = UiState::default();
        run(&mut st, &FAKE, status, Input::default());
        let at = |down, pressed, released| Input {
            pointer: Some(p),
            down,
            pressed,
            released,
            ..Input::default()
        };
        let mut out = run(&mut st, &FAKE, status, at(false, false, false)).0;
        out.extend(run(&mut st, &FAKE, status, at(true, true, false)).0);
        out.extend(run(&mut st, &FAKE, status, at(false, false, true)).0);
        out
    }

    fn frame(status: &MoveStatus) -> Frame {
        run(&mut UiState::default(), &FAKE, status, Input::default()).1
    }

    fn has_line(f: &Frame, part: &str) -> bool {
        texts(f).iter().any(|t| t.1.contains(part))
    }

    /// The lines drawn in `color`, joined with spaces (a wrapped status reads as one).
    fn joined(f: &Frame, color: Rgba) -> String {
        let mut v: Vec<_> = texts(f).into_iter().filter(|t| t.3 == color).collect();
        v.sort_by(|a, b| a.0.y.total_cmp(&b.0.y));
        v.into_iter().map(|t| t.1).collect::<Vec<_>>().join(" ")
    }

    const MOVE: &str = "Move to Program Files";
    const LATER: &str = "Ask me later";
    const NO: &str = "No thanks";

    #[test]
    fn shows_intro_hotkey_and_target() {
        let f = frame(&MoveStatus::Idle);
        let (_, heading, _) = find_text(&f, HEADING).expect("heading");
        assert!(heading.bold && heading.size > TextStyle::BODY.size);
        assert!(has_line(&f, "FPS overlay"));
        assert!(has_line(&f, "1% lows"));
        assert!(has_line(&f, "per-game stats"));
        assert!(has_line(&f, "Settings and Quit"));
        assert!(has_line(&f, "tray"));
        assert!(has_line(&f, "administrators"));
        let (_, path, _) = find_text(&f, DIR).expect("target dir");
        assert!(path.mono, "the folder reads as a path");
        assert_ne!(TITLE, HEADING, "the title bar doesn't repeat the heading");
    }

    #[test]
    fn hotkey_is_drawn_as_key_caps() {
        let f = frame(&MoveStatus::Idle);
        let y = |t: &str| {
            find_text(&f, t)
                .unwrap_or_else(|| panic!("no cap {t:?}"))
                .0
                .y
        };
        let x = |t: &str| find_text(&f, t).unwrap().0.x;
        assert_eq!(y("Ctrl"), y("Shift"));
        assert_eq!(y("Shift"), y("F"));
        assert!(x("Ctrl") < x("Shift") && x("Shift") < x("F"));
        assert!(!has_line(&f, "Ctrl+Shift+F"), "split into caps");
        // Each key sits inside its own outlined cap.
        for key in ["Ctrl", "Shift", "F"] {
            let (p, _, _) = find_text(&f, key).unwrap();
            let boxed = f.cmds.iter().any(|c| {
                matches!(c, Cmd::Stroke { rect, .. }
                    if rect.x <= p.x && p.x < rect.right() && rect.y <= p.y && p.y < rect.bottom()
                        && rect.h < theme::CONTROL_HEIGHT)
            });
            assert!(boxed, "{key} has no cap");
        }
    }

    #[test]
    fn buttons_sit_right_aligned_in_a_footer() {
        let f = frame(&MoveStatus::Idle);
        let footer = f
            .cmds
            .iter()
            .find_map(|c| match c {
                Cmd::Fill { rect, .. }
                    if rect.x == 0.0
                        && rect.w == POPUP_SIZE_DIP.0
                        && rect.bottom() == POPUP_SIZE_DIP.1 =>
                {
                    Some(*rect)
                }
                _ => None,
            })
            .expect("full-width footer band at the bottom");
        let (dir_pos, _, _) = find_text(&f, DIR).unwrap();
        for b in [MOVE, LATER, NO] {
            let (p, _, _) = find_text(&f, b).unwrap();
            assert!(p.y > footer.y && p.y > dir_pos.y, "{b} in the footer");
        }
        let (no, style, _) = find_text(&f, NO).unwrap();
        let right = no.x + FAKE.measure(NO, style).0 + theme::BUTTON_PAD_X;
        assert!(
            (right - (POPUP_SIZE_DIP.0 - PAD)).abs() < 1.0,
            "buttons end at the right padding: {right}"
        );
    }

    #[test]
    fn buttons_in_order_and_emit_actions() {
        let s = MoveStatus::Idle;
        let f = frame(&s);
        let x = |t| find_text(&f, t).expect(t).0.x;
        assert!(x(MOVE) < x(LATER) && x(LATER) < x(NO));
        assert_eq!(click_at(&s, center_of(&f, MOVE)), [MovePromptAction::Move]);
        assert_eq!(
            click_at(&s, center_of(&f, LATER)),
            [MovePromptAction::Later]
        );
        assert_eq!(
            click_at(&s, center_of(&f, NO)),
            [MovePromptAction::NoThanks]
        );
    }

    #[test]
    fn moving_hides_buttons() {
        let idle = frame(&MoveStatus::Idle);
        let at = center_of(&idle, MOVE);
        let f = frame(&MoveStatus::Moving);
        assert!(find_text(&f, "Moving…").is_some());
        for b in [MOVE, LATER, NO] {
            assert!(find_text(&f, b).is_none(), "{b} still drawn");
        }
        assert!(click_at(&MoveStatus::Moving, at).is_empty());
    }

    #[test]
    fn outcome_lines() {
        let done = |o| MoveStatus::Done(o);
        let f = frame(&done(MoveOutcome::Declined));
        let (_, _, color) =
            find_text(&f, "Not moved: Windows didn't get permission.").expect("declined line");
        assert_eq!(color, theme::MUTED);
        assert!(find_text(&f, MOVE).is_some());

        let f = frame(&done(MoveOutcome::Failed("access denied".into())));
        assert_eq!(
            joined(&f, PROBLEM_TEXT),
            "Couldn't move Lite Bar: access denied"
        );
        assert!(find_text(&f, NO).is_some());

        let f = frame(&done(MoveOutcome::OtherAccount));
        assert_eq!(
            joined(&f, PROBLEM_TEXT),
            "Moving needs your own administrator account. You can move lite-bar.exe to \
             Program Files by hand."
        );
        assert!(find_text(&f, LATER).is_some());

        let s = done(MoveOutcome::MovedNotStarted("x".into()));
        let f = frame(&s);
        assert_eq!(
            joined(&f, PROBLEM_TEXT),
            "Moved, but couldn't start the new copy: x"
        );
        for b in [MOVE, LATER, NO] {
            assert!(find_text(&f, b).is_none(), "{b} drawn");
        }
        assert_eq!(
            click_at(&s, center_of(&f, "Close")),
            [MovePromptAction::Later]
        );
    }

    #[test]
    fn fits_popup_size() {
        let wide = Fake {
            char_w: 7.5,
            line: 1.35,
        };
        let long = MoveStatus::Done(MoveOutcome::Failed("x".repeat(30) + " denied by policy"));
        for s in [
            MoveStatus::Idle,
            MoveStatus::Moving,
            MoveStatus::Done(MoveOutcome::OtherAccount),
            long,
            MoveStatus::Done(MoveOutcome::MovedNotStarted("boom".into())),
        ] {
            let f = run(&mut UiState::default(), &wide, &s, Input::default()).1;
            for (pos, text, style, _) in texts(&f) {
                let (w, h) = wide.measure(&text, style);
                assert!(
                    pos.x + w <= POPUP_SIZE_DIP.0 - PAD,
                    "{text:?} runs off right"
                );
                assert!(pos.y + h <= POPUP_SIZE_DIP.1, "{text:?} runs off bottom");
                let in_footer = [MOVE, LATER, NO, "Close", "Moving…"].contains(&text.as_str());
                assert!(
                    in_footer || pos.y + h <= POPUP_SIZE_DIP.1 - FOOTER_H - theme::GAP,
                    "{text:?} runs into the footer"
                );
            }
        }
    }
}
