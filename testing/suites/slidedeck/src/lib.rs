//! Test suite for the `slidedeck` component.
//!
//! Drives `generate` and `from-markdown` across the component boundary and
//! asserts on:
//!  * the output being a ZIP archive (`PK\x03\x04`) containing the core
//!    PresentationML parts,
//!  * deterministic output, and
//!  * descriptive errors for invalid input.
//!
//! The `bindings` module generates the *imports* (the `presentation`
//! interface); `wasi_test::suite!` generates the `wasi:test/tests` *export*.

mod bindings {
    wit_bindgen::generate!({
        world: "imports",
        path: "wit",
        pub_export_macro: false,
    });
}

use bindings::yoshuawuyts::slidedeck::presentation::{from_markdown, generate};
use bindings::yoshuawuyts::slidedeck::types::{
    Background, BarChart, BulletsLayout, Chart, ChartLayout, Deck, Frame, Layout, Series, Shape,
    Slide, TextBox, TextStyle, Theme, TitleLayout,
};
use wasi_test::TestContext;

fn slide(layout: Layout) -> Slide {
    Slide {
        layout,
        shapes: Vec::new(),
        notes: None,
        background: None,
        transition: None,
    }
}

fn deck(slides: Vec<Slide>) -> Deck {
    Deck {
        theme: Theme::Emerald,
        title: Some("Suite".to_string()),
        author: None,
        slides,
        slide_numbers: true,
        footer: None,
    }
}

fn sample() -> Deck {
    let mut title = slide(Layout::Title(TitleLayout {
        title: "Hello".to_string(),
        subtitle: Some("from a component".to_string()),
    }));
    title.notes = Some("Say hi".to_string());
    deck(vec![
        title,
        slide(Layout::Bullets(BulletsLayout {
            title: "Points".to_string(),
            items: vec!["one".to_string(), "two".to_string()],
        })),
        slide(Layout::Chart(ChartLayout {
            title: "Chart".to_string(),
            chart: Chart::Bar(BarChart {
                title: None,
                categories: vec!["a".to_string(), "b".to_string()],
                series: vec![Series {
                    name: "s".to_string(),
                    values: vec![1.0, 2.0],
                    color: None,
                }],
                horizontal: false,
                stacked: false,
                show_values: false,
                show_legend: false,
            }),
        })),
    ])
}

/// Whether the ZIP archive `bytes` stores a part named `name`.
fn has_part(bytes: &[u8], name: &str) -> bool {
    bytes.windows(name.len()).any(|w| w == name.as_bytes())
}

fn test_generate_produces_pptx(ctx: &TestContext) -> Result<(), String> {
    ctx.log("generate should return a ZIP with the core presentation parts");
    let bytes = generate(&sample())?;
    if !bytes.starts_with(b"PK\x03\x04") {
        return Err("output is not a ZIP archive".to_string());
    }
    for part in [
        "[Content_Types].xml",
        "ppt/presentation.xml",
        "ppt/slides/slide3.xml",
        "ppt/charts/chart1.xml",
        "ppt/notesSlides/notesSlide1.xml",
    ] {
        if !has_part(&bytes, part) {
            return Err(format!("missing part {part}"));
        }
    }
    Ok(())
}

fn test_generate_is_deterministic(ctx: &TestContext) -> Result<(), String> {
    ctx.log("the same deck should always render to the same bytes");
    if generate(&sample())? != generate(&sample())? {
        return Err("two renders of the same deck differ".to_string());
    }
    Ok(())
}

fn test_empty_deck_errors(ctx: &TestContext) -> Result<(), String> {
    ctx.log("a deck without slides should be rejected");
    match generate(&deck(Vec::new())) {
        Ok(_) => Err("expected an error for an empty deck".to_string()),
        Err(_) => Ok(()),
    }
}

fn test_bad_color_errors(ctx: &TestContext) -> Result<(), String> {
    ctx.log("a malformed color should name the offending slide");
    let mut bad = slide(Layout::Blank);
    bad.background = Some(Background::Solid("purple-ish".to_string()));
    match generate(&deck(vec![slide(Layout::Blank), bad])) {
        Ok(_) => Err("expected an error for a malformed color".to_string()),
        Err(e) if e.starts_with("slide 2:") => Ok(()),
        Err(e) => Err(format!("unexpected error: {e}")),
    }
}

fn test_off_slide_shape_errors(ctx: &TestContext) -> Result<(), String> {
    ctx.log("a shape past the slide edge should name the slide and shape");
    let mut s = slide(Layout::Blank);
    s.shapes.push(Shape::Text(TextBox {
        frame: Frame {
            x: 10.0,
            y: 1.0,
            width: 5.0,
            height: 1.0,
        },
        paragraphs: vec!["overflow".to_string()],
        style: TextStyle {
            font_size: None,
            color: None,
            bold: false,
            italic: false,
            font_family: None,
            align: None,
        },
        fill: None,
        vertical_align: None,
    }));
    match generate(&deck(vec![s])) {
        Ok(_) => Err("expected an error for an off-slide shape".to_string()),
        Err(e) if e.starts_with("slide 1: shape 1 (text):") => Ok(()),
        Err(e) => Err(format!("unexpected error: {e}")),
    }
}

fn test_from_markdown(ctx: &TestContext) -> Result<(), String> {
    ctx.log("from-markdown should split slides on headings");
    let markdown = "# Talk\n\nSubtitle\n\n## Points\n\n- a\n- b\n\n<!-- notes -->\n\n## Code\n\n```\nfn main() {}\n```\n";
    let bytes = from_markdown(markdown, Theme::DarkGradient)?;
    if !has_part(&bytes, "ppt/slides/slide3.xml") || has_part(&bytes, "ppt/slides/slide4.xml") {
        return Err("expected exactly three slides".to_string());
    }
    if !has_part(&bytes, "ppt/notesSlides/notesSlide1.xml") {
        return Err("expected speaker notes from the HTML comment".to_string());
    }
    Ok(())
}

fn test_empty_markdown_errors(ctx: &TestContext) -> Result<(), String> {
    ctx.log("markdown without content should be rejected");
    match from_markdown("", Theme::Black) {
        Ok(_) => Err("expected an error for empty markdown".to_string()),
        Err(_) => Ok(()),
    }
}

wasi_test::suite!(
    test_generate_produces_pptx,
    test_generate_is_deterministic,
    test_empty_deck_errors,
    test_bad_color_errors,
    test_off_slide_shape_errors,
    test_from_markdown,
    test_empty_markdown_errors,
);
