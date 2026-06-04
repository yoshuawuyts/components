# color

A Wasm Component that converts between CSS color spaces.

Conversions are exposed through a single `color` resource: one `from-*` static
constructor per input space and one `to-*` method per output space. This yields
`M + N` entry points but `M × N` conversion paths.

Supported spaces: hex strings, sRGB, HSL, HWB, CIE L\*a\*b\*, CIE LCH, Oklab,
Oklch, and Okhsl, each carrying a straight alpha channel.

Internally every color is stored as CIE XYZ (D65 white point), an unbounded,
device-independent hub. Converting through it is (near) lossless for every
space, including wide-gamut colors outside the sRGB cube. The bounded display
spaces (`srgb`, `hsl`, `hwb`, hex) are clamped into the sRGB gamut on output;
the perceptual spaces (`lab`, `lch`, `oklab`, `oklch`, `okhsl`) are returned
unclamped.

```wit
let c = color.from-hex("#ff8800")?;
let oklch = c.to-oklch();
```
