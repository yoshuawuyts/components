//! XML escaping and unit conversion helpers.

/// EMUs (English Metric Units) per inch. OOXML positions everything in EMUs.
pub(crate) const EMU_PER_INCH: f64 = 914_400.0;

/// EMUs per typographic point.
pub(crate) const EMU_PER_POINT: f64 = 12_700.0;

/// Slide width in inches (16:9 widescreen).
pub(crate) const SLIDE_WIDTH: f32 = 13.333;

/// Slide height in inches (16:9 widescreen).
pub(crate) const SLIDE_HEIGHT: f32 = 7.5;

/// Slide width in EMUs.
pub(crate) const SLIDE_WIDTH_EMU: i64 = 12_192_000;

/// Slide height in EMUs.
pub(crate) const SLIDE_HEIGHT_EMU: i64 = 6_858_000;

/// `xmlns:a` — DrawingML.
pub(crate) const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
/// `xmlns:r` — relationships.
pub(crate) const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
/// `xmlns:p` — PresentationML.
pub(crate) const NS_P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
/// `xmlns:c` — DrawingML charts.
pub(crate) const NS_C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";

/// The XML declaration every part starts with.
pub(crate) const DECLARATION: &str =
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// Escape text for use in XML content or attribute values, dropping control
/// characters that XML 1.0 can't represent.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Convert inches to EMUs.
#[allow(
    clippy::cast_possible_truncation,
    reason = "slide coordinates are validated to lie far inside the i64 range"
)]
pub(crate) fn emu(inches: f32) -> i64 {
    (f64::from(inches) * EMU_PER_INCH).round() as i64
}

/// Convert points to EMUs.
#[allow(
    clippy::cast_possible_truncation,
    reason = "line widths are validated to lie far inside the i64 range"
)]
pub(crate) fn emu_points(points: f32) -> i64 {
    (f64::from(points) * EMU_PER_POINT).round() as i64
}

/// Convert a font size in points to OOXML hundredths of a point.
#[allow(
    clippy::cast_possible_truncation,
    reason = "font sizes are validated to lie in [1, 400]"
)]
pub(crate) fn centipoints(points: f32) -> i64 {
    (f64::from(points) * 100.0).round() as i64
}

/// Convert a fraction in [0, 1] to OOXML thousandths of a percent.
#[allow(
    clippy::cast_possible_truncation,
    reason = "fractions are clamped to [0, 1] before conversion"
)]
pub(crate) fn per_mille_percent(fraction: f64) -> i64 {
    (fraction.clamp(0.0, 1.0) * 100_000.0).round() as i64
}

/// Format an angle in degrees as OOXML 60,000ths of a degree in [0, 360).
#[allow(
    clippy::cast_possible_truncation,
    reason = "the angle is reduced into [0, 360) before conversion"
)]
pub(crate) fn angle(degrees: f32) -> i64 {
    (f64::from(degrees).rem_euclid(360.0) * 60_000.0).round() as i64
}
