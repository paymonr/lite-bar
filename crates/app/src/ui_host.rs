//! Direct2D software host for the core immediate-mode UI toolkit.

use std::cell::{Cell, RefCell};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;
use std::sync::Once;
use std::time::Instant;

use lite_bar_core::layout::Size;
use lite_bar_core::ui::theme::{self, CARET_BLINK_MS};
use lite_bar_core::ui::{
    Clipboard, Cmd, CursorKind, Input, KeyPress, Pt, Rect, Ui, UiKey, UiState,
};
use windows::core::{w, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, D2D1_FACTORY_TYPE_SINGLE_THREADED,
};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, HDC,
    MONITORINFO, MONITOR_DEFAULTTOPRIMARY, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
    VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetCursorPos, GetWindowLongPtrW,
    IsIconic, KillTimer, LoadCursorW, RegisterClassExW, SetCursor, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_USERDATA, HCURSOR, HTCLIENT, IDC_ARROW,
    IDC_HAND, IDC_IBEAM, MINMAXINFO, SWP_NOACTIVATE, SWP_NOZORDER, SW_RESTORE, SW_SHOW,
    WINDOW_EX_STYLE, WM_CANCELMODE, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_DESTROY,
    WM_DPICHANGED, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WM_SIZE,
    WM_SYSKEYDOWN, WM_TIMER, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};

use crate::log;
use crate::ui_paint::{d2d_color, Canvas, Painter};

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

/// NUL-terminated UTF-16, for `CF_UNICODETEXT`.
fn to_utf16z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Text up to the first NUL (or the whole slice); invalid UTF-16 is replaced.
fn from_utf16z(w: &[u16]) -> String {
    let end = w.iter().position(|&u| u == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

thread_local! {
    /// True while `UiApp::frame` or `UiApp::closed` runs on this thread (a panic there is
    /// caught by the host and must not end the app).
    static IN_UI_FRAME: Cell<bool> = const { Cell::new(false) };
}

/// Whether a `UiApp` callback is running on this thread (read by the panic hook).
pub fn in_ui_frame() -> bool {
    IN_UI_FRAME.get()
}

/// Runs `f` with `in_ui_frame()` true; resets the flag even when `f` unwinds.
fn as_ui_frame<R>(f: impl FnOnce() -> R) -> R {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            IN_UI_FRAME.set(self.0);
        }
    }
    let _reset = Reset(IN_UI_FRAME.replace(true));
    f()
}

/// Top-left corner that centres a `size` window in `work`, kept inside it when too big.
fn centered_in(work: RECT, size: (i32, i32)) -> (i32, i32) {
    let x = work.left + ((work.right - work.left - size.0) / 2).max(0);
    let y = work.top + ((work.bottom - work.top - size.1) / 2).max(0);
    (x, y)
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
    /// Must return promptly and must never pump messages: no `MessageBox`, modal dialogs,
    /// menus or other modal loops. Messages arriving while a frame runs are skipped.
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
                let mem = windows::Win32::Foundation::HGLOBAL(handle.0);
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
            // Fill the memory first, so a failure leaves the clipboard's old content alone.
            let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else {
                return;
            };
            let ptr = GlobalLock(mem) as *mut u16;
            if ptr.is_null() {
                let _ = GlobalFree(Some(mem));
                return;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            let _ = GlobalUnlock(mem);
            if OpenClipboard(Some(self.hwnd)).is_err() {
                let _ = GlobalFree(Some(mem));
                return;
            }
            let _ = EmptyClipboard();
            if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))).is_err() {
                let _ = GlobalFree(Some(mem));
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
        let pointer_event = matches!(ev, Event::Move(_) | Event::Down(_) | Event::Up(_));
        // SAFETY: plain FFI on our own window.
        unsafe {
            if ok && pointer_event && self.input.pointer.is_some() {
                // Show the new cursor without waiting for the next WM_SETCURSOR.
                SetCursor(Some(cursor_handle(self.cursor)));
            }
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
            as_ui_frame(|| {
                let mut ui = Ui::new(&measure, input, state, area);
                ui.scale = scale;
                ui.clipboard = Some(clipboard);
                app.frame(&mut ui);
                ui.finish()
            })
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
                let _ = BitBlt(hdc, 0, 0, size.w, size.h, Some(canvas.dc()), 0, 0, SRCCOPY);
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
    /// `close` was asked for while the host was busy; it runs when the handler returns.
    close_pending: Cell<bool>,
    /// WM_DESTROY was seen: the hwnd is dead and must not be destroyed again.
    destroyed: Cell<bool>,
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

    /// Takes the host out, calls `closed` once and drops every resource. Returns false
    /// (and leaves `close_pending` set) when a handler further up the stack holds the host.
    fn shut_down(&self) -> bool {
        let Ok(mut slot) = self.host.try_borrow_mut() else {
            self.close_pending.set(true);
            return false;
        };
        let host = slot.take();
        drop(slot);
        self.close_pending.set(false);
        self.open.set(false);
        if let Some(mut host) = host {
            host.kill_timers();
            if catch_unwind(AssertUnwindSafe(|| as_ui_frame(|| host.app.closed()))).is_err() {
                log::error("settings window: closed() panicked");
            }
            drop(host);
        }
        true
    }

    /// Shuts down, then destroys the window unless it is already gone. If the host is busy,
    /// this happens when the outermost handler returns (see `window_proc`).
    fn close(&self, hwnd: HWND) {
        if self.shut_down() && !self.destroyed.get() {
            // SAFETY: our own window, still alive (no WM_DESTROY seen yet).
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
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

/// Work area and effective DPI of the monitor under the cursor.
fn cursor_monitor() -> (RECT, u32) {
    // SAFETY: plain FFI with valid, initialised out-parameters.
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let work = if GetMonitorInfoW(monitor, &mut info).as_bool() {
            info.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: 1024,
                bottom: 768,
            }
        };
        let (mut dpi, mut dpi_y) = (0u32, 0u32);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y).is_err() || dpi == 0 {
            dpi = 96;
        }
        (work, dpi)
    }
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
        if shared.failed.get() || shared.close_pending.get() {
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
        WM_DESTROY => {
            // Also reached by a DestroyWindow without WM_CLOSE (e.g. the owner going away).
            shared.destroyed.set(true);
            shared.open.set(false);
            shared.shut_down();
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
    /// Opens a window with a `size_dip` client area, centred on the monitor under the cursor;
    /// it can't be resized below `min_dip`.
    pub fn open(
        title: &str,
        size_dip: (f32, f32),
        min_dip: (f32, f32),
        app: Box<dyn UiApp>,
    ) -> windows::core::Result<UiWindow> {
        UiWindow::create(title, size_dip, min_dip, app, true)
    }

    /// `open`, but never shown (tests drive it with messages).
    #[cfg(test)]
    pub fn open_hidden(
        title: &str,
        size_dip: (f32, f32),
        min_dip: (f32, f32),
        app: Box<dyn UiApp>,
    ) -> windows::core::Result<UiWindow> {
        UiWindow::create(title, size_dip, min_dip, app, false)
    }

    fn create(
        title: &str,
        size_dip: (f32, f32),
        min_dip: (f32, f32),
        app: Box<dyn UiApp>,
        show: bool,
    ) -> windows::core::Result<UiWindow> {
        register_window_class();
        let title = HSTRING::from(title);
        let (work, monitor_dpi) = cursor_monitor();
        // SAFETY: plain FFI; the window is destroyed by PendingWindow on every error path.
        unsafe {
            // Created on the cursor's monitor so it gets that monitor's DPI.
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS_NAME,
                &title,
                WS_OVERLAPPEDWINDOW,
                work.left,
                work.top,
                window_size_for_client(size_dip, monitor_dpi).0,
                window_size_for_client(size_dip, monitor_dpi).1,
                None,
                None,
                Some(GetModuleHandleW(None)?.into()),
                None,
            )?;
            let pending = PendingWindow(hwnd);
            let dpi = GetDpiForWindow(hwnd);
            let (w, h) = window_size_for_client(size_dip, dpi);
            let (x, y) = centered_in(work, (w, h));
            SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE)?;
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
                canvas: Canvas::opaque(&d2d)?,
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
                close_pending: Cell::new(false),
                destroyed: Cell::new(false),
                min_client_dip: (min_dip.0.min(size_dip.0), min_dip.1.min(size_dip.1)),
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
        self.shared.open.get() && !self.shared.destroyed.get()
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
    pub fn hwnd(&self) -> HWND {
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
    fn window_is_centred_in_the_work_area() {
        let work = RECT {
            left: 1920,
            top: 0,
            right: 3840,
            bottom: 1040,
        };
        assert_eq!(centered_in(work, (900, 660)), (1920 + 510, 190));
        // Bigger than the work area: pinned to its top-left corner.
        assert_eq!(centered_in(work, (2000, 1200)), (1920, 0));
    }

    #[test]
    fn ui_frame_flag_resets_after_a_panic() {
        assert!(!in_ui_frame());
        assert!(as_ui_frame(in_ui_frame));
        let r = catch_unwind(|| as_ui_frame(|| panic!("frame bug")));
        assert!(r.is_err());
        assert!(!in_ui_frame());
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

    use lite_bar_core::ui::{id, theme, Rgba, TextEdit, TextStyle};
    use windows::Win32::Foundation::{LPARAM, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
    use windows::Win32::UI::WindowsAndMessaging::{
        DestroyWindow, IsWindow, SendMessageW, WM_CHAR, WM_CLOSE, WM_DPICHANGED, WM_LBUTTONDOWN,
        WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    };

    use super::*;

    /// COLORREF (0x00BBGGRR) of an opaque color.
    fn colorref(c: Rgba) -> u32 {
        u32::from(c.0) | u32::from(c.1) << 8 | u32::from(c.2) << 16
    }

    const RED: Rgba = Rgba(255, 0, 0, 255);

    /// Counts frames and `closed` calls; fills the window with `color`.
    struct TestApp {
        frames: Rc<Cell<u32>>,
        closed: Rc<Cell<u32>>,
        color: Rgba,
    }

    impl UiApp for TestApp {
        fn frame(&mut self, ui: &mut Ui) {
            assert!(in_ui_frame(), "frame runs flagged");
            self.frames.set(self.frames.get() + 1);
            ui.label("x", TextStyle::BODY, self.color);
            ui.button(id("ok"), "OK");
        }
        fn closed(&mut self) {
            assert!(in_ui_frame(), "closed runs flagged");
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
        let w = UiWindow::create(
            "Lite Bar test",
            (300.0, 200.0),
            (480.0, 320.0),
            Box::new(app),
            false,
        )
        .unwrap();
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
        let (w, frames, closed) = open_test(RED);
        assert!(w.is_open());
        send(&w, WM_PAINT, 0, 0);
        assert!(frames.get() >= 1);
        assert!(!in_ui_frame(), "flag cleared after the frame");
        // The window is cleared to the theme background.
        assert_eq!(w.pixel(250, 150), Some(colorref(theme::BG)));
        let hwnd = w.hwnd();
        w.close();
        assert!(!w.is_open());
        assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
        assert_eq!(closed.get(), 1);
        assert!(!in_ui_frame());
    }

    #[test]
    fn minimum_track_size_is_the_given_minimum() {
        let frames = Rc::new(Cell::new(0));
        let closed = Rc::new(Cell::new(0));
        let app = TestApp {
            frames,
            closed,
            color: RED,
        };
        let w =
            UiWindow::create("t", (900.0, 660.0), (720.0, 520.0), Box::new(app), false).unwrap();
        let mut info = MINMAXINFO::default();
        send(
            &w,
            WM_GETMINMAXINFO,
            0,
            &mut info as *mut MINMAXINFO as isize,
        );
        let dpi = unsafe { GetDpiForWindow(w.hwnd()) };
        let (mw, mh) = window_size_for_client((720.0, 520.0), dpi);
        assert_eq!((info.ptMinTrackSize.x, info.ptMinTrackSize.y), (mw, mh));
        // Opened at its default size, which is at least the minimum.
        let mut r = RECT::default();
        unsafe { GetClientRect(w.hwnd(), &mut r) }.unwrap();
        let s = scale_for_dpi(dpi);
        assert_eq!(r.right - r.left, dip_to_px(900.0, s));
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
            (480.0, 320.0),
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
            (480.0, 320.0),
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

    #[test]
    fn destroy_without_close_calls_closed_once() {
        let (w, _, closed) = open_test(RED);
        let hwnd = w.hwnd();
        unsafe { DestroyWindow(hwnd) }.unwrap();
        assert_eq!(closed.get(), 1);
        assert!(!w.is_open());
        w.close();
        drop(w);
        assert_eq!(closed.get(), 1);
    }

    /// Sends itself WM_CLOSE from inside a frame (a nested message while the host is busy).
    struct SelfClosingApp {
        hwnd: Rc<Cell<Option<HWND>>>,
        closed: Rc<Cell<u32>>,
    }

    impl UiApp for SelfClosingApp {
        fn frame(&mut self, _ui: &mut Ui) {
            if let Some(h) = self.hwnd.get() {
                unsafe { SendMessageW(h, WM_CLOSE, None, None) };
            }
        }
        fn closed(&mut self) {
            self.closed.set(self.closed.get() + 1);
        }
    }

    #[test]
    fn close_requested_during_a_frame_happens_when_it_returns() {
        let hwnd = Rc::new(Cell::new(None));
        let closed = Rc::new(Cell::new(0));
        let w = UiWindow::create(
            "t",
            (300.0, 200.0),
            (480.0, 320.0),
            Box::new(SelfClosingApp {
                hwnd: hwnd.clone(),
                closed: closed.clone(),
            }),
            false,
        )
        .unwrap();
        hwnd.set(Some(w.hwnd()));
        send(&w, WM_MOUSEMOVE, 0, at(5, 5));
        assert!(!w.is_open(), "closed without pumping messages");
        assert!(!unsafe { IsWindow(Some(w.hwnd())) }.as_bool());
        assert_eq!(closed.get(), 1);
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
            (480.0, 320.0),
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
