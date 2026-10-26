//! Hex color parsing and RGBA conversion.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An sRGB color with 8 bits per channel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(thiserror::Error, Debug)]
pub enum ColorError {
    #[error("not a #RRGGBB color: {0}")]
    Invalid(String),
}

impl Rgb {
    /// Parses `#RRGGBB` or `RRGGBB` (case-insensitive, surrounding whitespace trimmed).
    pub fn parse_hex(s: &str) -> Result<Rgb, ColorError> {
        let invalid = || ColorError::Invalid(s.to_string());
        let t = s.trim();
        let digits = t.strip_prefix('#').unwrap_or(t);
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        let channel = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).map_err(|_| invalid());
        Ok(Rgb {
            r: channel(0)?,
            g: channel(2)?,
            b: channel(4)?,
        })
    }

    /// Formats as uppercase `#RRGGBB`.
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

/// Converts to straight (not premultiplied) RGBA floats; Direct2D premultiplies.
/// `opacity_pct` above 100 is treated as 100.
pub fn rgba_f32(color: Rgb, opacity_pct: u8) -> [f32; 4] {
    [
        f32::from(color.r) / 255.0,
        f32::from(color.g) / 255.0,
        f32::from(color.b) / 255.0,
        f32::from(opacity_pct.min(100)) / 100.0,
    ]
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Rgb::parse_hex(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_with_and_without_hash() {
        assert_eq!(
            Rgb::parse_hex("#7CFC7C").unwrap(),
            Rgb {
                r: 0x7C,
                g: 0xFC,
                b: 0x7C
            }
        );
        assert_eq!(
            Rgb::parse_hex(" 7cfc7c ").unwrap(),
            Rgb {
                r: 0x7C,
                g: 0xFC,
                b: 0x7C
            }
        );
    }

    #[test]
    fn rejects_bad_input() {
        for s in ["", "#FFF", "#GGGGGG", "#1234567", "red"] {
            assert!(Rgb::parse_hex(s).is_err(), "{s}");
        }
    }

    #[test]
    fn hex_roundtrip_is_uppercase() {
        assert_eq!(Rgb::parse_hex("#a1b2c3").unwrap().to_hex(), "#A1B2C3");
    }

    #[test]
    fn rgba_applies_opacity() {
        assert_eq!(
            rgba_f32(Rgb { r: 255, g: 0, b: 0 }, 55),
            [1.0, 0.0, 0.0, 0.55]
        );
        assert_eq!(rgba_f32(Rgb { r: 0, g: 0, b: 0 }, 250)[3], 1.0);
    }

    #[test]
    fn serde_as_hex_string() {
        #[derive(serde::Deserialize)]
        struct W {
            c: Rgb,
        }
        let w: W = toml::from_str("c = \"#000000\"").unwrap();
        assert_eq!(w.c, Rgb { r: 0, g: 0, b: 0 });
    }
}
