//! Test suite for the `color` component.
//!
//! `color` exposes a `color` resource with `from-*` constructors and `to-*`
//! readers. The suite drives that resource across the component boundary and
//! asserts on:
//!  * known fixed points (pure red is `#ff0000`, hue 0, full saturation),
//!  * hex parsing and round-tripping (including alpha), and
//!  * cross-space round trips that survive the XYZ storage hub.
//!
//! Two `wit_bindgen::generate!` calls combine here, mirroring the other suites:
//! the `bindings` module generates the *imports* (the `conversion` interface);
//! `wasi_test::suite!` generates the `wasi:test/tests` *export*. At composition
//! time `wac compose` wires the component's `yoshuawuyts:color/conversion`
//! export into these imports.

mod bindings {
    wit_bindgen::generate!({
        world: "imports",
        path: "wit",
        pub_export_macro: false,
    });
}

use bindings::yoshuawuyts::color::conversion::{Color, Srgb};
use wasi_test::TestContext;

/// Absolute tolerance for floating-point comparisons across the boundary.
const EPS: f32 = 1e-3;

fn close(got: f32, want: f32) -> bool {
    (got - want).abs() <= EPS
}

fn test_hex_to_srgb(ctx: &TestContext) -> Result<(), String> {
    ctx.log("#ff0000 should parse to pure red in sRGB");
    let red = Color::from_hex("#ff0000").map_err(|e| format!("{e:?}"))?;
    let srgb = red.to_srgb();
    if !close(srgb.red, 1.0) || !close(srgb.green, 0.0) || !close(srgb.blue, 0.0) {
        return Err(format!("expected (1, 0, 0), got {srgb:?}"));
    }
    if !close(red.alpha(), 1.0) {
        return Err(format!("expected opaque alpha, got {}", red.alpha()));
    }
    Ok(())
}

fn test_hex_round_trip(ctx: &TestContext) -> Result<(), String> {
    ctx.log("#3a7bd5 should round-trip through the color resource");
    let c = Color::from_hex("#3a7bd5").map_err(|e| format!("{e:?}"))?;
    let hex = c.to_hex();
    if hex != "#3a7bd5" {
        return Err(format!("expected #3a7bd5, got {hex}"));
    }
    Ok(())
}

fn test_hex_alpha_round_trip(ctx: &TestContext) -> Result<(), String> {
    ctx.log("a translucent hex value should preserve its alpha byte");
    let c = Color::from_hex("#ff000080").map_err(|e| format!("{e:?}"))?;
    if !close(c.alpha(), 0.501_960_8) {
        return Err(format!("expected alpha ~0.502, got {}", c.alpha()));
    }
    if c.to_hex() != "#ff000080" {
        return Err(format!("expected #ff000080, got {}", c.to_hex()));
    }
    Ok(())
}

fn test_red_to_hsl(ctx: &TestContext) -> Result<(), String> {
    ctx.log("pure red should be hue 0, saturation 1, lightness 0.5 in HSL");
    let red = Color::from_hex("#ff0000").map_err(|e| format!("{e:?}"))?;
    let hsl = red.to_hsl();
    if !close(hsl.hue, 0.0) || !close(hsl.saturation, 1.0) || !close(hsl.lightness, 0.5) {
        return Err(format!("unexpected HSL: {hsl:?}"));
    }
    Ok(())
}

fn test_srgb_to_oklch_round_trip(ctx: &TestContext) -> Result<(), String> {
    ctx.log("sRGB -> color -> Oklch -> color -> sRGB should preserve the color");
    let srgb = Srgb {
        red: 0.2,
        green: 0.6,
        blue: 0.9,
    };
    let a = Color::from_srgb(srgb, 1.0).map_err(|e| format!("{e:?}"))?;
    let oklch = a.to_oklch();
    let b = Color::from_oklch(oklch, 1.0).map_err(|e| format!("{e:?}"))?;
    let out = b.to_srgb();
    if !close(out.red, 0.2) || !close(out.green, 0.6) || !close(out.blue, 0.9) {
        return Err(format!("round-trip drifted: got {out:?}"));
    }
    Ok(())
}

fn test_red_to_css_lab(ctx: &TestContext) -> Result<(), String> {
    ctx.log("pure red should match the CSS Color 4 D50 Lab reference");
    let red = Color::from_hex("#ff0000").map_err(|e| format!("{e:?}"))?;
    let lab = red.to_lab();
    // CSS reference: lab(54.29 80.81 69.89).
    if (lab.lightness - 54.29).abs() > 0.2
        || (lab.a - 80.81).abs() > 0.3
        || (lab.b - 69.89).abs() > 0.3
    {
        return Err(format!("unexpected Lab: {lab:?}"));
    }
    Ok(())
}

fn test_invalid_hex_errors(ctx: &TestContext) -> Result<(), String> {
    ctx.log("a malformed hex string should return an error, not panic");
    match Color::from_hex("not-a-color") {
        Ok(_) => Err("expected an error for invalid hex".to_string()),
        Err(_) => Ok(()),
    }
}

fn test_out_of_range_alpha_errors(ctx: &TestContext) -> Result<(), String> {
    ctx.log("an alpha outside [0, 1] should be rejected");
    let srgb = Srgb {
        red: 1.0,
        green: 0.0,
        blue: 0.0,
    };
    match Color::from_srgb(srgb, 2.0) {
        Ok(_) => Err("expected an error for out-of-range alpha".to_string()),
        Err(_) => Ok(()),
    }
}

wasi_test::suite!(
    test_hex_to_srgb,
    test_hex_round_trip,
    test_hex_alpha_round_trip,
    test_red_to_hsl,
    test_red_to_css_lab,
    test_srgb_to_oklch_round_trip,
    test_invalid_hex_errors,
    test_out_of_range_alpha_errors,
);
