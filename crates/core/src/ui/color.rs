//! HSV colors and the color wheel's pixel buffer.

use crate::color::Rgb;

/// Hue in degrees (0–360), saturation and value in 0–1.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

impl Hsv {
    /// Channels in 0–1.
    fn to_rgb_f32(self) -> [f32; 3] {
        let f = |n: f32| {
            let k = (n + self.h / 60.0).rem_euclid(6.0);
            self.v - self.v * self.s * k.min(4.0 - k).clamp(0.0, 1.0)
        };
        [f(5.0), f(3.0), f(1.0)]
    }

    pub fn to_rgb(self) -> Rgb {
        let [r, g, b] = self
            .to_rgb_f32()
            .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        Rgb { r, g, b }
    }

    pub fn from_rgb(c: Rgb) -> Hsv {
        let [r, g, b] = [c.r, c.g, c.b].map(|x| f32::from(x) / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d == 0.0 {
            0.0
        } else if max == r {
            60.0 * ((g - b) / d)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        Hsv {
            h: h.rem_euclid(360.0),
            s: if max == 0.0 { 0.0 } else { d / max },
            v: max,
        }
    }
}

/// RGBA pixels (premultiplied alpha, row-major, top row first) of a hue/saturation
/// disc `diameter_px` wide at brightness `v`. Hue 0 (red) is at the top and runs
/// clockwise; saturation grows from the centre to the rim. Outside the disc is
/// transparent, with about 1 px of antialiasing at the rim.
pub fn wheel_pixels(diameter_px: u32, v: f32) -> Vec<u8> {
    let d = diameter_px as usize;
    let r = diameter_px as f32 / 2.0;
    let mut out = vec![0u8; d * d * 4];
    for y in 0..d {
        for x in 0..d {
            let dx = x as f32 + 0.5 - r;
            let dy = y as f32 + 0.5 - r;
            let dist = dx.hypot(dy);
            let alpha = (r - dist + 0.5).clamp(0.0, 1.0);
            if alpha == 0.0 {
                continue;
            }
            let hsv = Hsv {
                h: dx.atan2(-dy).to_degrees().rem_euclid(360.0),
                s: (dist / r).min(1.0),
                v,
            };
            let [cr, cg, cb] = hsv.to_rgb_f32();
            let i = (y * d + x) * 4;
            for (o, c) in out[i..i + 4].iter_mut().zip([cr, cg, cb, 1.0]) {
                *o = (c.clamp(0.0, 1.0) * alpha * 255.0).round() as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(hex: &str) -> Rgb {
        Rgb::parse_hex(hex).unwrap()
    }

    fn hsv(h: f32, s: f32, v: f32) -> Hsv {
        Hsv { h, s, v }
    }

    #[test]
    fn known_colors_to_rgb() {
        assert_eq!(hsv(0.0, 1.0, 1.0).to_rgb(), rgb("#FF0000"));
        assert_eq!(hsv(60.0, 1.0, 1.0).to_rgb(), rgb("#FFFF00"));
        assert_eq!(hsv(120.0, 1.0, 1.0).to_rgb(), rgb("#00FF00"));
        assert_eq!(hsv(240.0, 1.0, 1.0).to_rgb(), rgb("#0000FF"));
        assert_eq!(hsv(300.0, 1.0, 1.0).to_rgb(), rgb("#FF00FF"));
        assert_eq!(hsv(360.0, 1.0, 1.0).to_rgb(), rgb("#FF0000"));
        assert_eq!(hsv(200.0, 0.0, 1.0).to_rgb(), rgb("#FFFFFF"));
        assert_eq!(hsv(200.0, 1.0, 0.0).to_rgb(), rgb("#000000"));
        assert_eq!(hsv(0.0, 0.0, 0.5).to_rgb(), rgb("#808080"));
    }

    #[test]
    fn known_colors_from_rgb() {
        let h = Hsv::from_rgb(rgb("#00FF00"));
        assert_eq!((h.h, h.s, h.v), (120.0, 1.0, 1.0));
        let h = Hsv::from_rgb(rgb("#000000"));
        assert_eq!((h.s, h.v), (0.0, 0.0));
        let h = Hsv::from_rgb(rgb("#FF00FF"));
        assert_eq!(h.h, 300.0);
    }

    #[test]
    fn hsv_round_trip() {
        for hex in [
            "#7CFC7C", "#FFFFFF", "#000000", "#FF5555", "#FFB86C", "#F1FA8C", "#50FA7B", "#8BE9FD",
            "#5FA8FF", "#BD93F9", "#FF79C6", "#9AA3AD", "#010203", "#FE0001",
        ] {
            let c = rgb(hex);
            let h = Hsv::from_rgb(c);
            assert!((0.0..360.0).contains(&h.h), "{hex} hue {}", h.h);
            assert_eq!(h.to_rgb(), c, "{hex}");
        }
    }

    fn px(buf: &[u8], d: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * d + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn wheel_pixels_layout() {
        let d = 101;
        let buf = wheel_pixels(d, 1.0);
        assert_eq!(buf.len(), (d * d * 4) as usize);
        // Centre is white, corners transparent.
        assert_eq!(px(&buf, d, 50, 50), [255, 255, 255, 255]);
        assert_eq!(px(&buf, d, 0, 0), [0, 0, 0, 0]);
        // Just inside the rim: top is red and bottom is cyan.
        let top = px(&buf, d, 50, 2);
        assert_eq!(top[3], 255);
        assert!(top[0] > 240 && top[1] < 20 && top[2] < 20, "{top:?}");
        let bottom = px(&buf, d, 50, 98);
        assert!(
            bottom[0] < 20 && bottom[1] > 240 && bottom[2] > 240,
            "{bottom:?}"
        );
    }

    #[test]
    fn wheel_pixels_rim_is_antialiased_and_premultiplied() {
        let d = 101;
        let buf = wheel_pixels(d, 1.0);
        // At 45 degrees this pixel straddles the rim.
        let edge = px(&buf, d, 86, 15);
        assert!(edge[3] > 0 && edge[3] < 255, "{edge:?}");
        assert!(edge[0] <= edge[3] && edge[1] <= edge[3] && edge[2] <= edge[3]);
    }

    #[test]
    fn wheel_pixels_scale_with_value() {
        let buf = wheel_pixels(11, 0.5);
        assert_eq!(px(&buf, 11, 5, 5), [128, 128, 128, 255]);
    }
}
