//! Direct2D software host for the core immediate-mode UI toolkit.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::{Rc, Weak};
use std::sync::Once;
use std::time::Instant;

use lite_bar_core::layout::Size;
use lite_bar_core::ui::theme::{self, CARET_BLINK_MS};
use lite_bar_core::ui::{
    Clipboard, Cmd, CursorKind, Input, KeyPress, Pt, Rect, Rgba, TextMeasure, TextStyle, Ui, UiKey,
    UiState,
};
use windows::core::{w, Interface, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
    D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory, ID2D1RenderTarget,
    ID2D1SolidColorBrush, ID2D1StrokeStyle, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_CAP_STYLE_FLAT,
    D2D1_DASH_STYLE_CUSTOM, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_LINE_JOIN_MITER, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_STROKE_STYLE_PROPERTIES,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
#[cfg(test)]
use windows::Win32::Graphics::Gdi::GetPixel;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, EndPaint, InvalidateRect, SelectObject, HDC, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
    VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW, IsIconic,
    KillTimer, LoadCursorW, PostMessageW, RegisterClassExW, SetCursor, SetForegroundWindow,
    SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, CW_USEDEFAULT, GWLP_USERDATA, HCURSOR,
    HTCLIENT, IDC_ARROW, IDC_HAND, IDC_IBEAM, MINMAXINFO, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
    SW_RESTORE, SW_SHOW, WINDOW_EX_STYLE, WM_CANCELMODE, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE,
    WM_DPICHANGED, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WM_SIZE,
    WM_SYSKEYDOWN, WM_TIMER, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use windows_numerics::Vector2;

use crate::log;
use crate::overlay::{Dib, MemDc};

/// DIP to pixel scale for a monitor DPI.
fn scale_for_dpi(dpi: u32) -> f32 {
    dpi as f32 / 96.0
}

/// `round(dip × scale)` in pixels.
fn dip_to_px(dip: f32, scale: f32) -> i32 {
    (dip * scale).round() as i32
}

/// Pixels to DIPs.
fn px_to_dip(px: i32, scale: f32) -> f32 {
    px as f32 / scale
}

/// Signed client coordinates packed in a mouse message's `lParam`.
fn lparam_point(lparam: isize) -> (i32, i32) {
    let x = (lparam & 0xFFFF) as u16 as i16;
    let y = ((lparam >> 16) & 0xFFFF) as u16 as i16;
    (i32::from(x), i32::from(y))
}

/// `GET_WHEEL_DELTA_WPARAM / WHEEL_DELTA`: positive scrolls up.
fn wheel_notches(wparam: usize) -> f32 {
    f32::from(((wparam >> 16) & 0xFFFF) as u16 as i16) / 120.0
}

/// Maps a virtual-key code to the toolkit's key.
fn vk_to_key(vk: u16) -> UiKey {
    match vk {
        0x09 => UiKey::Tab,
        0x0D => UiKey::Enter,
        0x1B => UiKey::Esc,
        0x08 => UiKey::Backspace,
        0x2E => UiKey::Delete,
        0x25 => UiKey::Left,
        0x27 => UiKey::Right,
        0x24 => UiKey::Home,
        0x23 => UiKey::End,
        0x41 => UiKey::A,
        0x43 => UiKey::C,
        0x56 => UiKey::V,
        other => UiKey::Other(other),
    }
}

/// Premultiplied RGBA to premultiplied BGRA, for `DXGI_FORMAT_B8G8R8A8_UNORM`.
fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for px in out.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    out
}

/// NUL-terminated UTF-16, for `CF_UNICODETEXT`.
fn to_utf16z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Text up to the first NUL (or the whole slice); invalid UTF-16 is replaced.
fn from_utf16z(w: &[u16]) -> String {
    let end = w.iter().position(|&u| u == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// One Win32 input message, already converted to DIPs.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Event {
    Move(Pt),
    Leave,
    Down(Pt),
    Up(Pt),
    RightUp(Pt),
    Wheel(f32),
    Key(KeyPress),
    /// One UTF-16 unit from `WM_CHAR`.
    Char(u16),
    /// Capture was taken away (`WM_CAPTURECHANGED`, `WM_CANCELMODE`).
    CaptureLost,
}

/// Input state that persists between messages: pointer, button and a pending high surrogate.
#[derive(Default)]
struct InputState {
    pointer: Option<Pt>,
    down: bool,
    high_surrogate: Option<u16>,
}

impl InputState {
    /// The `Input` for one message, or `None` when the message produces no frame.
    fn apply(&mut self, ev: Event, now_ms: u64) -> Option<Input> {
        let mut input = Input::default();
        match ev {
            Event::Move(p) => self.pointer = Some(p),
            Event::Leave => {
                // While captured the pointer is still tracked outside the window.
                if !self.down {
                    self.pointer = None;
                }
            }
            Event::Down(p) => {
                self.pointer = Some(p);
                input.pressed = !self.down;
                self.down = true;
            }
            Event::Up(p) => {
                self.pointer = Some(p);
                input.released = self.down;
                self.down = false;
            }
            Event::RightUp(p) => {
                self.pointer = Some(p);
                input.right_released = true;
            }
            Event::Wheel(n) => input.wheel = n,
            Event::Key(k) => input.keys.push(k),
            Event::Char(u) => input.text.push(self.decode(u)?),
            Event::CaptureLost => {
                if !self.down {
                    return None;
                }
                self.down = false;
                input.released = true;
            }
        }
        input.pointer = self.pointer;
        input.down = self.down;
        input.now_ms = now_ms;
        Some(input)
    }

    /// Combines surrogate pairs and drops control characters.
    fn decode(&mut self, u: u16) -> Option<char> {
        let c = match u {
            0xD800..=0xDBFF => {
                self.high_surrogate = Some(u);
                return None;
            }
            0xDC00..=0xDFFF => {
                let high = self.high_surrogate.take()?;
                char::decode_utf16([high, u]).next()?.ok()?
            }
            _ => {
                self.high_surrogate = None;
                char::from_u32(u32::from(u))?
            }
        };
        (c >= ' ' && c != '\u{7F}').then_some(c)
    }

    /// An `Input` with no events, for frames run by paint and timers.
    fn idle(&self, now_ms: u64) -> Input {
        Input {
            pointer: self.pointer,
            down: self.down,
            now_ms,
            ..Input::default()
        }
    }
}

/// A page shown in a `UiWindow`. `frame` builds the UI for one frame; `closed` runs once
/// when the window closes (X, Alt+F4, `UiWindow::close`, drop, or a panic in `frame`).
pub trait UiApp {
    fn frame(&mut self, ui: &mut Ui);
    fn closed(&mut self);
}

const CLASS_NAME: PCWSTR = w!("LiteBarUiWindow");
/// `WM_MOUSELEAVE` lives in `Win32_UI_Controls`, which this crate does not otherwise need.
const WM_MOUSELEAVE: u32 = 0x02A3;
/// `CF_UNICODETEXT` lives in `Win32_System_Ole`.
const CF_UNICODETEXT: u32 = 13;
const BLINK_TIMER: usize = 1;
const REDRAW_TIMER: usize = 2;
/// The window never gets smaller than this client size (or the size it opened at, if smaller).
const MIN_CLIENT_DIP: (f32, f32) = (480.0, 320.0);
/// 1 DIP dashes: 4 on, 3 off, in stroke widths.
const DASHES: [f32; 2] = [4.0, 3.0];

/// Straight-alpha Direct2D color; D2D premultiplies when blending.
fn d2d_color(c: Rgba) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(c.0) / 255.0,
        g: f32::from(c.1) / 255.0,
        b: f32::from(c.2) / 255.0,
        a: f32::from(c.3) / 255.0,
    }
}

/// A DIP rect in pixels, edges rounded so fills and clips are crisp.
fn px_rect(r: Rect, scale: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: (r.x * scale).round(),
        top: (r.y * scale).round(),
        right: (r.right() * scale).round(),
        bottom: (r.bottom() * scale).round(),
    }
}

/// Insets a pixel rect by `d` on every side (never inverting it).
fn inset(r: D2D_RECT_F, d: f32) -> D2D_RECT_F {
    let dx = d.min((r.right - r.left) / 2.0).max(0.0);
    let dy = d.min((r.bottom - r.top) / 2.0).max(0.0);
    D2D_RECT_F {
        left: r.left + dx,
        top: r.top + dy,
        right: r.right - dx,
        bottom: r.bottom - dy,
    }
}

fn rounded(rect: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect,
        radiusX: radius.max(0.0),
        radiusY: radius.max(0.0),
    }
}

/// DirectWrite text formats, cached per pixel size, weight and family.
struct Fonts {
    dwrite: IDWriteFactory,
    formats: RefCell<HashMap<(u32, bool, bool), IDWriteTextFormat>>,
}

impl Fonts {
    fn new() -> windows::core::Result<Fonts> {
        // SAFETY: plain COM call.
        let dwrite = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }?;
        Ok(Fonts {
            dwrite,
            formats: RefCell::new(HashMap::new()),
        })
    }

    fn format(&self, style: TextStyle, scale: f32) -> windows::core::Result<IDWriteTextFormat> {
        let size_px = style.size * scale;
        let key = (size_px.to_bits(), style.bold, style.mono);
        if let Some(f) = self.formats.borrow().get(&key) {
            return Ok(f.clone());
        }
        let family = if style.mono {
            w!("Consolas")
        } else {
            w!("Segoe UI")
        };
        let weight = if style.bold {
            DWRITE_FONT_WEIGHT_BOLD
        } else {
            DWRITE_FONT_WEIGHT_NORMAL
        };
        // SAFETY: plain COM call. A missing font falls back to a system default inside DirectWrite.
        let format = unsafe {
            self.dwrite.CreateTextFormat(
                family,
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size_px,
                w!(""),
            )
        }?;
        // SAFETY: plain COM call on the new format.
        unsafe { format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP) }?;
        self.formats.borrow_mut().insert(key, format.clone());
        Ok(format)
    }

    /// A single-line layout in pixels at `scale`.
    fn layout(
        &self,
        text: &str,
        style: TextStyle,
        scale: f32,
    ) -> windows::core::Result<IDWriteTextLayout> {
        let format = self.format(style, scale)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: plain COM call with valid arguments.
        unsafe { self.dwrite.CreateTextLayout(&wide, &format, 1.0e6, 1.0e6) }
    }

    /// (width, height) in DIPs, measured at the pixel size it will be painted at.
    fn measure(
        &self,
        text: &str,
        style: TextStyle,
        scale: f32,
    ) -> windows::core::Result<(f32, f32)> {
        let layout = self.layout(text, style, scale)?;
        let mut m = DWRITE_TEXT_METRICS::default();
        // SAFETY: valid out-parameter.
        unsafe { layout.GetMetrics(&mut m) }?;
        Ok((m.widthIncludingTrailingWhitespace / scale, m.height / scale))
    }
}

/// An uploaded `Cmd::Image`, kept while its source `Rc` is alive.
struct CachedImage {
    source: Weak<Vec<u8>>,
    size: (u32, u32),
    bitmap: ID2D1Bitmap,
}

/// Paints `Cmd` lists onto any Direct2D render target (the settings window's or the
/// overlay's). Target-bound resources are rebuilt when the target changes.
pub struct Painter {
    fonts: Fonts,
    /// Keyed by `Rc::as_ptr`; the `Weak` keeps the address from being reused.
    images: HashMap<usize, CachedImage>,
    dashed: Option<ID2D1StrokeStyle>,
    /// `as_raw` of the target the cached resources belong to.
    target: usize,
}

impl Painter {
    pub fn new() -> windows::core::Result<Painter> {
        Ok(Painter {
            fonts: Fonts::new()?,
            images: HashMap::new(),
            dashed: None,
            target: 0,
        })
    }

    /// Measures text in DIPs as it will be painted at `scale`.
    pub fn measurer(&self, scale: f32) -> Measurer<'_> {
        Measurer {
            fonts: &self.fonts,
            scale,
        }
    }

    /// Paints `cmds` (in DIPs) at `scale` pixels per DIP; the target is in pixels (96 DPI).
    /// The caller brackets this with `BeginDraw`/`EndDraw`. Unbalanced clips are balanced.
    pub fn paint(
        &mut self,
        target: &ID2D1RenderTarget,
        cmds: &[Cmd],
        scale: f32,
    ) -> windows::core::Result<()> {
        let key = target.as_raw() as usize;
        if key != self.target {
            self.images.clear();
            self.dashed = None;
            self.target = key;
        }
        self.images.retain(|_, c| c.source.strong_count() > 0);
        // SAFETY: plain COM call.
        let brush = unsafe { target.CreateSolidColorBrush(&d2d_color(Rgba(0, 0, 0, 0)), None) }?;
        let mut depth = 0usize;
        let mut result = Ok(());
        for cmd in cmds {
            if let Err(e) = self.paint_one(target, &brush, cmd, scale, &mut depth) {
                result = Err(e);
                break;
            }
        }
        for _ in 0..depth {
            // SAFETY: pops only clips this call pushed.
            unsafe { target.PopAxisAlignedClip() };
        }
        result
    }

    fn paint_one(
        &mut self,
        target: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        cmd: &Cmd,
        scale: f32,
        depth: &mut usize,
    ) -> windows::core::Result<()> {
        let set = |c: Rgba| {
            // SAFETY: plain COM call.
            unsafe { brush.SetColor(&d2d_color(c)) }
        };
        // SAFETY (whole match): plain COM calls with pointers to live locals.
        match cmd {
            Cmd::Fill {
                rect,
                color,
                radius,
            } => {
                set(*color);
                let r = px_rect(*rect, scale);
                if *radius > 0.0 {
                    unsafe { target.FillRoundedRectangle(&rounded(r, radius * scale), brush) };
                } else {
                    unsafe { target.FillRectangle(&r, brush) };
                }
            }
            Cmd::Stroke {
                rect,
                color,
                radius,
                width,
            } => {
                set(*color);
                let w = (width * scale).round().max(1.0);
                // D2D strokes straddle the geometry; inset so the stroke stays inside `rect`.
                let r = inset(px_rect(*rect, scale), w / 2.0);
                unsafe {
                    target.DrawRoundedRectangle(
                        &rounded(r, radius * scale - w / 2.0),
                        brush,
                        w,
                        None,
                    )
                };
            }
            Cmd::Dashed {
                rect,
                color,
                radius,
            } => {
                set(*color);
                let style = self.dashed_style(target)?;
                let w = scale.round().max(1.0);
                let r = inset(px_rect(*rect, scale), w / 2.0);
                unsafe {
                    target.DrawRoundedRectangle(
                        &rounded(r, radius * scale - w / 2.0),
                        brush,
                        w,
                        &style,
                    )
                };
            }
            Cmd::Line {
                from,
                to,
                color,
                width,
            } => {
                set(*color);
                let p = |p: Pt| Vector2 {
                    X: p.x * scale,
                    Y: p.y * scale,
                };
                unsafe { target.DrawLine(p(*from), p(*to), brush, width * scale, None) };
            }
            Cmd::Text {
                pos,
                text,
                style,
                color,
            } => {
                set(*color);
                let layout = self.fonts.layout(text, *style, scale)?;
                let origin = Vector2 {
                    X: (pos.x * scale).round(),
                    Y: (pos.y * scale).round(),
                };
                unsafe {
                    target.DrawTextLayout(origin, &layout, brush, D2D1_DRAW_TEXT_OPTIONS_NONE)
                };
            }
            Cmd::Image {
                rect,
                width_px,
                height_px,
                rgba,
            } => {
                if let Some(bitmap) = self.bitmap(target, rgba, *width_px, *height_px)? {
                    let r = px_rect(*rect, scale);
                    unsafe {
                        target.DrawBitmap(
                            &bitmap,
                            Some(&r),
                            1.0,
                            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                            None,
                        )
                    };
                }
            }
            Cmd::ClipPush(rect) => {
                // Intersects with the current clip.
                unsafe {
                    target.PushAxisAlignedClip(&px_rect(*rect, scale), D2D1_ANTIALIAS_MODE_ALIASED)
                };
                *depth += 1;
            }
            Cmd::ClipPop => {
                if *depth > 0 {
                    unsafe { target.PopAxisAlignedClip() };
                    *depth -= 1;
                }
            }
        }
        Ok(())
    }

    fn dashed_style(
        &mut self,
        target: &ID2D1RenderTarget,
    ) -> windows::core::Result<ID2D1StrokeStyle> {
        if let Some(s) = &self.dashed {
            return Ok(s.clone());
        }
        let props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_FLAT,
            endCap: D2D1_CAP_STYLE_FLAT,
            dashCap: D2D1_CAP_STYLE_FLAT,
            lineJoin: D2D1_LINE_JOIN_MITER,
            miterLimit: 10.0,
            dashStyle: D2D1_DASH_STYLE_CUSTOM,
            dashOffset: 0.0,
        };
        // SAFETY: plain COM calls; the stroke style comes from the target's own factory.
        let style = unsafe {
            target
                .GetFactory()?
                .CreateStrokeStyle(&props, Some(&DASHES))
        }?;
        self.dashed = Some(style.clone());
        Ok(style)
    }

    /// The uploaded bitmap for `rgba`, or `None` when the buffer does not match its size.
    fn bitmap(
        &mut self,
        target: &ID2D1RenderTarget,
        rgba: &Rc<Vec<u8>>,
        w: u32,
        h: u32,
    ) -> windows::core::Result<Option<ID2D1Bitmap>> {
        if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 {
            return Ok(None);
        }
        let key = Rc::as_ptr(rgba) as usize;
        if let Some(c) = self.images.get(&key) {
            if c.size == (w, h) {
                return Ok(Some(c.bitmap.clone()));
            }
        }
        let bgra = rgba_to_bgra(rgba);
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        // SAFETY: `bgra` holds h rows of w*4 bytes and outlives the call (D2D copies it).
        let bitmap = unsafe {
            target.CreateBitmap(
                D2D_SIZE_U {
                    width: w,
                    height: h,
                },
                Some(bgra.as_ptr().cast()),
                w * 4,
                &props,
            )
        }?;
        self.images.insert(
            key,
            CachedImage {
                source: Rc::downgrade(rgba),
                size: (w, h),
                bitmap: bitmap.clone(),
            },
        );
        Ok(Some(bitmap))
    }

    #[cfg(test)]
    fn cached_images(&self) -> usize {
        self.images.len()
    }
}

/// `TextMeasure` for one frame at one scale.
pub struct Measurer<'a> {
    fonts: &'a Fonts,
    scale: f32,
}

impl TextMeasure for Measurer<'_> {
    fn measure(&self, text: &str, style: TextStyle) -> (f32, f32) {
        self.fonts
            .measure(text, style, self.scale)
            .unwrap_or((0.0, style.size))
    }
}

/// A software DC render target drawing into a memory DIB, reallocated only on resize.
struct Canvas {
    // Drop order: the target before the DC/DIB it is bound to; the DC before the DIB.
    target: ID2D1DCRenderTarget,
    dc: MemDc,
    dib: Option<Dib>,
}

impl Canvas {
    fn new(d2d: &ID2D1Factory) -> windows::core::Result<Canvas> {
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                // The window is opaque, which also allows ClearType text.
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            // One DIP is one pixel; the painter applies the DPI scale.
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        // SAFETY: plain COM call.
        let target = unsafe { d2d.CreateDCRenderTarget(&props) }?;
        Ok(Canvas {
            target,
            dc: MemDc::new()?,
            dib: None,
        })
    }

    /// Runs `f` between `BeginDraw` and `EndDraw` on a `size` pixel buffer.
    fn draw(
        &mut self,
        size: Size,
        f: impl FnOnce(&ID2D1RenderTarget) -> windows::core::Result<()>,
    ) -> windows::core::Result<()> {
        if self.dib.as_ref().map(|d| d.size) != Some(size) {
            let dib = Dib::new(self.dc.dc, size)?;
            // SAFETY: the new bitmap is valid; the previous DIB is deselected by this call
            // and only then dropped (by the assignment below).
            unsafe { SelectObject(self.dc.dc, dib.bitmap.into()) };
            self.dib = Some(dib);
        }
        let bounds = RECT {
            left: 0,
            top: 0,
            right: size.w,
            bottom: size.h,
        };
        // SAFETY: BeginDraw is always paired with EndDraw.
        unsafe {
            self.target.BindDC(self.dc.dc, &bounds)?;
            self.target.BeginDraw();
            let painted = f(&self.target);
            let ended = self.target.EndDraw(None, None);
            painted.and(ended)
        }
    }

    #[cfg(test)]
    fn pixel(&self, x: i32, y: i32) -> u32 {
        // SAFETY: plain GDI call on our memory DC.
        unsafe { GetPixel(self.dc.dc, x, y) }.0
    }
}

/// `Clipboard` over the Win32 clipboard, as `CF_UNICODETEXT`.
struct WinClipboard {
    hwnd: HWND,
}

impl Clipboard for WinClipboard {
    fn get(&mut self) -> Option<String> {
        // SAFETY: the clipboard is closed on every path after a successful open; the locked
        // memory is read only up to its allocated size.
        unsafe {
            OpenClipboard(Some(self.hwnd)).ok()?;
            let text = (|| {
                let handle = GetClipboardData(CF_UNICODETEXT).ok()?;
                let mem = HGLOBAL(handle.0);
                let ptr = GlobalLock(mem) as *const u16;
                if ptr.is_null() {
                    return None;
                }
                let units = GlobalSize(mem) / 2;
                let text = from_utf16z(std::slice::from_raw_parts(ptr, units));
                let _ = GlobalUnlock(mem);
                Some(text)
            })();
            let _ = CloseClipboard();
            text
        }
    }

    fn set(&mut self, s: &str) {
        let wide = to_utf16z(s);
        // SAFETY: the allocation holds `wide`; after SetClipboardData succeeds the system owns
        // it, otherwise it is freed here. The clipboard is closed after a successful open.
        unsafe {
            if OpenClipboard(Some(self.hwnd)).is_err() {
                return;
            }
            let _ = EmptyClipboard();
            if let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
                let ptr = GlobalLock(mem) as *mut u16;
                if ptr.is_null() {
                    let _ = GlobalFree(Some(mem));
                } else {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                    let _ = GlobalUnlock(mem);
                    if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))).is_err() {
                        let _ = GlobalFree(Some(mem));
                    }
                }
            }
            let _ = CloseClipboard();
        }
    }
}

/// Everything the window owns while open. Dropped (with every D2D/DWrite resource) on close.
struct Host {
    hwnd: HWND,
    app: Box<dyn UiApp>,
    state: UiState,
    input: InputState,
    clipboard: WinClipboard,
    // Drop order: the canvas (render target) before the factory and painter.
    canvas: Canvas,
    painter: Painter,
    _d2d: ID2D1Factory,
    scale: f32,
    opened: Instant,
    /// Commands of the last frame, painted by `WM_PAINT`.
    cmds: Vec<Cmd>,
    cursor: CursorKind,
    blink_on: bool,
    /// Whether `TrackMouseEvent` is armed for the next `WM_MOUSELEAVE`.
    tracking: bool,
}

impl Host {
    fn now_ms(&self) -> u64 {
        self.opened.elapsed().as_millis() as u64
    }

    fn client_px(&self) -> Size {
        let mut r = RECT::default();
        // SAFETY: valid out-parameter for our own window.
        let _ = unsafe { GetClientRect(self.hwnd, &mut r) };
        Size {
            w: (r.right - r.left).max(1),
            h: (r.bottom - r.top).max(1),
        }
    }

    /// Feeds one message's event to the app: one frame per event, then a repaint.
    fn event(&mut self, ev: Event) -> bool {
        let now = self.now_ms();
        let Some(input) = self.input.apply(ev, now) else {
            return true;
        };
        let ok = self.run_frame(&input);
        // SAFETY: plain FFI on our own window.
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
        ok
    }

    /// Runs `UiApp::frame` and keeps its commands. Returns false if the frame panicked.
    fn run_frame(&mut self, input: &Input) -> bool {
        let size = self.client_px();
        let scale = self.scale;
        let area = Rect::new(0.0, 0.0, px_to_dip(size.w, scale), px_to_dip(size.h, scale));
        let Host {
            app,
            state,
            clipboard,
            painter,
            ..
        } = self;
        let measure = painter.measurer(scale);
        let frame = catch_unwind(AssertUnwindSafe(|| {
            let mut ui = Ui::new(&measure, input, state, area);
            ui.scale = scale;
            ui.clipboard = Some(clipboard);
            app.frame(&mut ui);
            ui.finish()
        }));
        let Ok(frame) = frame else {
            log::error("settings window: a frame panicked; closing the window");
            return false;
        };
        self.cmds = frame.cmds;
        self.cursor = frame.cursor;
        // SAFETY (timers and cursor): plain FFI on our own window.
        unsafe {
            if frame.wants_caret_blink != self.blink_on {
                self.blink_on = frame.wants_caret_blink;
                if self.blink_on {
                    SetTimer(Some(self.hwnd), BLINK_TIMER, CARET_BLINK_MS as u32, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), BLINK_TIMER);
                }
            }
            let _ = KillTimer(Some(self.hwnd), REDRAW_TIMER);
            if let Some(at) = frame.redraw_after_ms {
                let delay = at
                    .saturating_sub(input.now_ms)
                    .clamp(1, u64::from(u32::MAX));
                SetTimer(Some(self.hwnd), REDRAW_TIMER, delay as u32, None);
            }
            if self.input.pointer.is_some() {
                // Without waiting for the next WM_SETCURSOR.
                SetCursor(Some(cursor_handle(self.cursor)));
            }
        }
        true
    }

    /// Runs a frame with no input (timers, resize, redraw requests).
    fn idle_frame(&mut self) -> bool {
        let input = self.input.idle(self.now_ms());
        self.run_frame(&input)
    }

    /// Paints the last frame into the DIB and copies it to `hdc`.
    fn paint(&mut self, hdc: HDC) {
        let size = self.client_px();
        let Host {
            canvas,
            painter,
            cmds,
            scale,
            ..
        } = self;
        let drawn = canvas.draw(size, |t| {
            // SAFETY: plain COM call inside BeginDraw/EndDraw.
            unsafe { t.Clear(Some(&d2d_color(theme::BG))) };
            painter.paint(t, cmds, *scale)
        });
        match drawn {
            // SAFETY: both DCs are valid; the memory DC holds a DIB of `size`.
            Ok(()) => unsafe {
                let _ = BitBlt(hdc, 0, 0, size.w, size.h, Some(canvas.dc.dc), 0, 0, SRCCOPY);
            },
            Err(e) => log::warn(&format!("settings window paint failed: {e}")),
        }
    }

    fn kill_timers(&mut self) {
        // SAFETY: plain FFI on our own window.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), BLINK_TIMER);
            let _ = KillTimer(Some(self.hwnd), REDRAW_TIMER);
        }
        self.blink_on = false;
    }
}

fn cursor_handle(kind: CursorKind) -> HCURSOR {
    let name = match kind {
        CursorKind::Arrow => IDC_ARROW,
        CursorKind::Hand => IDC_HAND,
        CursorKind::IBeam => IDC_IBEAM,
    };
    // SAFETY: loads a shared system cursor.
    unsafe { LoadCursorW(None, name) }.unwrap_or_default()
}

/// State shared by `UiWindow` and the window procedure (through `GWLP_USERDATA`).
struct Shared {
    /// `None` once closed. Borrowed only with `try_borrow_mut`, so re-entrant messages
    /// (sent while a handler runs) are skipped instead of double-borrowing.
    host: RefCell<Option<Host>>,
    open: Cell<bool>,
    /// Run a fresh frame on the next paint (resize, DPI change, `request_redraw`, first paint).
    needs_frame: Cell<bool>,
    /// A frame panicked; the window closes when the handler returns.
    failed: Cell<bool>,
    min_client_dip: (f32, f32),
}

impl Shared {
    fn with_host<R>(&self, f: impl FnOnce(&mut Host) -> R) -> Option<R> {
        let mut guard = self.host.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    }

    /// Feeds one input event; marks the window failed if the frame panicked.
    fn event(&self, ev: Event) {
        if self.with_host(|h| h.event(ev)) == Some(false) {
            self.failed.set(true);
        }
    }

    /// Calls `closed` once, drops every resource, then destroys the window.
    fn close(&self, hwnd: HWND) {
        if !self.open.get() {
            return;
        }
        let Ok(mut slot) = self.host.try_borrow_mut() else {
            // A handler is running further up the stack; close once it returns.
            // SAFETY: plain FFI.
            let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
            return;
        };
        let host = slot.take();
        drop(slot);
        self.open.set(false);
        if let Some(mut host) = host {
            host.kill_timers();
            if catch_unwind(AssertUnwindSafe(|| host.app.closed())).is_err() {
                log::error("settings window: closed() panicked");
            }
            drop(host);
        }
        // SAFETY: our own window, destroyed once (open was true).
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

fn register_window_class() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        // SAFETY: plain FFI; the class name is a static NUL-terminated string. A failed
        // registration surfaces as a CreateWindowExW error.
        unsafe {
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(window_proc),
                hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            RegisterClassExW(&class);
        }
    });
}

/// Window size in pixels for a client size in DIPs at `dpi`.
fn window_size_for_client(client_dip: (f32, f32), dpi: u32) -> (i32, i32) {
    let scale = scale_for_dpi(dpi);
    let mut r = RECT {
        left: 0,
        top: 0,
        right: dip_to_px(client_dip.0, scale),
        bottom: dip_to_px(client_dip.1, scale),
    };
    // SAFETY: valid in/out parameter.
    let _ = unsafe {
        AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE(0), dpi)
    };
    (r.right - r.left, r.bottom - r.top)
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: GWLP_USERDATA holds either 0 or a pointer from `Rc::into_raw`, reclaimed on
    // WM_NCDESTROY. Taking a counted reference keeps `Shared` alive for this call even if
    // the `UiWindow` is dropped by a nested message.
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Shared;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        if msg == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Rc::from_raw(ptr));
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        Rc::increment_strong_count(ptr);
        let shared = Rc::from_raw(ptr);
        let result = handle(&shared, hwnd, msg, wparam, lparam);
        if shared.failed.get() {
            shared.close(hwnd);
        }
        result
    }
}

/// Handles one message. Calls that can send messages back to this window (capture,
/// `SetWindowPos`, `DefWindowProc`) run outside the host borrow.
fn handle(shared: &Shared, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let point = |h: &Host| {
        let (x, y) = lparam_point(lparam.0);
        Pt {
            x: px_to_dip(x, h.scale),
            y: px_to_dip(y, h.scale),
        }
    };
    // SAFETY (whole function): plain FFI on our own window with valid arguments.
    let default = || unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            shared.with_host(|h| {
                if shared.needs_frame.replace(false) && !h.idle_frame() {
                    shared.failed.set(true);
                    return;
                }
                h.paint(hdc);
            });
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            if let Some(p) = shared.with_host(|h| point(h)) {
                shared.event(Event::Move(p));
            }
            let arm = shared.with_host(|h| !std::mem::replace(&mut h.tracking, true));
            if arm == Some(true) {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = unsafe { TrackMouseEvent(&mut tme) };
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            shared.with_host(|h| h.tracking = false);
            shared.event(Event::Leave);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            if let Some(p) = shared.with_host(|h| point(h)) {
                shared.event(Event::Down(p));
                unsafe { SetCapture(hwnd) };
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(p) = shared.with_host(|h| point(h)) {
                shared.event(Event::Up(p));
            }
            let _ = unsafe { ReleaseCapture() };
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            if let Some(p) = shared.with_host(|h| point(h)) {
                shared.event(Event::RightUp(p));
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            if lparam.0 != hwnd.0 as isize {
                shared.event(Event::CaptureLost);
            }
            LRESULT(0)
        }
        WM_CANCELMODE => {
            shared.event(Event::CaptureLost);
            let _ = unsafe { ReleaseCapture() };
            default()
        }
        WM_MOUSEWHEEL => {
            shared.event(Event::Wheel(wheel_notches(wparam.0)));
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let down = |vk: VIRTUAL_KEY| unsafe { GetKeyState(i32::from(vk.0)) } < 0;
            shared.event(Event::Key(KeyPress {
                key: vk_to_key(wparam.0 as u16),
                ctrl: down(VK_CONTROL),
                alt: down(VK_MENU),
                shift: down(VK_SHIFT),
                win: down(VK_LWIN) || down(VK_RWIN),
            }));
            if msg == WM_SYSKEYDOWN {
                // Alt+F4 and the system menu.
                default()
            } else {
                LRESULT(0)
            }
        }
        WM_CHAR => {
            shared.event(Event::Char(wparam.0 as u16));
            LRESULT(0)
        }
        WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
            match shared.with_host(|h| h.cursor) {
                Some(kind) => {
                    unsafe { SetCursor(Some(cursor_handle(kind))) };
                    LRESULT(1)
                }
                None => default(),
            }
        }
        WM_TIMER if wparam.0 == BLINK_TIMER || wparam.0 == REDRAW_TIMER => {
            let ran = shared.with_host(|h| {
                if wparam.0 == REDRAW_TIMER {
                    let _ = unsafe { KillTimer(Some(hwnd), REDRAW_TIMER) };
                }
                let ok = h.idle_frame();
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                ok
            });
            if ran == Some(false) {
                shared.failed.set(true);
            }
            LRESULT(0)
        }
        WM_SIZE => {
            shared.needs_frame.set(true);
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = ((wparam.0 >> 16) & 0xFFFF) as u32;
            shared.with_host(|h| h.scale = scale_for_dpi(dpi));
            shared.needs_frame.set(true);
            // SAFETY: lParam points to the suggested window rect for the new DPI.
            let r = unsafe { *(lparam.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let dpi = unsafe { GetDpiForWindow(hwnd) };
            let (w, h) = window_size_for_client(shared.min_client_dip, dpi);
            // SAFETY: lParam points to the system's MINMAXINFO for this message.
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            info.ptMinTrackSize = POINT { x: w, y: h };
            LRESULT(0)
        }
        WM_CLOSE => {
            shared.close(hwnd);
            LRESULT(0)
        }
        _ => default(),
    }
}

/// Destroys a half-built window if `open` fails partway.
struct PendingWindow(HWND);

impl Drop for PendingWindow {
    fn drop(&mut self) {
        // SAFETY: the window was created on this thread and is not yet owned by anything.
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

/// A normal top-level window showing a `UiApp`. Created on open, destroyed on close; the
/// tray app keeps nothing of it afterwards. Must be used on the thread that opened it.
///
/// Frame model: every input message builds its own `Input` and runs one frame right away
/// (so text and key order are preserved and hot tracks every move), keeping the commands
/// and invalidating. `WM_PAINT` paints the kept commands, running a fresh frame first only
/// when one is due without input (first paint, resize, DPI change, `request_redraw`).
/// Timers (caret blink, `redraw_after_ms`) run a frame and invalidate.
pub struct UiWindow {
    hwnd: HWND,
    shared: Rc<Shared>,
}

impl UiWindow {
    pub fn open(
        title: &str,
        size_dip: (f32, f32),
        app: Box<dyn UiApp>,
    ) -> windows::core::Result<UiWindow> {
        UiWindow::create(title, size_dip, app, true)
    }

    fn create(
        title: &str,
        size_dip: (f32, f32),
        app: Box<dyn UiApp>,
        show: bool,
    ) -> windows::core::Result<UiWindow> {
        register_window_class();
        let title = HSTRING::from(title);
        // SAFETY: plain FFI; the window is destroyed by PendingWindow on every error path.
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS_NAME,
                &title,
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(GetModuleHandleW(None)?.into()),
                None,
            )?;
            let pending = PendingWindow(hwnd);
            let dpi = GetDpiForWindow(hwnd);
            let (w, h) = window_size_for_client(size_dip, dpi);
            SetWindowPos(
                hwnd,
                None,
                0,
                0,
                w,
                h,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )?;
            let dark = BOOL(1);
            if let Err(e) = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const BOOL as *const _,
                size_of::<BOOL>() as u32,
            ) {
                log::warn(&format!("settings window: no dark title bar: {e}"));
            }

            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let host = Host {
                hwnd,
                app,
                state: UiState::default(),
                input: InputState::default(),
                clipboard: WinClipboard { hwnd },
                canvas: Canvas::new(&d2d)?,
                painter: Painter::new()?,
                _d2d: d2d,
                scale: scale_for_dpi(dpi),
                opened: Instant::now(),
                cmds: Vec::new(),
                cursor: CursorKind::Arrow,
                blink_on: false,
                tracking: false,
            };
            let shared = Rc::new(Shared {
                host: RefCell::new(Some(host)),
                open: Cell::new(true),
                needs_frame: Cell::new(true),
                failed: Cell::new(false),
                min_client_dip: (
                    MIN_CLIENT_DIP.0.min(size_dip.0),
                    MIN_CLIENT_DIP.1.min(size_dip.1),
                ),
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::into_raw(shared.clone()) as isize);
            std::mem::forget(pending);
            if show {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
            }
            Ok(UiWindow { hwnd, shared })
        }
    }

    pub fn is_open(&self) -> bool {
        self.shared.open.get()
    }

    /// Brings the window to the front (restoring it if minimized).
    pub fn focus(&self) {
        if !self.is_open() {
            return;
        }
        // SAFETY: plain FFI on our own window.
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    /// Runs a fresh frame on the next paint, e.g. after settings changed elsewhere.
    pub fn request_redraw(&self) {
        if !self.is_open() {
            return;
        }
        self.shared.needs_frame.set(true);
        // SAFETY: plain FFI on our own window.
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// Calls `UiApp::closed` (once) and destroys the window. Does nothing if already closed.
    pub fn close(&self) {
        self.shared.close(self.hwnd);
    }

    #[cfg(test)]
    fn hwnd(&self) -> HWND {
        self.hwnd
    }

    #[cfg(test)]
    fn pixel(&self, x: i32, y: i32) -> Option<u32> {
        self.shared.with_host(|h| h.canvas.pixel(x, y))
    }

    #[cfg(test)]
    fn scale(&self) -> f32 {
        self.shared.with_host(|h| h.scale).unwrap_or(0.0)
    }

    #[cfg(test)]
    fn blink_active(&self) -> bool {
        self.shared.with_host(|h| h.blink_on).unwrap_or(false)
    }
}

impl Drop for UiWindow {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f32, y: f32) -> Pt {
        Pt { x, y }
    }

    fn key(k: UiKey) -> KeyPress {
        KeyPress {
            key: k,
            ctrl: true,
            alt: false,
            shift: false,
            win: false,
        }
    }

    #[test]
    fn dip_to_px_uses_monitor_scale() {
        assert_eq!(scale_for_dpi(96), 1.0);
        assert_eq!(scale_for_dpi(144), 1.5);
        assert_eq!(scale_for_dpi(192), 2.0);
        assert_eq!(dip_to_px(100.0, 1.5), 150);
        assert_eq!(dip_to_px(13.0, 1.25), 16); // 16.25 rounds down
        assert_eq!(dip_to_px(10.0, 1.25), 13); // 12.5 rounds up
        assert_eq!(px_to_dip(150, 1.5), 100.0);
    }

    #[test]
    fn lparam_point_is_signed() {
        assert_eq!(lparam_point(0x0014_000A), (10, 20));
        // Captured drags report negative coordinates left of and above the window.
        assert_eq!(lparam_point(0xFFFE_FFFF), (-1, -2));
    }

    #[test]
    fn wheel_delta_is_in_notches() {
        assert_eq!(wheel_notches(120 << 16), 1.0);
        assert_eq!(
            wheel_notches((-240i16 as u16 as usize) << 16 | 0x0008),
            -2.0
        );
        assert_eq!(wheel_notches(60 << 16), 0.5);
    }

    #[test]
    fn vk_maps_to_ui_keys() {
        let cases = [
            (0x09, UiKey::Tab),
            (0x0D, UiKey::Enter),
            (0x1B, UiKey::Esc),
            (0x08, UiKey::Backspace),
            (0x2E, UiKey::Delete),
            (0x25, UiKey::Left),
            (0x27, UiKey::Right),
            (0x24, UiKey::Home),
            (0x23, UiKey::End),
            (0x41, UiKey::A),
            (0x43, UiKey::C),
            (0x56, UiKey::V),
            (0x70, UiKey::Other(0x70)),
            (0x26, UiKey::Other(0x26)),
        ];
        for (vk, want) in cases {
            assert_eq!(vk_to_key(vk), want, "vk {vk:#x}");
        }
    }

    #[test]
    fn image_rgba_is_converted_to_bgra() {
        assert_eq!(
            rgba_to_bgra(&[1, 2, 3, 4, 10, 20, 30, 40]),
            vec![3, 2, 1, 4, 30, 20, 10, 40]
        );
    }

    #[test]
    fn clipboard_text_round_trips_through_utf16z() {
        let w = to_utf16z("a€𝄞");
        assert_eq!(w, vec![0x61, 0x20AC, 0xD834, 0xDD1E, 0]);
        assert_eq!(from_utf16z(&w), "a€𝄞");
        assert_eq!(from_utf16z(&[0x68, 0x69]), "hi");
        assert_eq!(from_utf16z(&[0x68, 0, 0x69]), "h");
    }

    #[test]
    fn surrogate_pairs_combine_into_one_char() {
        let mut s = InputState::default();
        assert_eq!(s.apply(Event::Char(0xD834), 0), None);
        let i = s.apply(Event::Char(0xDD1E), 0).unwrap();
        assert_eq!(i.text, vec!['𝄞']);
        // A lone low surrogate produces nothing.
        assert_eq!(s.apply(Event::Char(0xDD1E), 0), None);
    }

    #[test]
    fn control_chars_are_skipped() {
        let mut s = InputState::default();
        for c in [0x08u16, 0x0D, 0x1B, 0x01, 0x7F] {
            assert_eq!(s.apply(Event::Char(c), 0), None, "{c:#x}");
        }
        assert_eq!(s.apply(Event::Char(0x20), 0).unwrap().text, vec![' ']);
        assert_eq!(s.apply(Event::Char(0xE9), 0).unwrap().text, vec!['é']);
    }

    #[test]
    fn each_event_gets_its_own_input_with_fresh_edges() {
        let mut s = InputState::default();
        let i = s.apply(Event::Move(pt(5.0, 6.0)), 10).unwrap();
        assert_eq!(i.pointer, Some(pt(5.0, 6.0)));
        assert_eq!(i.now_ms, 10);
        assert!(!i.pressed && !i.down);

        let i = s.apply(Event::Down(pt(7.0, 8.0)), 11).unwrap();
        assert!(i.pressed && i.down);
        assert_eq!(i.pointer, Some(pt(7.0, 8.0)));

        let i = s.apply(Event::Key(key(UiKey::A)), 12).unwrap();
        assert!(!i.pressed && i.down, "pressed only on the down message");
        assert_eq!(i.keys, vec![key(UiKey::A)]);
        assert!(i.text.is_empty());

        let i = s.apply(Event::Char(u16::from(b'x')), 13).unwrap();
        assert_eq!(i.text, vec!['x']);
        assert!(i.keys.is_empty());

        let i = s.apply(Event::Up(pt(9.0, 9.0)), 14).unwrap();
        assert!(i.released && !i.down);

        let i = s.apply(Event::Wheel(-1.0), 15).unwrap();
        assert_eq!(i.wheel, -1.0);
        assert!(!i.released);
        assert_eq!(i.pointer, Some(pt(9.0, 9.0)));

        let idle = s.idle(16);
        assert_eq!(idle.pointer, Some(pt(9.0, 9.0)));
        assert_eq!(idle.now_ms, 16);
        assert!(!idle.released && idle.keys.is_empty() && idle.wheel == 0.0);
    }

    #[test]
    fn mouse_leave_clears_pointer() {
        let mut s = InputState::default();
        s.apply(Event::Move(pt(1.0, 1.0)), 0);
        let i = s.apply(Event::Leave, 1).unwrap();
        assert_eq!(i.pointer, None);
    }

    #[test]
    fn capture_lost_while_down_synthesizes_release() {
        let mut s = InputState::default();
        s.apply(Event::Down(pt(1.0, 1.0)), 0);
        let i = s.apply(Event::CaptureLost, 1).unwrap();
        assert!(i.released && !i.down);
        // Losing capture again (button already up) runs no frame.
        assert_eq!(s.apply(Event::CaptureLost, 2), None);
    }

    #[test]
    fn up_without_down_is_not_a_release() {
        let mut s = InputState::default();
        let i = s.apply(Event::Up(pt(1.0, 1.0)), 0).unwrap();
        assert!(!i.released);
    }

    #[test]
    fn right_button_up_sets_right_released() {
        let mut s = InputState::default();
        let i = s.apply(Event::RightUp(pt(3.0, 4.0)), 0).unwrap();
        assert!(i.right_released && !i.released);
        assert_eq!(i.pointer, Some(pt(3.0, 4.0)));
    }
}

#[cfg(test)]
mod win_tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use lite_bar_core::ui::{id, theme, Rect, Rgba, TextEdit};
    use windows::core::w;
    use windows::Win32::Foundation::{LPARAM, RECT, WPARAM};
    use windows::Win32::Graphics::Direct2D::{
        D2D1CreateFactory, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    };
    use windows::Win32::Graphics::DirectWrite::{
        DWriteCreateFactory, IDWriteFactory, DWRITE_FACTORY_TYPE_SHARED,
        DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
        DWRITE_LINE_METRICS,
    };
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindow, SendMessageW, WM_CHAR, WM_CLOSE, WM_DPICHANGED, WM_LBUTTONDOWN, WM_LBUTTONUP,
        WM_MOUSEMOVE, WM_PAINT,
    };

    use super::*;

    /// COLORREF (0x00BBGGRR) of an opaque color.
    fn colorref(c: Rgba) -> u32 {
        u32::from(c.0) | u32::from(c.1) << 8 | u32::from(c.2) << 16
    }

    const RED: Rgba = Rgba(255, 0, 0, 255);
    const BLUE: Rgba = Rgba(0, 0, 255, 255);

    fn fill(rect: Rect, color: Rgba) -> Cmd {
        Cmd::Fill {
            rect,
            color,
            radius: 0.0,
        }
    }

    fn canvas_paint(painter: &mut Painter, canvas: &mut Canvas, cmds: &[Cmd], scale: f32) {
        canvas
            .draw(Size { w: 20, h: 20 }, |t| painter.paint(t, cmds, scale))
            .unwrap();
    }

    fn new_canvas() -> Canvas {
        let d2d: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }.unwrap();
        Canvas::new(&d2d).unwrap()
    }

    #[test]
    fn measure_matches_directwrite() {
        let p = Painter::new().unwrap();
        let (w, h) = p.measurer(1.0).measure("Hello", TextStyle::BODY);
        let dwrite: IDWriteFactory =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }.unwrap();
        let line_h = unsafe {
            let f = dwrite
                .CreateTextFormat(
                    w!("Segoe UI"),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    13.0,
                    w!(""),
                )
                .unwrap();
            let text: Vec<u16> = "Hello".encode_utf16().collect();
            let l = dwrite.CreateTextLayout(&text, &f, 4096.0, 4096.0).unwrap();
            let mut lines = [DWRITE_LINE_METRICS::default(); 1];
            let mut n = 0;
            l.GetLineMetrics(Some(&mut lines), &mut n).unwrap();
            lines[0].height
        };
        assert!(w > 10.0 && w < 60.0, "width {w}");
        assert!((h - line_h).abs() < 0.5, "height {h} vs line {line_h}");
        // Measured in DIPs at any scale.
        let (w2, h2) = p.measurer(2.0).measure("Hello", TextStyle::BODY);
        assert!(
            (w2 - w).abs() < 1.0 && (h2 - h).abs() < 1.0,
            "{w2}x{h2} vs {w}x{h}"
        );
        // Mono and bold differ from the body font.
        let (wm, _) = p.measurer(1.0).measure(
            "iiii",
            TextStyle {
                mono: true,
                ..TextStyle::BODY
            },
        );
        let (wb, _) = p.measurer(1.0).measure("iiii", TextStyle::BODY);
        assert!(wm > wb, "Consolas is wider for i: {wm} vs {wb}");
    }

    #[test]
    fn painter_fills_and_clips() {
        let mut p = Painter::new().unwrap();
        let mut c = new_canvas();
        let cmds = [
            fill(Rect::new(0.0, 0.0, 20.0, 20.0), RED),
            Cmd::ClipPush(Rect::new(0.0, 0.0, 10.0, 20.0)),
            Cmd::ClipPush(Rect::new(0.0, 0.0, 20.0, 10.0)),
            fill(Rect::new(0.0, 0.0, 20.0, 20.0), BLUE),
            Cmd::ClipPop,
            Cmd::ClipPop,
        ];
        canvas_paint(&mut p, &mut c, &cmds, 1.0);
        assert_eq!(c.pixel(5, 5), colorref(BLUE));
        assert_eq!(c.pixel(15, 5), colorref(RED), "outside the first clip");
        assert_eq!(c.pixel(5, 15), colorref(RED), "clips intersect");
    }

    #[test]
    fn painter_scales_dips_to_pixels() {
        let mut p = Painter::new().unwrap();
        let mut c = new_canvas();
        let cmds = [
            fill(Rect::new(0.0, 0.0, 20.0, 20.0), RED),
            fill(Rect::new(0.0, 0.0, 5.0, 5.0), BLUE),
        ];
        canvas_paint(&mut p, &mut c, &cmds, 2.0);
        assert_eq!(c.pixel(8, 8), colorref(BLUE));
        assert_eq!(c.pixel(11, 11), colorref(RED));
    }

    #[test]
    fn image_bitmaps_are_cached_while_rc_alive() {
        let mut p = Painter::new().unwrap();
        let mut c = new_canvas();
        // 2x2 opaque red, RGBA.
        let rgba = Rc::new([255u8, 0, 0, 255].repeat(4));
        let cmds = [Cmd::Image {
            rect: Rect::new(0.0, 0.0, 20.0, 20.0),
            width_px: 2,
            height_px: 2,
            rgba: rgba.clone(),
        }];
        canvas_paint(&mut p, &mut c, &cmds, 1.0);
        assert_eq!(c.pixel(10, 10), colorref(RED), "RGBA swizzled to BGRA");
        canvas_paint(&mut p, &mut c, &cmds, 1.0);
        assert_eq!(p.cached_images(), 1);
        drop(cmds);
        drop(rgba);
        canvas_paint(&mut p, &mut c, &[], 1.0);
        assert_eq!(p.cached_images(), 0, "stale bitmap dropped");
    }

    #[test]
    fn painter_handles_every_cmd() {
        let mut p = Painter::new().unwrap();
        let mut c = new_canvas();
        let r = Rect::new(2.0, 2.0, 16.0, 16.0);
        let cmds = [
            fill(Rect::new(0.0, 0.0, 20.0, 20.0), theme::BG),
            Cmd::Fill {
                rect: r,
                color: theme::SURFACE,
                radius: 4.0,
            },
            Cmd::Stroke {
                rect: r,
                color: theme::BORDER,
                radius: 4.0,
                width: 1.0,
            },
            Cmd::Dashed {
                rect: r,
                color: theme::MUTED,
                radius: 0.0,
            },
            Cmd::Line {
                from: Pt { x: 0.0, y: 0.0 },
                to: Pt { x: 20.0, y: 20.0 },
                color: theme::ACCENT,
                width: 1.0,
            },
            Cmd::Text {
                pos: Pt { x: 1.0, y: 1.0 },
                text: "Hi 42".into(),
                style: TextStyle::BOLD,
                color: theme::TEXT,
            },
            // A pop without a push and a push without a pop must not break EndDraw.
            Cmd::ClipPop,
            Cmd::ClipPush(r),
        ];
        canvas_paint(&mut p, &mut c, &cmds, 1.5);
    }

    #[test]
    fn stroke_stays_inside_its_rect() {
        let mut p = Painter::new().unwrap();
        let mut c = new_canvas();
        let cmds = [
            fill(Rect::new(0.0, 0.0, 20.0, 20.0), RED),
            Cmd::Stroke {
                rect: Rect::new(2.0, 2.0, 16.0, 16.0),
                color: BLUE,
                radius: 0.0,
                width: 1.0,
            },
        ];
        canvas_paint(&mut p, &mut c, &cmds, 1.0);
        assert_eq!(c.pixel(1, 10), colorref(RED), "nothing bleeds outside");
        assert_eq!(c.pixel(2, 10), colorref(BLUE), "a crisp 1 px edge");
        assert_eq!(c.pixel(3, 10), colorref(RED));
    }

    /// Counts frames and `closed` calls; fills the window with `color`.
    struct TestApp {
        frames: Rc<Cell<u32>>,
        closed: Rc<Cell<u32>>,
        color: Rgba,
    }

    impl UiApp for TestApp {
        fn frame(&mut self, ui: &mut Ui) {
            self.frames.set(self.frames.get() + 1);
            ui.label("x", TextStyle::BODY, self.color);
            ui.button(id("ok"), "OK");
        }
        fn closed(&mut self) {
            self.closed.set(self.closed.get() + 1);
        }
    }

    fn open_test(color: Rgba) -> (UiWindow, Rc<Cell<u32>>, Rc<Cell<u32>>) {
        let frames = Rc::new(Cell::new(0));
        let closed = Rc::new(Cell::new(0));
        let app = TestApp {
            frames: frames.clone(),
            closed: closed.clone(),
            color,
        };
        let w = UiWindow::create("Lite Bar test", (300.0, 200.0), Box::new(app), false).unwrap();
        (w, frames, closed)
    }

    fn send(w: &UiWindow, msg: u32, wparam: usize, lparam: isize) {
        unsafe { SendMessageW(w.hwnd(), msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) };
    }

    fn at(x: i32, y: i32) -> isize {
        ((y as isize) << 16) | (x as isize & 0xFFFF)
    }

    #[test]
    fn window_opens_paints_and_closes() {
        let (w, frames, _) = open_test(RED);
        assert!(w.is_open());
        send(&w, WM_PAINT, 0, 0);
        assert!(frames.get() >= 1);
        // The window is cleared to the theme background.
        assert_eq!(w.pixel(250, 150), Some(colorref(theme::BG)));
        let hwnd = w.hwnd();
        w.close();
        assert!(!w.is_open());
        assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    }

    #[test]
    fn close_calls_closed_once() {
        let (w, _, closed) = open_test(RED);
        let hwnd = w.hwnd();
        send(&w, WM_CLOSE, 0, 0);
        assert_eq!(closed.get(), 1);
        assert!(!w.is_open());
        assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
        w.close();
        drop(w);
        assert_eq!(closed.get(), 1);
    }

    #[test]
    fn dropping_an_open_window_closes_it() {
        let (w, _, closed) = open_test(RED);
        let hwnd = w.hwnd();
        drop(w);
        assert_eq!(closed.get(), 1);
        assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    }

    #[test]
    fn dark_title_bar_attribute_is_set() {
        let (w, _, _) = open_test(RED);
        let mut dark = windows::core::BOOL(0);
        unsafe {
            DwmGetWindowAttribute(
                w.hwnd(),
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &mut dark as *mut _ as *mut _,
                size_of::<windows::core::BOOL>() as u32,
            )
        }
        .unwrap();
        assert!(dark.as_bool());
    }

    #[test]
    fn each_input_message_runs_one_frame_and_paint_reuses_it() {
        let (w, frames, _) = open_test(RED);
        send(&w, WM_PAINT, 0, 0);
        let base = frames.get();
        for i in 0..3 {
            send(&w, WM_MOUSEMOVE, 0, at(10 + i, 10));
        }
        assert_eq!(frames.get(), base + 3);
        send(&w, WM_CHAR, 'a' as usize, 0);
        assert_eq!(frames.get(), base + 4);
        // Painting shows the last input frame without running another.
        send(&w, WM_PAINT, 0, 0);
        assert_eq!(frames.get(), base + 4);
        // A redraw request runs a fresh frame on the next paint.
        w.request_redraw();
        send(&w, WM_PAINT, 0, 0);
        assert_eq!(frames.get(), base + 5);
    }

    struct ButtonApp {
        clicks: Rc<Cell<u32>>,
    }

    impl UiApp for ButtonApp {
        fn frame(&mut self, ui: &mut Ui) {
            if ui.button(id("b"), "Button") {
                self.clicks.set(self.clicks.get() + 1);
            }
        }
        fn closed(&mut self) {}
    }

    #[test]
    fn click_messages_press_a_button() {
        let clicks = Rc::new(Cell::new(0));
        let w = UiWindow::create(
            "t",
            (300.0, 200.0),
            Box::new(ButtonApp {
                clicks: clicks.clone(),
            }),
            false,
        )
        .unwrap();
        let s = w.scale();
        let p = at(dip_to_px(10.0, s), dip_to_px(10.0, s));
        send(&w, WM_MOUSEMOVE, 0, p);
        send(&w, WM_LBUTTONDOWN, 1, p);
        send(&w, WM_LBUTTONUP, 0, p);
        assert_eq!(clicks.get(), 1);
    }

    struct FieldApp {
        edit: Rc<RefCell<TextEdit>>,
    }

    impl UiApp for FieldApp {
        fn frame(&mut self, ui: &mut Ui) {
            ui.text_field(id("f"), &mut self.edit.borrow_mut(), "");
        }
        fn closed(&mut self) {}
    }

    #[test]
    fn typed_chars_reach_a_focused_text_field_and_blink_the_caret() {
        let edit = Rc::new(RefCell::new(TextEdit::default()));
        let w = UiWindow::create(
            "t",
            (300.0, 200.0),
            Box::new(FieldApp { edit: edit.clone() }),
            false,
        )
        .unwrap();
        assert!(!w.blink_active());
        let s = w.scale();
        let p = at(dip_to_px(10.0, s), dip_to_px(10.0, s));
        send(&w, WM_MOUSEMOVE, 0, p);
        send(&w, WM_LBUTTONDOWN, 1, p);
        send(&w, WM_LBUTTONUP, 0, p);
        assert!(w.blink_active(), "focused field blinks its caret");
        send(&w, WM_CHAR, 'h' as usize, 0);
        send(&w, WM_CHAR, 0xD834, 0);
        send(&w, WM_CHAR, 0xDD1E, 0);
        send(&w, WM_CHAR, 'i' as usize, 0);
        assert_eq!(edit.borrow().text, "h𝄞i");
    }

    struct PanicApp {
        closed: Rc<Cell<u32>>,
    }

    impl UiApp for PanicApp {
        fn frame(&mut self, _ui: &mut Ui) {
            panic!("frame bug");
        }
        fn closed(&mut self) {
            self.closed.set(self.closed.get() + 1);
        }
    }

    #[test]
    fn panic_in_frame_closes_the_window() {
        let closed = Rc::new(Cell::new(0));
        let w = UiWindow::create(
            "t",
            (300.0, 200.0),
            Box::new(PanicApp {
                closed: closed.clone(),
            }),
            false,
        )
        .unwrap();
        let hwnd = w.hwnd();
        send(&w, WM_PAINT, 0, 0);
        assert!(!w.is_open());
        assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
        assert_eq!(closed.get(), 1);
    }

    #[test]
    fn dpi_change_rescales() {
        let (w, _, _) = open_test(RED);
        let rect = RECT {
            left: 0,
            top: 0,
            right: 600,
            bottom: 400,
        };
        send(
            &w,
            WM_DPICHANGED,
            168 | (168 << 16),
            &rect as *const RECT as isize,
        );
        assert_eq!(w.scale(), 1.75);
        send(&w, WM_PAINT, 0, 0);
        assert!(w.pixel(1, 1).is_some());
    }
}
