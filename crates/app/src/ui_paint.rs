//! Paints the core UI toolkit's `Cmd` lists with Direct2D and measures text with
//! DirectWrite. Shared by the settings window (`ui_host`) and the overlay.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use lite_bar_core::layout::Size;
use lite_bar_core::ui::{Cmd, Pt, Rect, Rgba, TextMeasure, TextStyle};
use windows::core::w;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
    D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory, ID2D1RenderTarget, ID2D1SolidColorBrush,
    ID2D1StrokeStyle, D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
    D2D1_BITMAP_PROPERTIES, D2D1_CAP_STYLE_FLAT, D2D1_DASH_STYLE_CUSTOM,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_LINE_JOIN_MITER,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
#[cfg(test)]
use windows::Win32::Graphics::Gdi::GetPixel;
use windows::Win32::Graphics::Gdi::{SelectObject, HDC};
use windows_numerics::Vector2;

use crate::log;
use crate::overlay::{Dib, MemDc};

/// 1 DIP dashes: 4 on, 3 off, in stroke widths.
const DASHES: [f32; 2] = [4.0, 3.0];

/// Premultiplied RGBA to premultiplied BGRA, for `DXGI_FORMAT_B8G8R8A8_UNORM`.
fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for px in out.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    out
}

/// Straight-alpha Direct2D color; D2D premultiplies when blending.
pub(crate) fn d2d_color(c: Rgba) -> D2D1_COLOR_F {
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

/// Line end points and width in pixels. Horizontal and vertical lines get a whole-pixel
/// width and sit on pixel centres (odd widths) or edges (even widths), so they stay crisp.
fn line_px(from: Pt, to: Pt, width: f32, scale: f32) -> (Vector2, Vector2, f32) {
    let mut a = Vector2 {
        X: from.x * scale,
        Y: from.y * scale,
    };
    let mut b = Vector2 {
        X: to.x * scale,
        Y: to.y * scale,
    };
    let horizontal = from.y == to.y;
    let vertical = from.x == to.x;
    if !horizontal && !vertical {
        return (a, b, width * scale);
    }
    let w = (width * scale).round().max(1.0);
    let snap = |v: f32| {
        if w as i32 % 2 == 1 {
            v.floor() + 0.5
        } else {
            v.round()
        }
    };
    if horizontal {
        a.Y = snap(a.Y);
        b.Y = a.Y;
    }
    if vertical {
        a.X = snap(a.X);
        b.X = a.X;
    }
    (a, b, w)
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
    /// A measuring failure has been logged (logged once, not every frame).
    measure_failed: Cell<bool>,
}

impl Fonts {
    fn new() -> windows::core::Result<Fonts> {
        // SAFETY: plain COM call.
        let dwrite = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }?;
        Ok(Fonts {
            dwrite,
            formats: RefCell::new(HashMap::new()),
            measure_failed: Cell::new(false),
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
    /// The target the cached resources belong to (held so its address cannot be reused).
    target: Option<ID2D1RenderTarget>,
}

impl Painter {
    pub fn new() -> windows::core::Result<Painter> {
        Ok(Painter {
            fonts: Fonts::new()?,
            images: HashMap::new(),
            dashed: None,
            target: None,
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
        if self.target.as_ref() != Some(target) {
            self.images.clear();
            self.dashed = None;
            self.target = Some(target.clone());
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
                let (a, b, w) = line_px(*from, *to, *width, scale);
                unsafe { target.DrawLine(a, b, brush, w, None) };
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
            .unwrap_or_else(|e| {
                if !self.fonts.measure_failed.replace(true) {
                    log::warn(&format!("DirectWrite text measuring failed: {e}"));
                }
                (0.0, style.size)
            })
    }
}

/// A software DC render target drawing into a memory DIB, reallocated only on resize.
pub(crate) struct Canvas {
    // Drop order: the target before the DC/DIB it is bound to; the DC before the DIB.
    target: ID2D1DCRenderTarget,
    dc: MemDc,
    dib: Option<Dib>,
}

impl Canvas {
    pub(crate) fn new(d2d: &ID2D1Factory) -> windows::core::Result<Canvas> {
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
    pub(crate) fn draw(
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

    /// The memory DC holding the last drawn pixels.
    pub(crate) fn dc(&self) -> HDC {
        self.dc.dc
    }

    #[cfg(test)]
    pub(crate) fn pixel(&self, x: i32, y: i32) -> u32 {
        // SAFETY: plain GDI call on our memory DC.
        unsafe { GetPixel(self.dc.dc, x, y) }.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_rgba_is_converted_to_bgra() {
        assert_eq!(
            rgba_to_bgra(&[1, 2, 3, 4, 10, 20, 30, 40]),
            vec![3, 2, 1, 4, 30, 20, 10, 40]
        );
    }
}

#[cfg(test)]
mod win_tests {
    use lite_bar_core::ui::theme;
    use windows::Win32::Graphics::Direct2D::{
        D2D1CreateFactory, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    };
    use windows::Win32::Graphics::DirectWrite::DWRITE_LINE_METRICS;

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
    fn axis_aligned_lines_are_pixel_snapped() {
        let mut p = Painter::new().unwrap();
        let mut c = new_canvas();
        let line = |from: Pt, to: Pt| Cmd::Line {
            from,
            to,
            color: BLUE,
            width: 1.0,
        };
        let cmds = [
            fill(Rect::new(0.0, 0.0, 20.0, 20.0), RED),
            line(Pt { x: 0.0, y: 5.0 }, Pt { x: 20.0, y: 5.0 }),
            line(Pt { x: 15.0, y: 0.0 }, Pt { x: 15.0, y: 20.0 }),
        ];
        canvas_paint(&mut p, &mut c, &cmds, 1.0);
        assert_eq!(c.pixel(5, 5), colorref(BLUE), "horizontal: one crisp row");
        assert_eq!(c.pixel(5, 4), colorref(RED));
        assert_eq!(
            c.pixel(15, 10),
            colorref(BLUE),
            "vertical: one crisp column"
        );
        assert_eq!(c.pixel(14, 10), colorref(RED));
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
}
