//! Color WIT component: convert between CSS color spaces.
//!
//! Conversions are exposed through a single `color` resource. Each `from-*`
//! static constructor ingests a color in one space and each `to-*` method reads
//! it back out in another, giving `M × N` conversion paths from `M + N` entry
//! points.
//!
//! Internally every color is stored as CIE XYZ (D65 white point) plus an alpha
//! channel. XYZ is an unbounded, device-independent hub, so converting *into*
//! it and back *out* of it is (near) lossless for every supported space —
//! including wide-gamut colors that fall outside the sRGB cube. The bounded,
//! display-oriented spaces (`srgb`, `hsl`, `hwb`, `okhsl`, and the hex string)
//! are clamped into the sRGB gamut on the way out; the unbounded perceptual
//! spaces (`lab`, `lch`, `oklab`, `oklch`) are returned as-is. CIE L*a*b* and
//! LCH use the D50 white point to match CSS Color 4.
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_docs_in_private_items,
    reason = "wit-bindgen generates unsafe FFI glue and undocumented items"
)]

wit_bindgen::generate!({
    world: "convert",
    path: "wit",
});

use exports::yoshuawuyts::color::conversion::{
    Color, ColorError, Guest, GuestColor, Hsl as WitHsl, Hwb as WitHwb, Lab as WitLab,
    Lch as WitLch, Okhsl as WitOkhsl, Oklab as WitOklab, Oklch as WitOklch, Srgb as WitSrgb,
};

use palette::{
    chromatic_adaptation::AdaptInto, convert::FromColorUnclamped, encoding, white_point::D50,
    white_point::D65, Hsl, Hwb, Lab, Lch, Okhsl, Oklab, Oklch, Srgb, Xyz,
};
use std::convert::TryFrom;

/// Device-independent hub the component stores every color in.
type Hub = Xyz<D65, f32>;
type SrgbF = Srgb<f32>;
type HslF = Hsl<encoding::Srgb, f32>;
type HwbF = Hwb<encoding::Srgb, f32>;
/// CIE L\*a\*b\* and LCH use the D50 white point to match CSS Color 4.
type LabF = Lab<D50, f32>;
type LchF = Lch<D50, f32>;
type Xyz50 = Xyz<D50, f32>;
type OklabF = Oklab<f32>;
type OklchF = Oklch<f32>;
type OkhslF = Okhsl<f32>;

/// An opaque color value, stored as CIE XYZ (D65) plus a straight alpha
/// channel in `[0.0, 1.0]`.
struct ColorValue {
    xyz: Hub,
    alpha: f32,
}

impl ColorValue {
    /// Wrap a hub color and alpha into an exported resource handle.
    fn handle(xyz: Hub, alpha: f32) -> Color {
        Color::new(Self { xyz, alpha })
    }

    /// The sRGB representation, clamped into the `[0.0, 1.0]` gamut so the
    /// bounded display spaces (`srgb`, `hsl`, `hwb`, `okhsl`, hex) are well
    /// defined.
    fn clamped_srgb(&self) -> SrgbF {
        let s = SrgbF::from_color_unclamped(self.xyz);
        Srgb::new(clamp_unit(s.red), clamp_unit(s.green), clamp_unit(s.blue))
    }

    /// The stored color as CIE XYZ adapted to the D50 white point (CSS Lab/LCH).
    fn xyz_d50(&self) -> Xyz50 {
        self.xyz.adapt_into()
    }
}

/// Build the storage hub from a D50 XYZ color (CSS Lab/LCH input).
fn hub_from_d50(xyz: Xyz50) -> Hub {
    xyz.adapt_into()
}

/// Clamp a value into the `[0.0, 1.0]` range.
fn clamp_unit(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// Validate that an alpha value lies within `[0.0, 1.0]`.
fn check_alpha(alpha: f32) -> Result<f32, ColorError> {
    if (0.0..=1.0).contains(&alpha) {
        Ok(alpha)
    } else {
        Err(ColorError::OutOfRange(format!(
            "alpha must be in [0.0, 1.0], got {alpha}"
        )))
    }
}

/// Reject non-finite (NaN / infinite) color components so they cannot poison
/// later conversions.
fn check_finite(values: &[f32]) -> Result<(), ColorError> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(ColorError::OutOfRange(
            "color components must be finite".to_string(),
        ))
    }
}

/// Reject bounded color components that fall outside the documented
/// `[0.0, 1.0]` range. NaN values also fail this check, so it implies
/// `check_finite` for the channels it covers.
fn check_unit(values: &[f32]) -> Result<(), ColorError> {
    if values.iter().all(|v| (0.0..=1.0).contains(v)) {
        Ok(())
    } else {
        Err(ColorError::OutOfRange(
            "color components must be in [0.0, 1.0]".to_string(),
        ))
    }
}

/// Normalize HWB whiteness/blackness the way CSS does: clamp negatives to zero
/// and, when they sum to more than one, scale them so the sum is one (the color
/// is then an achromatic gray).
fn normalize_hwb(whiteness: f32, blackness: f32) -> (f32, f32) {
    let w = whiteness.max(0.0);
    let b = blackness.max(0.0);
    let sum = w + b;
    if sum > 1.0 {
        (w / sum, b / sum)
    } else {
        (w, b)
    }
}

/// Normalize a hue in degrees to the `[0.0, 360.0)` range.
fn normalize_hue(degrees: f32) -> f32 {
    let h = degrees % 360.0;
    if h < 0.0 {
        h + 360.0
    } else {
        h
    }
}

/// Convert a unit-interval channel to an 8-bit value.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to [0.0, 255.0] before the cast"
)]
fn to_u8(value: f32) -> u8 {
    (clamp_unit(value) * 255.0).round() as u8
}

impl GuestColor for ColorValue {
    fn from_hex(value: String) -> Result<Color, ColorError> {
        let (srgb, alpha) = parse_hex(&value)?;
        Ok(Self::handle(Hub::from_color_unclamped(srgb), alpha))
    }

    fn from_srgb(value: WitSrgb, alpha: f32) -> Result<Color, ColorError> {
        let alpha = check_alpha(alpha)?;
        check_unit(&[value.red, value.green, value.blue])?;
        let c = SrgbF::new(value.red, value.green, value.blue);
        Ok(Self::handle(Hub::from_color_unclamped(c), alpha))
    }

    fn from_hsl(value: WitHsl, alpha: f32) -> Result<Color, ColorError> {
        let alpha = check_alpha(alpha)?;
        check_finite(&[value.hue])?;
        check_unit(&[value.saturation, value.lightness])?;
        let c = HslF::new(value.hue, value.saturation, value.lightness);
        Ok(Self::handle(Hub::from_color_unclamped(c), alpha))
    }

    fn from_hwb(value: WitHwb, alpha: f32) -> Result<Color, ColorError> {
        let alpha = check_alpha(alpha)?;
        check_finite(&[value.hue, value.whiteness, value.blackness])?;
        let (whiteness, blackness) = normalize_hwb(value.whiteness, value.blackness);
        let c = HwbF::new(value.hue, whiteness, blackness);
        Ok(Self::handle(Hub::from_color_unclamped(c), alpha))
    }

    fn from_lab(value: WitLab, alpha: f32) -> Result<Color, ColorError> {
        let alpha = check_alpha(alpha)?;
        check_finite(&[value.lightness, value.a, value.b])?;
        let c = LabF::new(value.lightness, value.a, value.b);
        Ok(Self::handle(
            hub_from_d50(Xyz50::from_color_unclamped(c)),
            alpha,
        ))
    }

    fn from_lch(value: WitLch, alpha: f32) -> Result<Color, ColorError> {
        let alpha = check_alpha(alpha)?;
        check_finite(&[value.lightness, value.chroma, value.hue])?;
        let c = LchF::new(value.lightness, value.chroma, value.hue);
        Ok(Self::handle(
            hub_from_d50(Xyz50::from_color_unclamped(c)),
            alpha,
        ))
    }

    fn from_oklab(value: WitOklab, alpha: f32) -> Result<Color, ColorError> {
        let alpha = check_alpha(alpha)?;
        check_finite(&[value.lightness, value.a, value.b])?;
        let c = OklabF::new(value.lightness, value.a, value.b);
        Ok(Self::handle(Hub::from_color_unclamped(c), alpha))
    }

    fn from_oklch(value: WitOklch, alpha: f32) -> Result<Color, ColorError> {
        let alpha = check_alpha(alpha)?;
        check_finite(&[value.lightness, value.chroma, value.hue])?;
        let c = OklchF::new(value.lightness, value.chroma, value.hue);
        Ok(Self::handle(Hub::from_color_unclamped(c), alpha))
    }

    fn from_okhsl(value: WitOkhsl, alpha: f32) -> Result<Color, ColorError> {
        let alpha = check_alpha(alpha)?;
        check_finite(&[value.hue])?;
        check_unit(&[value.saturation, value.lightness])?;
        let c = OkhslF::new(value.hue, value.saturation, value.lightness);
        Ok(Self::handle(Hub::from_color_unclamped(c), alpha))
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn to_hex(&self) -> String {
        let s = self.clamped_srgb();
        let (r, g, b) = (to_u8(s.red), to_u8(s.green), to_u8(s.blue));
        if self.alpha < 1.0 {
            format!("#{r:02x}{g:02x}{b:02x}{:02x}", to_u8(self.alpha))
        } else {
            format!("#{r:02x}{g:02x}{b:02x}")
        }
    }

    fn to_srgb(&self) -> WitSrgb {
        let s = self.clamped_srgb();
        WitSrgb {
            red: s.red,
            green: s.green,
            blue: s.blue,
        }
    }

    fn to_hsl(&self) -> WitHsl {
        let c = HslF::from_color_unclamped(self.clamped_srgb());
        WitHsl {
            hue: normalize_hue(c.hue.into_degrees()),
            saturation: c.saturation,
            lightness: c.lightness,
        }
    }

    fn to_hwb(&self) -> WitHwb {
        let c = HwbF::from_color_unclamped(self.clamped_srgb());
        WitHwb {
            hue: normalize_hue(c.hue.into_degrees()),
            whiteness: c.whiteness,
            blackness: c.blackness,
        }
    }

    fn to_lab(&self) -> WitLab {
        let c = LabF::from_color_unclamped(self.xyz_d50());
        WitLab {
            lightness: c.l,
            a: c.a,
            b: c.b,
        }
    }

    fn to_lch(&self) -> WitLch {
        let c = LchF::from_color_unclamped(self.xyz_d50());
        WitLch {
            lightness: c.l,
            chroma: c.chroma,
            hue: normalize_hue(c.hue.into_degrees()),
        }
    }

    fn to_oklab(&self) -> WitOklab {
        let c = OklabF::from_color_unclamped(self.xyz);
        WitOklab {
            lightness: c.l,
            a: c.a,
            b: c.b,
        }
    }

    fn to_oklch(&self) -> WitOklch {
        let c = OklchF::from_color_unclamped(self.xyz);
        WitOklch {
            lightness: c.l,
            chroma: c.chroma,
            hue: normalize_hue(c.hue.into_degrees()),
        }
    }

    fn to_okhsl(&self) -> WitOkhsl {
        // Okhsl encodes the sRGB gamut as a cylinder, so derive it from the
        // gamut-clamped sRGB value to keep saturation/lightness in [0.0, 1.0].
        let c = OkhslF::from_color_unclamped(self.clamped_srgb());
        WitOkhsl {
            hue: normalize_hue(c.hue.into_degrees()),
            saturation: c.saturation,
            lightness: c.lightness,
        }
    }
}

/// Parse a `#RGB`, `#RGBA`, `#RRGGBB`, or `#RRGGBBAA` hex string into an sRGB
/// color and a straight alpha channel in `[0.0, 1.0]`.
fn parse_hex(value: &str) -> Result<(SrgbF, f32), ColorError> {
    let invalid = || ColorError::InvalidHex(value.to_string());
    let hex = value.strip_prefix('#').ok_or_else(invalid)?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }

    // Expand the shorthand forms (each nibble is doubled) to a flat list of
    // 8-bit channels: [r, g, b] or [r, g, b, a].
    let channels: Vec<u8> = match hex.len() {
        3 | 4 => hex
            .chars()
            .map(|c| {
                let n = u8::try_from(c.to_digit(16).unwrap_or(0)).unwrap_or(0);
                n << 4 | n
            })
            .collect(),
        6 | 8 => (0..hex.len() / 2)
            .map(|i| {
                let byte = hex.get(i * 2..i * 2 + 2).unwrap_or("");
                u8::from_str_radix(byte, 16).unwrap_or(0)
            })
            .collect(),
        _ => return Err(invalid()),
    };

    let ([r, g, b] | [r, g, b, _]) = channels.as_slice() else {
        return Err(invalid());
    };
    let srgb = Srgb::new(*r, *g, *b).into_format::<f32>();
    let alpha = channels.get(3).map_or(1.0, |&a| f32::from(a) / 255.0);
    Ok((srgb, alpha))
}

/// The WIT component implementation.
struct Component;

impl Guest for Component {
    type Color = ColorValue;
}

export!(Component);

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a color value directly from an sRGB triple, bypassing the
    /// resource-handle machinery so the conversion math can be unit tested on
    /// the native host.
    fn from_srgb(red: f32, green: f32, blue: f32, alpha: f32) -> ColorValue {
        ColorValue {
            xyz: Hub::from_color_unclamped(SrgbF::new(red, green, blue)),
            alpha,
        }
    }

    fn assert_close(label: &str, got: f32, want: f32, eps: f32) {
        assert!(
            (got - want).abs() <= eps,
            "{}: got {}, want {} (eps {})",
            label,
            got,
            want,
            eps
        );
    }

    #[test]
    fn parses_six_digit_hex() {
        let (srgb, alpha) = parse_hex("#ff8800").unwrap();
        assert_close("red", srgb.red, 1.0, 1e-4);
        assert_close("green", srgb.green, 0.533_333, 1e-3);
        assert_close("blue", srgb.blue, 0.0, 1e-4);
        assert_close("alpha", alpha, 1.0, 1e-6);
    }

    #[test]
    fn parses_shorthand_hex() {
        let (long, _) = parse_hex("#ffaa00").unwrap();
        let (short, _) = parse_hex("#fa0").unwrap();
        assert_close("red", short.red, long.red, 1e-6);
        assert_close("green", short.green, long.green, 1e-6);
        assert_close("blue", short.blue, long.blue, 1e-6);
    }

    #[test]
    fn parses_hex_with_alpha() {
        let (_, alpha) = parse_hex("#ff000080").unwrap();
        assert_close("alpha", alpha, 0.501_960, 1e-4);
        let (_, short_alpha) = parse_hex("#f008").unwrap();
        assert_close("short-alpha", short_alpha, 0.533_333, 1e-3);
    }

    #[test]
    fn rejects_malformed_hex() {
        for bad in ["ff0000", "#fffff", "#gggggg", "#12345", ""] {
            assert!(
                matches!(parse_hex(bad), Err(ColorError::InvalidHex(_))),
                "expected {:?} to be rejected",
                bad
            );
        }
    }

    #[test]
    fn red_round_trips_to_hex() {
        let red = from_srgb(1.0, 0.0, 0.0, 1.0);
        assert_eq!(red.to_hex(), "#ff0000");
    }

    #[test]
    fn hex_includes_alpha_when_translucent() {
        let mut red = from_srgb(1.0, 0.0, 0.0, 1.0);
        red.alpha = 0.5;
        assert_eq!(red.to_hex(), "#ff000080");
    }

    #[test]
    fn red_converts_to_hsl() {
        let hsl = from_srgb(1.0, 0.0, 0.0, 1.0).to_hsl();
        assert_close("hue", hsl.hue, 0.0, 1e-3);
        assert_close("saturation", hsl.saturation, 1.0, 1e-3);
        assert_close("lightness", hsl.lightness, 0.5, 1e-3);
    }

    #[test]
    fn out_of_gamut_oklch_round_trips() {
        // A saturated Oklch color outside the sRGB gamut. Because colors are
        // stored as unbounded XYZ, reading it back as Oklch is lossless.
        let value = ColorValue {
            xyz: Hub::from_color_unclamped(OklchF::new(0.7, 0.2, 30.0)),
            alpha: 1.0,
        };
        let got = value.to_oklch();
        assert_close("lightness", got.lightness, 0.7, 1e-3);
        assert_close("chroma", got.chroma, 0.2, 1e-3);
        assert_close("hue", got.hue, 30.0, 1e-2);
    }

    #[test]
    fn normalizes_negative_hue() {
        assert_close("wrap", normalize_hue(-90.0), 270.0, 1e-6);
        assert_close("identity", normalize_hue(45.0), 45.0, 1e-6);
        assert_close("wrap-360", normalize_hue(360.0), 0.0, 1e-6);
    }

    #[test]
    fn red_converts_to_css_d50_lab() {
        // CSS Color 4 reference value for sRGB red is Lab(54.29, 80.81, 69.89).
        let lab = from_srgb(1.0, 0.0, 0.0, 1.0).to_lab();
        assert_close("lightness", lab.lightness, 54.29, 0.1);
        assert_close("a", lab.a, 80.81, 0.2);
        assert_close("b", lab.b, 69.89, 0.2);
    }

    #[test]
    fn lab_round_trips_through_d50() {
        let value = ColorValue {
            xyz: hub_from_d50(Xyz50::from_color_unclamped(LabF::new(62.0, 25.0, -40.0))),
            alpha: 1.0,
        };
        let lab = value.to_lab();
        assert_close("lightness", lab.lightness, 62.0, 1e-2);
        assert_close("a", lab.a, 25.0, 1e-2);
        assert_close("b", lab.b, -40.0, 1e-2);
    }

    #[test]
    fn hwb_normalizes_when_sum_exceeds_one() {
        // whiteness + blackness > 1 must collapse to an achromatic gray rather
        // than producing a negative saturation.
        let (w, b) = normalize_hwb(0.8, 0.8);
        assert_close("whiteness", w, 0.5, 1e-6);
        assert_close("blackness", b, 0.5, 1e-6);

        let value = ColorValue {
            xyz: Hub::from_color_unclamped(HwbF::new(120.0, w, b)),
            alpha: 1.0,
        };
        let srgb = value.to_srgb();
        assert_close("gray r==g", srgb.red, srgb.green, 1e-3);
        assert_close("gray g==b", srgb.green, srgb.blue, 1e-3);
    }

    #[test]
    fn okhsl_stays_in_unit_range_for_wide_gamut() {
        let value = ColorValue {
            xyz: Hub::from_color_unclamped(OklchF::new(0.7, 0.2, 30.0)),
            alpha: 1.0,
        };
        let okhsl = value.to_okhsl();
        assert!(
            (0.0..=1.0).contains(&okhsl.saturation),
            "saturation out of range: {}",
            okhsl.saturation
        );
        assert!(
            (0.0..=1.0).contains(&okhsl.lightness),
            "lightness out of range: {}",
            okhsl.lightness
        );
    }

    #[test]
    fn rejects_non_finite_components() {
        assert!(matches!(
            ColorValue::from_srgb(
                WitSrgb {
                    red: f32::NAN,
                    green: 0.0,
                    blue: 0.0,
                },
                1.0,
            ),
            Err(ColorError::OutOfRange(_))
        ));
    }

    #[test]
    fn rejects_out_of_range_alpha() {
        let value = WitSrgb {
            red: 1.0,
            green: 0.0,
            blue: 0.0,
        };
        assert!(matches!(
            ColorValue::from_srgb(value, 1.5),
            Err(ColorError::OutOfRange(_))
        ));
    }

    #[test]
    fn rejects_out_of_range_bounded_components() {
        assert!(matches!(
            ColorValue::from_srgb(
                WitSrgb {
                    red: 1.5,
                    green: 0.0,
                    blue: 0.0,
                },
                1.0,
            ),
            Err(ColorError::OutOfRange(_))
        ));
        assert!(matches!(
            ColorValue::from_hsl(
                WitHsl {
                    hue: 120.0,
                    saturation: 1.2,
                    lightness: 0.5,
                },
                1.0,
            ),
            Err(ColorError::OutOfRange(_))
        ));
        assert!(matches!(
            ColorValue::from_okhsl(
                WitOkhsl {
                    hue: 120.0,
                    saturation: 0.5,
                    lightness: -0.1,
                },
                1.0,
            ),
            Err(ColorError::OutOfRange(_))
        ));
    }
}
