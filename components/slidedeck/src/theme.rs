//! Colors and built-in themes.

use crate::types::Theme;

/// An sRGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rgb {
    pub(crate) r: u8,
    pub(crate) g: u8,
    pub(crate) b: u8,
}

/// Pure white.
pub(crate) const WHITE: Rgb = Rgb::new(0xFF, 0xFF, 0xFF);

/// The dark gray used for text on light backgrounds.
pub(crate) const INK: Rgb = Rgb::new(0x33, 0x33, 0x33);

impl Rgb {
    /// Create a color from its components.
    pub(crate) const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse `"RRGGBB"` or `"#RRGGBB"`. `field` names the input in errors.
    pub(crate) fn parse(value: &str, field: &str) -> Result<Self, String> {
        let hex = value.trim();
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        let invalid = || {
            format!(
                "{field}: `{value}` is not a valid color; expected six hex digits such as \"2196F3\" or \"#2196F3\""
            )
        };
        if hex.len() != 6 || !hex.is_ascii() {
            return Err(invalid());
        }
        let channel = |range: std::ops::Range<usize>| {
            hex.get(range)
                .and_then(|digits| u8::from_str_radix(digits, 16).ok())
                .ok_or_else(invalid)
        };
        Ok(Self::new(channel(0..2)?, channel(2..4)?, channel(4..6)?))
    }

    /// Parse an optional color, returning `default` when unset.
    pub(crate) fn parse_or(
        value: Option<&str>,
        field: &str,
        default: Self,
    ) -> Result<Self, String> {
        value.map_or(Ok(default), |value| Self::parse(value, field))
    }

    /// Format as six uppercase hex digits, as OOXML expects.
    pub(crate) fn hex(self) -> String {
        format!("{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// WCAG 2.x relative luminance in [0, 1].
    pub(crate) fn luminance(self) -> f64 {
        let linear = |channel: u8| {
            let v = f64::from(channel) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(self.r) + 0.7152 * linear(self.g) + 0.0722 * linear(self.b)
    }

    /// WCAG 2.x contrast ratio between two colors, in [1, 21].
    pub(crate) fn contrast(self, other: Self) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Whether light text reads better than dark text on this color.
    pub(crate) fn is_dark(self) -> bool {
        self.contrast(WHITE) >= self.contrast(INK)
    }

    /// White or dark gray, whichever is more readable on this background.
    pub(crate) fn readable_text(self) -> Self {
        if self.is_dark() {
            WHITE
        } else {
            INK
        }
    }

    /// Linearly interpolate towards `other` by `amount` in [0, 1].
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the interpolated value is clamped to [0, 255] before casting"
    )]
    pub(crate) fn mix(self, other: Self, amount: f64) -> Self {
        let lerp = |a: u8, b: u8| {
            let (a, b) = (f64::from(a), f64::from(b));
            (a + (b - a) * amount).round().clamp(0.0, 255.0) as u8
        };
        Self::new(
            lerp(self.r, other.r),
            lerp(self.g, other.g),
            lerp(self.b, other.b),
        )
    }
}

/// The resolved colors and fonts of a theme.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) name: &'static str,
    pub(crate) background: Rgb,
    pub(crate) foreground: Rgb,
    pub(crate) accents: [Rgb; 4],
    pub(crate) subtle: Rgb,
    pub(crate) title_font: &'static str,
    pub(crate) body_font: &'static str,
}

impl Palette {
    /// Look up a built-in theme.
    pub(crate) fn of(theme: Theme) -> Self {
        let rgb = |hex: u32| {
            let [_, r, g, b] = hex.to_be_bytes();
            Rgb::new(r, g, b)
        };
        let (name, bg, fg, accents, subtle, title_font, body_font) = match theme {
            Theme::CorporateBlue => (
                "Corporate Blue",
                0x1B_2A4A,
                0xFF_FFFF,
                [0x21_96F3, 0x4C_AF50, 0xFF_9800, 0xE9_1E63],
                0x88_99AA,
                "Segoe UI",
                "Segoe UI",
            ),
            Theme::DarkGradient => (
                "Dark Gradient",
                0x0D_1117,
                0xE6_EDF3,
                [0x58_A6FF, 0x3F_B950, 0xD2_9922, 0xF8_5149],
                0x8B_949E,
                "Segoe UI",
                "Segoe UI",
            ),
            Theme::LightClean => (
                "Light Clean",
                0xFF_FFFF,
                0x33_3333,
                [0x3F_51B5, 0xFF_5722, 0x00_9688, 0x79_5548],
                0x75_7575,
                "Calibri",
                "Calibri",
            ),
            Theme::Emerald => (
                "Emerald",
                0x00_4D40,
                0xFF_FFFF,
                [0x00_E676, 0xFF_D740, 0x40_C4FF, 0xFF_6E40],
                0x80_CBC4,
                "Segoe UI",
                "Segoe UI",
            ),
            Theme::Sunset => (
                "Sunset",
                0x37_0617,
                0xFF_FFFF,
                [0xF4_8C06, 0xFF_BA08, 0xE8_5D04, 0xDC_2F02],
                0xD4_A373,
                "Segoe UI",
                "Segoe UI",
            ),
            Theme::Black => (
                "Black",
                0x00_0000,
                0xFF_FFFF,
                [0x58_A6FF, 0x3F_B950, 0xD2_9922, 0xF8_5149],
                0x8B_949E,
                "Segoe UI",
                "Segoe UI",
            ),
            Theme::Brutalist => (
                "Brutalist",
                0x0A_0A0A,
                0xF5_F5F5,
                [0xFF_0000, 0xFF_FFFF, 0xFF_3333, 0xCC_CCCC],
                0x66_6666,
                "Arial Black",
                "Arial",
            ),
        };
        Self {
            name,
            background: rgb(bg),
            foreground: rgb(fg),
            accents: accents.map(rgb),
            subtle: rgb(subtle),
            title_font,
            body_font,
        }
    }

    /// The primary accent.
    pub(crate) fn accent(&self) -> Rgb {
        self.accents[0]
    }

    /// The `index`-th color of the series palette, cycling.
    pub(crate) fn series(&self, index: usize) -> Rgb {
        const EXTRA: [Rgb; 4] = [
            Rgb::new(0x9C, 0x27, 0xB0),
            Rgb::new(0x00, 0xBC, 0xD4),
            Rgb::new(0x60, 0x7D, 0x8B),
            Rgb::new(0xCD, 0xDC, 0x39),
        ];
        let all = [
            self.accents[0],
            self.accents[1],
            self.accents[2],
            self.accents[3],
            EXTRA[0],
            EXTRA[1],
            EXTRA[2],
            EXTRA[3],
        ];
        all.get(index % all.len())
            .copied()
            .unwrap_or(self.accents[0])
    }

    /// Text color for secondary text on `background`: the theme's subtle
    /// color if it's legible there, otherwise the most readable fallback.
    pub(crate) fn muted_on(&self, background: Rgb) -> Rgb {
        if self.subtle.contrast(background) >= 3.0 {
            self.subtle
        } else {
            background.readable_text()
        }
    }

    /// Text color for primary text on `background`: the theme foreground if
    /// it's legible there, otherwise the most readable fallback.
    pub(crate) fn text_on(&self, background: Rgb) -> Rgb {
        if self.foreground.contrast(background) >= 4.5 {
            self.foreground
        } else {
            background.readable_text()
        }
    }
}
