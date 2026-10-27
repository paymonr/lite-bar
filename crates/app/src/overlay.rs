//! Click-through, never-activating, top-most overlay window.
//!
//! The overlay's look comes from core `overlay_cmds` (shared with the settings preview), painted
//! by `Painter` through a software `ID2D1DCRenderTarget` into a 32-bit premultiplied top-down DIB
//! and pushed to a layered window with `UpdateLayeredWindow`, so no GPU driver is loaded.

use std::sync::Once;

use lite_bar_core::layout::{overlay_origin, Size, StatsText, BASE_MARGIN_PX};
use lite_bar_core::overlay_draw::overlay_cmds;
use lite_bar_core::settings::{OverlaySettings, StatsLayout};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, D2D1_FACTORY_TYPE_SINGLE_THREADED,
};
use windows::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, SetWindowPos, ShowWindow,
    UpdateLayeredWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE,
    SW_SHOWNOACTIVATE, ULW_ALPHA, WM_CLOSE, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::ui_paint::{Canvas, Painter, OVERLAY_MEASURE_CACHE};

const CLASS_NAME: PCWSTR = w!("LiteBarOverlay");
/// Placeholder shown until the first `update`.
const INITIAL_TEXT: &str = "—";

/// `round(base × scale)` in pixels.
fn px(base: f32, scale: f32) -> f32 {
    (base * scale).round()
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // `Overlay` owns the window; a stray WM_CLOSE must not destroy it (DefWindowProc would).
    if msg == WM_CLOSE {
        return LRESULT(0);
    }
    // SAFETY: forwards the arguments unchanged from the system.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn register_window_class() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        // SAFETY: plain FFI; the class name is a static NUL-terminated string. A failed
        // registration (e.g. class already exists) surfaces as a CreateWindowExW error.
        unsafe {
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(window_proc),
                hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            RegisterClassExW(&class);
        }
    });
}

/// Owns the overlay window; destroyed on drop.
struct OwnedWindow(HWND);

impl Drop for OwnedWindow {
    fn drop(&mut self) {
        // SAFETY: the window was created on this thread and is destroyed exactly once.
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

pub struct Overlay {
    window: OwnedWindow,
    /// Holds a reference to the canvas's target, so it is dropped first.
    painter: Painter,
    canvas: Canvas,
    settings: OverlaySettings,
    text: String,
    stats: Option<StatsText>,
    scale: f32,
    size: Size,
    pos: Option<(i32, i32)>,
    visible: bool,
    dirty: bool,
}

impl Overlay {
    pub fn new(settings: &OverlaySettings) -> windows::core::Result<Overlay> {
        register_window_class();
        // SAFETY: plain FFI calls; every handle/interface is owned by the returned value or
        // dropped on the error path.
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE,
                CLASS_NAME,
                w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(GetModuleHandleW(None)?.into()),
                None,
            )?;
            let window = OwnedWindow(hwnd);

            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;

            Ok(Overlay {
                window,
                painter: Painter::new(OVERLAY_MEASURE_CACHE)?,
                canvas: Canvas::transparent(&d2d)?,
                settings: settings.clone(),
                text: INITIAL_TEXT.to_owned(),
                stats: None,
                scale: 1.0,
                size: Size { w: 1, h: 1 },
                pos: None,
                visible: false,
                dirty: true,
            })
        }
    }

    /// The overlay's window handle (tests inspect the window's styles and visibility).
    #[cfg(test)]
    pub fn hwnd(&self) -> HWND {
        self.window.0
    }

    pub fn set_settings(&mut self, s: &OverlaySettings) {
        if *s != self.settings {
            self.settings = s.clone();
            if self.settings.stats == StatsLayout::Off {
                self.stats = None;
            }
            self.dirty = true;
        }
    }

    /// Redraws now with the current text (e.g. right after `set_settings`). Hidden: nothing
    /// to do, the next `show` draws.
    pub fn redraw(&mut self) {
        if self.visible {
            self.refresh();
        }
    }

    #[cfg(test)]
    pub fn settings(&self) -> &OverlaySettings {
        &self.settings
    }

    /// Sets the FPS text and the stats line (`None`, or a settings layout of `Off`, draws the
    /// plain counter) and re-runs placement. Only redraws when the FPS text, the stats text,
    /// the settings or the DPI changed.
    pub fn update(&mut self, text: &str, stats: Option<&StatsText>) {
        if text != self.text {
            self.text = text.to_owned();
            self.dirty = true;
        }
        let stats = stats.filter(|_| self.settings.stats != StatsLayout::Off);
        if stats != self.stats.as_ref() {
            self.stats = stats.cloned();
            self.dirty = true;
        }
        self.refresh();
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.refresh();
        // SAFETY: plain FFI on a window we own. SW_SHOWNOACTIVATE never takes focus.
        unsafe {
            let _ = ShowWindow(self.window.0, SW_SHOWNOACTIVATE);
        }
        self.assert_topmost();
    }

    pub fn hide(&mut self) {
        self.visible = false;
        // SAFETY: plain FFI on a window we own.
        unsafe {
            let _ = ShowWindow(self.window.0, SW_HIDE);
        }
    }

    /// Redraws if needed, then places the window in its location on the foreground monitor.
    fn refresh(&mut self) {
        let (area, scale) = crate::foreground::foreground_monitor();
        if scale != self.scale {
            self.scale = scale;
            self.dirty = true;
        }

        let mut redrawn = false;
        if self.dirty {
            // On failure keep `dirty` so the next call retries; the old frame stays on screen.
            if let Ok(size) = self.draw() {
                self.size = size;
                self.dirty = false;
                redrawn = true;
            }
        }

        let margin = px(BASE_MARGIN_PX as f32, self.scale) as i32;
        let origin = overlay_origin(self.settings.location, area, self.size, margin);
        if redrawn {
            // Pushes bitmap, size and position in one call, so the window never shows a new
            // size at the old position.
            if self.push(origin).is_ok() {
                self.pos = Some(origin);
            }
            self.assert_topmost();
        } else if self.pos != Some(origin) {
            self.move_to(origin);
        } else {
            self.assert_topmost();
        }
    }

    /// Re-asserts top-most z-order without moving, resizing or activating. Visible only.
    fn assert_topmost(&self) {
        if !self.visible {
            return;
        }
        // SAFETY: plain FFI on a window we own.
        unsafe {
            let _ = SetWindowPos(
                self.window.0,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
        }
    }

    fn move_to(&mut self, (x, y): (i32, i32)) {
        // SAFETY: plain FFI on a window we own. Without SWP_SHOWWINDOW a hidden window stays hidden.
        let moved = unsafe {
            SetWindowPos(
                self.window.0,
                Some(HWND_TOPMOST),
                x,
                y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE,
            )
        };
        if moved.is_ok() {
            self.pos = Some((x, y));
        }
    }

    /// Paints the current text into the DIB and returns its size in pixels.
    fn draw(&mut self) -> windows::core::Result<Size> {
        let scale = self.scale;
        let (dip, cmds) = overlay_cmds(
            &self.painter.measurer(scale),
            &self.settings,
            &self.text,
            self.stats.as_ref(),
        );
        // Rounded like the painter rounds the background rect, so it fills the DIB exactly.
        let size = Size {
            w: (px(dip.w as f32, scale) as i32).max(1),
            h: (px(dip.h as f32, scale) as i32).max(1),
        };
        let painter = &mut self.painter;
        self.canvas.draw(size, |target| {
            // SAFETY: plain COM call inside the canvas's BeginDraw/EndDraw.
            unsafe { target.Clear(Some(&D2D1_COLOR_F::default())) };
            painter.paint(target, &cmds, scale)
        })?;
        Ok(size)
    }

    /// Pushes the DIB to the layered window at `origin`.
    fn push(&self, origin: (i32, i32)) -> windows::core::Result<()> {
        let size = self.size;
        let dst = POINT {
            x: origin.0,
            y: origin.1,
        };
        let sz = SIZE {
            cx: size.w,
            cy: size.h,
        };
        let src = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: all pointers refer to live locals; the canvas's DC holds the DIB drawn in `draw`.
        unsafe {
            UpdateLayeredWindow(
                self.window.0,
                None,
                Some(&dst),
                Some(&sz),
                Some(self.canvas.dc()),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lite_bar_core::color::Rgb;
    use lite_bar_core::layout::stats_text;
    use lite_bar_core::settings::{Location, Settings, StatsLayout};
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, GetWindowRect, IsWindow, IsWindowVisible, SendMessageW, GWL_EXSTYLE,
        WM_CLOSE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_EX_TRANSPARENT,
    };

    #[test]
    fn wm_close_does_not_destroy_the_overlay_window() {
        let o = Overlay::new(&Settings::default().overlay).unwrap();
        unsafe { SendMessageW(o.hwnd(), WM_CLOSE, None, None) };
        assert!(unsafe { IsWindow(Some(o.hwnd())) }.as_bool());
    }

    fn window_size(o: &Overlay) -> (i32, i32) {
        let mut r = RECT::default();
        unsafe { GetWindowRect(o.hwnd(), &mut r) }.unwrap();
        (r.right - r.left, r.bottom - r.top)
    }

    #[test]
    fn overlay_window_is_click_through_and_never_activates() {
        let o = Overlay::new(&Settings::default().overlay).unwrap();
        let ex = unsafe { GetWindowLongPtrW(o.hwnd(), GWL_EXSTYLE) } as u32;
        for flag in [
            WS_EX_LAYERED,
            WS_EX_TRANSPARENT,
            WS_EX_TOPMOST,
            WS_EX_TOOLWINDOW,
            WS_EX_NOACTIVATE,
        ] {
            assert!(ex & flag.0 != 0, "missing {flag:?}");
        }
    }

    #[test]
    fn update_and_settings_change_do_not_fail() {
        let mut o = Overlay::new(&Settings::default().overlay).unwrap();
        o.show();
        o.update("144", None);
        o.update("—", None);
        let mut s = Settings::default().overlay;
        s.location = Location::BottomRight;
        s.background_opacity = 0;
        o.set_settings(&s);
        o.update("60", None);
        o.hide();
    }

    #[test]
    fn show_and_hide_toggle_visibility() {
        let mut o = Overlay::new(&Settings::default().overlay).unwrap();
        assert!(!unsafe { IsWindowVisible(o.hwnd()) }.as_bool());
        o.show();
        assert!(unsafe { IsWindowVisible(o.hwnd()) }.as_bool());
        o.hide();
        assert!(!unsafe { IsWindowVisible(o.hwnd()) }.as_bool());
    }

    #[test]
    fn box_width_does_not_jump_between_two_and_three_digits() {
        let mut o = Overlay::new(&Settings::default().overlay).unwrap();
        o.update("99", None);
        let two = window_size(&o);
        o.update("100", None);
        let three = window_size(&o);
        assert!(two.0 > 0 && two.1 > 0, "{two:?}");
        assert_eq!(two, three);
    }

    #[test]
    fn larger_counter_size_makes_a_larger_window() {
        let mut s = Settings::default().overlay;
        let mut o = Overlay::new(&s).unwrap();
        o.update("144", None);
        let small = window_size(&o);
        s.counter_size = 36;
        o.set_settings(&s);
        o.update("144", None);
        let big = window_size(&o);
        assert!(big.0 > small.0 && big.1 > small.1, "{small:?} -> {big:?}");
    }

    #[test]
    fn larger_text_size_makes_a_larger_window() {
        let mut s = Settings::default().overlay;
        let mut o = Overlay::new(&s).unwrap();
        o.update("144", None);
        let small = window_size(&o);
        s.text_size = 30;
        o.set_settings(&s);
        o.update("144", None);
        let big = window_size(&o);
        assert!(big.0 > small.0, "{small:?} -> {big:?}");
    }

    #[test]
    fn painted_background_is_premultiplied() {
        let mut s = Settings::default().overlay;
        s.background_color = Rgb {
            r: 0xFF,
            g: 0,
            b: 0,
        };
        s.background_opacity = 40;
        let mut o = Overlay::new(&s).unwrap();
        o.update("144", None);
        let [b, g, r, a] = o.canvas.pixel_bgra(2, o.size.h / 2);
        // Straight alpha would keep R at 255; premultiplied R is 255 × 0.4.
        assert!(a.abs_diff(102) <= 1, "alpha {a}");
        assert!(r.abs_diff(102) <= 1, "red {r}");
        assert_eq!((g, b), (0, 0));
    }

    #[test]
    fn painted_overlay_shows_background_opacity_and_number_color() {
        let mut s = Settings::default().overlay;
        s.background_color = Rgb { r: 0, g: 0, b: 0 };
        s.background_opacity = 40;
        s.number_color = Rgb {
            r: 0x20,
            g: 0xF0,
            b: 0x40,
        };
        s.stats = StatsLayout::Off;
        let mut o = Overlay::new(&s).unwrap();
        o.update("888", None);
        let (w, h) = window_size(&o);
        let dib = &o.canvas;
        assert_eq!(o.size, Size { w, h });
        // Left padding, away from the rounded corners: background only.
        let bg = dib.pixel_bgra(2, h / 2);
        assert_eq!(bg[..3], [0, 0, 0], "black background");
        assert!(bg[3].abs_diff(102) <= 1, "alpha {} for 40%", bg[3]);
        // Transparent outside the rounded corner.
        assert_eq!(dib.pixel_bgra(0, 0)[3], 0, "corner is cut");
        // Fully covered glyph pixels show the opaque number color.
        let number_px = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let p = dib.pixel_bgra(x, y);
                p[3] == 255
                    && p[0].abs_diff(0x40) <= 2
                    && p[1].abs_diff(0xF0) <= 2
                    && p[2].abs_diff(0x20) <= 2
            })
            .count();
        assert!(number_px > 10, "{number_px} number-colored pixels");
    }

    #[test]
    fn creating_and_dropping_several_overlays_in_sequence_works() {
        let s = Settings::default().overlay;
        let mut hwnds = Vec::new();
        for i in 0..5 {
            let mut o = Overlay::new(&s).unwrap();
            o.show();
            o.update(&format!("{}", 100 + i), None);
            hwnds.push(o.hwnd());
            drop(o);
        }
        for h in hwnds {
            assert!(!unsafe { IsWindow(Some(h)) }.as_bool(), "window leaked");
        }
    }

    /// Window size after showing "144" with the given layout and (optionally) stats.
    fn size_with(layout: StatsLayout, stats: bool) -> (i32, i32) {
        let mut s = Settings::default().overlay;
        s.stats = layout;
        let mut o = Overlay::new(&s).unwrap();
        let text = stats_text(None);
        o.update("144", stats.then_some(&text));
        window_size(&o)
    }

    #[test]
    fn inline_stats_make_the_box_wider() {
        let plain = size_with(StatsLayout::Inline, false);
        let inline = size_with(StatsLayout::Inline, true);
        assert!(inline.0 > plain.0, "{plain:?} -> {inline:?}");
        assert!(inline.1 >= plain.1, "{plain:?} -> {inline:?}");
    }

    #[test]
    fn stacked_stats_make_the_box_taller_not_wider_than_inline() {
        let plain = size_with(StatsLayout::Stacked, false);
        let stacked = size_with(StatsLayout::Stacked, true);
        let inline = size_with(StatsLayout::Inline, true);
        assert!(stacked.1 > plain.1, "{plain:?} -> {stacked:?}");
        assert!(
            stacked.1 > inline.1,
            "stacked {stacked:?} inline {inline:?}"
        );
        assert!(
            stacked.0 < inline.0,
            "stacked {stacked:?} inline {inline:?}"
        );
    }

    #[test]
    fn no_stats_keeps_the_m1_size() {
        let m1 = size_with(StatsLayout::Off, false);
        assert_eq!(size_with(StatsLayout::Inline, false), m1);
        assert_eq!(size_with(StatsLayout::Stacked, false), m1);
        // Layout Off ignores any stats text it is handed.
        assert_eq!(size_with(StatsLayout::Off, true), m1);
    }

    #[test]
    fn dropping_the_stats_text_shrinks_the_box_back() {
        let mut o = Overlay::new(&Settings::default().overlay).unwrap();
        let text = stats_text(None);
        o.update("144", None);
        let plain = window_size(&o);
        o.update("144", Some(&text));
        assert_ne!(window_size(&o), plain);
        o.update("144", None);
        assert_eq!(window_size(&o), plain);
    }
}
