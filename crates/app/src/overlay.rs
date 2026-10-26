//! Click-through, never-activating, top-most overlay window drawn with Direct2D.
//!
//! Text is rendered by a software `ID2D1DCRenderTarget` into a 32-bit premultiplied top-down DIB
//! and pushed to a layered window with `UpdateLayeredWindow`, so no GPU driver is loaded.

use std::sync::Once;

use lite_bar_core::color::{rgba_f32, Rgb};
use lite_bar_core::layout::{overlay_origin, Size, BASE_MARGIN_PX};
use lite_bar_core::settings::OverlaySettings;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_LINE_METRICS,
    DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetCurrentObject, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ, OBJ_BITMAP,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, SetWindowPos, ShowWindow,
    UpdateLayeredWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE,
    SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows_numerics::Vector2;

const CLASS_NAME: PCWSTR = w!("LiteBarOverlay");
const FONT_FAMILY: PCWSTR = w!("Consolas");
const LABEL_TEXT: &str = "FPS";
/// Widest text the box always has room for, so it doesn't jump between 99 and 100.
const MIN_NUMBER_TEXT: &str = "000";
/// Label text size relative to the number text size.
const LABEL_SIZE_RATIO: f32 = 0.6;
// Metrics at 100% DPI; multiplied by the DPI scale.
const BASE_PAD_X: f32 = 8.0;
const BASE_PAD_Y: f32 = 4.0;
const BASE_GAP: f32 = 4.0;
const BASE_RADIUS: f32 = 4.0;
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

/// Memory DC the DIB is selected into. Restores the stock bitmap before deleting the DC.
struct MemDc {
    dc: HDC,
    stock: HGDIOBJ,
}

impl MemDc {
    fn new() -> windows::core::Result<MemDc> {
        // SAFETY: plain FFI; the DC is deleted in Drop.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return Err(windows::core::Error::from_thread());
            }
            Ok(MemDc {
                dc,
                stock: GetCurrentObject(dc, OBJ_BITMAP),
            })
        }
    }
}

impl Drop for MemDc {
    fn drop(&mut self) {
        // SAFETY: the DC was created by CreateCompatibleDC and is deleted exactly once.
        unsafe {
            SelectObject(self.dc, self.stock);
            let _ = DeleteDC(self.dc);
        }
    }
}

/// A 32-bit top-down DIB section. Must not be selected into a DC when dropped.
struct Dib {
    bitmap: HBITMAP,
    size: Size,
}

impl Dib {
    fn new(dc: HDC, size: Size) -> windows::core::Result<Dib> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size.w,
                biHeight: -size.h, // negative: top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        // SAFETY: `info` is a valid BITMAPINFO and `bits` a valid out-pointer.
        let bitmap =
            unsafe { CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }?;
        Ok(Dib { bitmap, size })
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY: the bitmap is owned by this value and deselected from its DC beforehand.
        unsafe {
            let _ = DeleteObject(self.bitmap.into());
        }
    }
}

/// Text resources that only depend on the text size and DPI scale.
struct Style {
    key: (u16, f32),
    number_format: IDWriteTextFormat,
    label_layout: IDWriteTextLayout,
    label_width: f32,
    label_height: f32,
    label_baseline: f32,
    min_number_width: f32,
}

/// Everything needed to paint one frame.
struct Frame {
    size: Size,
    number: IDWriteTextLayout,
    number_pos: Vector2,
    label_pos: Vector2,
    radius: f32,
}

pub struct Overlay {
    // Drop order matters: render target before the DC/DIB it was bound to; the DC before the DIB
    // (so the DIB is deselected first).
    window: OwnedWindow,
    target: ID2D1DCRenderTarget,
    dwrite: IDWriteFactory,
    _d2d: ID2D1Factory,
    dc: MemDc,
    dib: Option<Dib>,
    style: Option<Style>,
    settings: OverlaySettings,
    text: String,
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
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            // dpi 96 makes one DIP equal one pixel, so sizes below are plain pixels.
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: windows::Win32::Graphics::Direct2D::Common::D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let target = d2d.CreateDCRenderTarget(&props)?;
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

            Ok(Overlay {
                window,
                target,
                dwrite,
                _d2d: d2d,
                dc: MemDc::new()?,
                dib: None,
                style: None,
                settings: settings.clone(),
                text: INITIAL_TEXT.to_owned(),
                scale: 1.0,
                size: Size { w: 1, h: 1 },
                pos: None,
                visible: false,
                dirty: true,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.window.0
    }

    pub fn set_settings(&mut self, s: &OverlaySettings) {
        if *s != self.settings {
            self.settings = s.clone();
            self.dirty = true;
        }
    }

    /// Sets the text and re-runs placement. Only redraws when the text, settings or DPI changed.
    pub fn update(&mut self, text: &str) {
        if text != self.text {
            self.text = text.to_owned();
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

    /// Redraws if needed, then places the window in its corner of the foreground monitor.
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
        let origin = overlay_origin(self.settings.corner, area, self.size, margin);
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

    fn style(&mut self) -> windows::core::Result<&Style> {
        let key = (self.settings.text_size, self.scale);
        if self.style.as_ref().map(|s| s.key) != Some(key) {
            let number_px = px(f32::from(self.settings.text_size), self.scale);
            let label_px = px(
                (f32::from(self.settings.text_size) * LABEL_SIZE_RATIO).round(),
                self.scale,
            );
            let number_format = self.format(DWRITE_FONT_WEIGHT_BOLD, number_px)?;
            let label_format = self.format(DWRITE_FONT_WEIGHT_NORMAL, label_px)?;
            let label_layout = self.layout(LABEL_TEXT, &label_format)?;
            let (label_width, label_height, label_baseline) = measure(&label_layout)?;
            let min_layout = self.layout(MIN_NUMBER_TEXT, &number_format)?;
            let (min_number_width, ..) = measure(&min_layout)?;
            self.style = Some(Style {
                key,
                number_format,
                label_layout,
                label_width,
                label_height,
                label_baseline,
                min_number_width,
            });
        }
        Ok(self.style.as_ref().expect("style was just built"))
    }

    fn format(
        &self,
        weight: DWRITE_FONT_WEIGHT,
        size_px: f32,
    ) -> windows::core::Result<IDWriteTextFormat> {
        // SAFETY: plain COM call. A missing font falls back to a system default inside DirectWrite.
        unsafe {
            self.dwrite.CreateTextFormat(
                FONT_FAMILY,
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size_px,
                w!(""),
            )
        }
    }

    fn layout(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
    ) -> windows::core::Result<IDWriteTextLayout> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: plain COM calls with valid arguments.
        unsafe {
            let layout = self
                .dwrite
                .CreateTextLayout(&wide, format, 4096.0, 4096.0)?;
            layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            Ok(layout)
        }
    }

    fn build_frame(&mut self) -> windows::core::Result<Frame> {
        let scale = self.scale;
        let text = self.text.clone();
        let (number_format, label_width, label_height, label_baseline, min_number_width) = {
            let s = self.style()?;
            (
                s.number_format.clone(),
                s.label_width,
                s.label_height,
                s.label_baseline,
                s.min_number_width,
            )
        };
        let number = self.layout(&text, &number_format)?;
        let (number_width, number_height, number_baseline) = measure(&number)?;

        let pad_x = px(BASE_PAD_X, scale);
        let pad_y = px(BASE_PAD_Y, scale);
        let gap = px(BASE_GAP, scale);
        let number_box = number_width.max(min_number_width).ceil();
        // The label sits on the number's baseline.
        let label_dy = number_baseline - label_baseline;
        let content_h = number_height.max(label_dy + label_height);

        Ok(Frame {
            size: Size {
                w: (pad_x * 2.0 + number_box + gap + label_width.ceil()).ceil() as i32,
                h: (pad_y * 2.0 + content_h).ceil() as i32,
            },
            number,
            number_pos: Vector2 { X: pad_x, Y: pad_y },
            label_pos: Vector2 {
                X: pad_x + number_box + gap,
                Y: pad_y + label_dy,
            },
            radius: px(BASE_RADIUS, scale),
        })
    }

    /// Paints the current text into the DIB and returns its size.
    fn draw(&mut self) -> windows::core::Result<Size> {
        let frame = self.build_frame()?;
        let size = Size {
            w: frame.size.w.max(1),
            h: frame.size.h.max(1),
        };

        if self.dib.as_ref().map(|d| d.size) != Some(size) {
            let dib = Dib::new(self.dc.dc, size)?;
            // SAFETY: the new bitmap is valid; the previous DIB is deselected by this call
            // and only then dropped (by the assignment below).
            unsafe {
                SelectObject(self.dc.dc, dib.bitmap.into());
            }
            self.dib = Some(dib);
        }

        let (nc, lc, bg) = (
            color(self.settings.number_color, 100),
            color(self.settings.label_color, 100),
            color(
                self.settings.background_color,
                self.settings.background_opacity,
            ),
        );
        let style = self.style.as_ref().expect("style built by build_frame");
        let bounds = RECT {
            left: 0,
            top: 0,
            right: size.w,
            bottom: size.h,
        };
        // SAFETY: plain COM calls. BeginDraw is always paired with EndDraw (no early return
        // between them); brushes are created first so their errors can't leave a draw open.
        unsafe {
            let number_brush = self.target.CreateSolidColorBrush(&nc, None)?;
            let label_brush = self.target.CreateSolidColorBrush(&lc, None)?;
            let bg_brush = self.target.CreateSolidColorBrush(&bg, None)?;
            self.target.BindDC(self.dc.dc, &bounds)?;
            self.target.BeginDraw();
            self.target.Clear(Some(&D2D1_COLOR_F::default()));
            self.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: 0.0,
                        top: 0.0,
                        right: size.w as f32,
                        bottom: size.h as f32,
                    },
                    radiusX: frame.radius,
                    radiusY: frame.radius,
                },
                &bg_brush,
            );
            self.target.DrawTextLayout(
                frame.number_pos,
                &frame.number,
                &number_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
            self.target.DrawTextLayout(
                frame.label_pos,
                &style.label_layout,
                &label_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
            self.target.EndDraw(None, None)?;
        }
        Ok(size)
    }

    /// Pushes the DIB to the layered window at `origin`.
    fn push(&self, origin: (i32, i32)) -> windows::core::Result<()> {
        let size = self.dib.as_ref().map(|d| d.size).unwrap_or(self.size);
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
        // SAFETY: all pointers refer to live locals; the DC holds the DIB selected in `draw`.
        unsafe {
            UpdateLayeredWindow(
                self.window.0,
                None,
                Some(&dst),
                Some(&sz),
                Some(self.dc.dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }
}

/// Straight-alpha Direct2D color; D2D premultiplies when blending.
fn color(rgb: Rgb, opacity_pct: u8) -> D2D1_COLOR_F {
    let [r, g, b, a] = rgba_f32(rgb, opacity_pct);
    D2D1_COLOR_F { r, g, b, a }
}

/// (width, height, baseline) of a single-line layout.
fn measure(layout: &IDWriteTextLayout) -> windows::core::Result<(f32, f32, f32)> {
    let mut metrics = DWRITE_TEXT_METRICS::default();
    let mut lines = [DWRITE_LINE_METRICS::default(); 1];
    let mut count = 0u32;
    // SAFETY: plain COM calls with valid out-parameters; `lines` has room for the single line.
    unsafe {
        layout.GetMetrics(&mut metrics)?;
        layout.GetLineMetrics(Some(&mut lines), &mut count)?;
    }
    Ok((metrics.width, metrics.height, lines[0].baseline))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lite_bar_core::settings::{Corner, Settings};
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, GetWindowRect, IsWindow, IsWindowVisible, GWL_EXSTYLE, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    };

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
        o.update("144");
        o.update("—");
        let mut s = Settings::default().overlay;
        s.corner = Corner::BottomRight;
        s.background_opacity = 0;
        o.set_settings(&s);
        o.update("60");
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
        o.update("99");
        let two = window_size(&o);
        o.update("100");
        let three = window_size(&o);
        assert!(two.0 > 0 && two.1 > 0, "{two:?}");
        assert_eq!(two, three);
    }

    #[test]
    fn larger_text_size_makes_a_larger_window() {
        let mut s = Settings::default().overlay;
        let mut o = Overlay::new(&s).unwrap();
        o.update("144");
        let small = window_size(&o);
        s.text_size = 36;
        o.set_settings(&s);
        o.update("144");
        let big = window_size(&o);
        assert!(big.0 > small.0 && big.1 > small.1, "{small:?} -> {big:?}");
    }

    #[test]
    fn creating_and_dropping_several_overlays_in_sequence_works() {
        let s = Settings::default().overlay;
        let mut hwnds = Vec::new();
        for i in 0..5 {
            let mut o = Overlay::new(&s).unwrap();
            o.show();
            o.update(&format!("{}", 100 + i));
            hwnds.push(o.hwnd());
            drop(o);
        }
        for h in hwnds {
            assert!(!unsafe { IsWindow(Some(h)) }.as_bool(), "window leaked");
        }
    }
}
