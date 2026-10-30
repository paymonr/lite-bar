//! The welcome popup page: offers to move lite-bar.exe to `<Program Files>\Lite Bar`.
//! Pure, like the Settings page: the host (a small Win32 window) runs the frame, applies
//! the returned actions and feeds back the move's status.

use crate::install::{MoveOutcome, MoveStatus};
use crate::settings_ui::PROBLEM_TEXT;
use crate::ui::{id, theme, Rect, Rgba, TextMeasure, TextStyle, Ui};

/// Popup client size in DIPs.
pub const POPUP_SIZE_DIP: (f32, f32) = (440.0, 300.0);
/// The popup's title bar text.
pub const TITLE: &str = "Welcome to Lite Bar";

const PAD: f32 = 20.0;
const MOVE: &str = "Move to Program Files";
const LATER: &str = "Ask me later";
const NO_THANKS: &str = "No thanks";
const CLOSE: &str = "Close";

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

/// Draws the popup in the Ui's area and returns what the user chose.
pub fn move_prompt_frame(ui: &mut Ui, input: &MovePromptInput) -> Vec<MovePromptAction> {
    let mut out = Vec::new();
    let a = ui.available();
    let inner = Rect::new(a.x + PAD, a.y + PAD, a.w - 2.0 * PAD, a.h - 2.0 * PAD);
    ui.region(inner, |ui| {
        paragraph(
            ui,
            "A tiny FPS overlay with 1% lows and per-game stats.",
            theme::TEXT,
        );
        paragraph(
            ui,
            &format!("Press {} to show or hide it.", input.hotkey),
            theme::TEXT,
        );
        paragraph(
            ui,
            "Settings and Quit are in the tray icon menu.",
            theme::TEXT,
        );
        ui.space(theme::GAP);
        paragraph(
            ui,
            "Only administrators can change files in Program Files, so a copy \
             there is safe from tampering. Move it to:",
            theme::TEXT,
        );
        paragraph(ui, input.target_dir, theme::MUTED);
        ui.space(theme::GAP);
        let (text, color) = status_line(input.status);
        if !text.is_empty() {
            paragraph(ui, &text, color);
            ui.space(theme::GAP);
        }
        buttons(ui, input.status, &mut out);
    });
    out
}

/// The status line and its color; empty when there is nothing to say.
fn status_line(status: &MoveStatus) -> (String, Rgba) {
    match status {
        MoveStatus::Idle | MoveStatus::Done(MoveOutcome::Moved) => (String::new(), theme::MUTED),
        MoveStatus::Moving => ("Moving…".into(), theme::MUTED),
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

/// The buttons the state offers; none while moving, only Close once the file has moved.
fn buttons(ui: &mut Ui, status: &MoveStatus, out: &mut Vec<MovePromptAction>) {
    let m = ui.text_measure();
    let w = |label: &str, style| m.measure(label, style).0 + 2.0 * theme::BUTTON_PAD_X;
    match status {
        MoveStatus::Moving => {}
        MoveStatus::Done(MoveOutcome::MovedNotStarted(_)) => {
            ui.row(theme::CONTROL_HEIGHT, |ui| {
                ui.next_width(w(CLOSE, TextStyle::BODY));
                if ui.button(id("move_prompt.close"), CLOSE) {
                    out.push(MovePromptAction::Later);
                }
            });
        }
        _ => ui.row(theme::CONTROL_HEIGHT, |ui| {
            ui.next_width(w(MOVE, TextStyle::BOLD));
            if ui.button_accent(id("move_prompt.move"), MOVE) {
                out.push(MovePromptAction::Move);
            }
            ui.next_width(w(LATER, TextStyle::BODY));
            if ui.button(id("move_prompt.later"), LATER) {
                out.push(MovePromptAction::Later);
            }
            ui.next_width(w(NO_THANKS, TextStyle::BODY));
            if ui.button(id("move_prompt.no_thanks"), NO_THANKS) {
                out.push(MovePromptAction::NoThanks);
            }
        }),
    }
}

/// Body text wrapped at word boundaries to the column's width (the toolkit has no wrapping).
fn paragraph(ui: &mut Ui, text: &str, color: Rgba) {
    let style = TextStyle::BODY;
    let max = ui.available().w;
    for line in wrap(ui.text_measure(), text, style, max) {
        ui.label(&line, style, color);
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
        assert!(has_line(&f, "FPS overlay"));
        assert!(has_line(&f, "Ctrl+Shift+F"));
        assert!(has_line(&f, "tray"));
        assert!(has_line(&f, "1% lows"));
        assert!(has_line(&f, "per-game stats"));
        assert!(has_line(&f, "Settings"));
        assert!(has_line(&f, "Quit"));
        assert!(has_line(&f, "Only administrators"));
        let (_, _, color) = find_text(&f, DIR).expect("target dir");
        assert_eq!(color, theme::MUTED);
        assert!(
            find_text(&f, TITLE).is_none(),
            "the title bar has the title"
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
            }
        }
    }
}
