//! Native tests: render decks and check the package is well-formed.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::trivially_copy_pass_by_ref,
    reason = "tests may panic on unexpected input"
)]

use crate::types::{
    Arrowheads, Background, BarChart, BigNumberLayout, Border, BulletsLayout, Chart, ChartBox,
    ChartLayout, CodeBlock, CodeLayout, ColumnsLayout, ComparisonLayout, Connector, Dash, Deck,
    Frame, Geometry, Gradient, ImageFit, ImageLayout, Layout, LineChart, ListBox, Picture,
    PieChart, ProcessLayout, QuoteLayout, SectionLayout, Series, Shape, ShapeBox, Slide, Stat,
    StatBox, StatsLayout, Step, Table, TableLayout, TextAlign, TextBox, TextLayout, TextStyle,
    Theme, TitleLayout, Transition, VerticalAlign,
};
use crate::{image, markdown, package};
use std::collections::BTreeMap;
use std::io::{Cursor, Read};

const ALL_THEMES: [Theme; 7] = [
    Theme::CorporateBlue,
    Theme::DarkGradient,
    Theme::LightClean,
    Theme::Emerald,
    Theme::Sunset,
    Theme::Black,
    Theme::Brutalist,
];

// ── Helpers ──────────────────────────────────────────────────────────────

/// Encode a tiny RGB PNG of the given size.
fn png(width: u32, height: u32) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(kind);
        hasher.update(data);
        out.extend_from_slice(&hasher.finalize().to_be_bytes());
    }
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut raw = Vec::new();
    for y in 0..height {
        raw.push(0);
        for x in 0..width {
            raw.extend_from_slice(&[(x * 60) as u8, (y * 60) as u8, 0x99]);
        }
    }
    let idat = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn frame(x: f32, y: f32, width: f32, height: f32) -> Frame {
    Frame {
        x,
        y,
        width,
        height,
    }
}

fn style() -> TextStyle {
    TextStyle {
        font_size: None,
        color: None,
        bold: false,
        italic: false,
        font_family: None,
        align: None,
    }
}

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
        theme: Theme::CorporateBlue,
        title: Some("Test deck".to_owned()),
        author: Some("Tester".to_owned()),
        slides,
        slide_numbers: true,
        footer: Some("Footer & co".to_owned()),
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn bar() -> Chart {
    Chart::Bar(BarChart {
        title: Some("Revenue".to_owned()),
        categories: strings(&["Q1", "Q2", "Q3"]),
        series: vec![
            Series {
                name: "2024".to_owned(),
                values: vec![1.0, 2.5, 3.0],
                color: None,
            },
            Series {
                name: "2025".to_owned(),
                values: vec![2.0, 3.5, 4.0],
                color: Some("#FF0000".to_owned()),
            },
        ],
        horizontal: false,
        stacked: false,
        show_values: true,
        show_legend: true,
    })
}

/// An unpacked package: part name to contents.
type Parts = BTreeMap<String, Vec<u8>>;

fn unzip(bytes: &[u8]) -> Parts {
    let mut archive = ::zip::ZipArchive::new(Cursor::new(bytes)).expect("valid zip");
    let mut parts = Parts::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).unwrap();
        let mut data = Vec::new();
        file.read_to_end(&mut data).unwrap();
        parts.insert(file.name().to_owned(), data);
    }
    parts
}

/// Check every XML part parses and every relationship target exists.
fn check_package(bytes: &[u8]) -> Parts {
    assert!(bytes.starts_with(b"PK\x03\x04"));
    let parts = unzip(bytes);
    let names: Vec<_> = parts.keys().cloned().collect();
    assert_eq!(
        names.iter().filter(|n| *n == "[Content_Types].xml").count(),
        1
    );
    let content_types = text(&parts, "[Content_Types].xml");

    for (name, data) in &parts {
        let extension = name.rsplit('.').next().unwrap();
        if name.ends_with(".xml") || name.ends_with(".rels") {
            parse_xml(name, data);
        }
        if name.ends_with(".xml") && name != "[Content_Types].xml" {
            assert!(
                content_types.contains(&format!("PartName=\"/{name}\"")),
                "{name} missing from content types"
            );
        } else if !name.ends_with(".rels") && name != "[Content_Types].xml" {
            assert!(
                content_types.contains(&format!("Extension=\"{extension}\"")),
                "{name} has no default content type"
            );
        }
        if name.ends_with(".rels") {
            let base = rels_base(name);
            for target in attribute_values(&String::from_utf8_lossy(data), "Target") {
                let resolved = resolve(&base, &target);
                assert!(
                    parts.contains_key(&resolved),
                    "{name} points at missing part {resolved}; parts: {names:?}"
                );
            }
        }
    }
    for part in attribute_values(&content_types, "PartName") {
        assert!(
            parts.contains_key(part.trim_start_matches('/')),
            "stray override {part}"
        );
    }
    parts
}

fn parse_xml(name: &str, data: &[u8]) {
    let mut reader = quick_xml::Reader::from_reader(data);
    let mut buf = Vec::new();
    let mut depth = 0i32;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(quick_xml::events::Event::Start(_)) => depth += 1,
            Ok(quick_xml::events::Event::End(_)) => depth -= 1,
            Ok(_) => {}
            Err(e) => panic!("{name} is not well-formed XML: {e}"),
        }
        buf.clear();
    }
    assert_eq!(depth, 0, "{name} has unbalanced tags");
}

fn attribute_values(xml: &str, attribute: &str) -> Vec<String> {
    let needle = format!(" {attribute}=\"");
    xml.match_indices(&needle)
        .map(|(i, _)| {
            let rest = &xml[i + needle.len()..];
            rest[..rest.find('"').unwrap()].to_owned()
        })
        .collect()
}

/// The directory a `.rels` file's targets are relative to.
fn rels_base(name: &str) -> String {
    let dir = name.trim_end_matches(|c| c != '/').trim_end_matches('/');
    dir.trim_end_matches("_rels")
        .trim_end_matches('/')
        .to_owned()
}

fn resolve(base: &str, target: &str) -> String {
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for segment in target.split('/') {
        match segment {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn text(parts: &Parts, name: &str) -> String {
    String::from_utf8(
        parts
            .get(name)
            .unwrap_or_else(|| panic!("missing {name}"))
            .clone(),
    )
    .unwrap()
}

fn every_layout() -> Vec<Slide> {
    vec![
        slide(Layout::Title(TitleLayout {
            title: "Hello <world> & \"friends\"".to_owned(),
            subtitle: Some("A subtitle".to_owned()),
        })),
        slide(Layout::Section(SectionLayout {
            title: "Part one".to_owned(),
            subtitle: None,
        })),
        slide(Layout::Text(TextLayout {
            title: "Text".to_owned(),
            paragraphs: strings(&["First paragraph.", "Second\nwith a break."]),
        })),
        slide(Layout::Bullets(BulletsLayout {
            title: "Bullets".to_owned(),
            items: strings(&["One", "Two", "Three"]),
        })),
        slide(Layout::Columns(ColumnsLayout {
            title: "Columns".to_owned(),
            left: strings(&["L1", "L2"]),
            right: strings(&["R1"]),
        })),
        slide(Layout::Comparison(ComparisonLayout {
            title: "Compare".to_owned(),
            left_heading: "Before".to_owned(),
            left: strings(&["Slow"]),
            right_heading: "After".to_owned(),
            right: strings(&["Fast"]),
        })),
        slide(Layout::Quote(QuoteLayout {
            quote: "Simplicity is prerequisite for reliability.".to_owned(),
            author: Some("Edsger Dijkstra".to_owned()),
            role: Some("Computer scientist".to_owned()),
        })),
        slide(Layout::BigNumber(BigNumberLayout {
            number: "2.6".to_owned(),
            unit: Some("SECONDS".to_owned()),
            label: Some("Median build time".to_owned()),
        })),
        slide(Layout::Stats(StatsLayout {
            title: Some("Stats".to_owned()),
            stats: vec![
                Stat {
                    value: "99.9%".to_owned(),
                    label: "Uptime".to_owned(),
                },
                Stat {
                    value: "12ms".to_owned(),
                    label: "Latency".to_owned(),
                },
            ],
        })),
        slide(Layout::Chart(ChartLayout {
            title: "Chart".to_owned(),
            chart: bar(),
        })),
        slide(Layout::Table(TableLayout {
            title: "Table".to_owned(),
            headers: strings(&["Name", "Value"]),
            rows: vec![strings(&["a", "1"]), strings(&["b", "2"])],
        })),
        slide(Layout::Image(ImageLayout {
            title: Some("Image".to_owned()),
            data: png(4, 2),
            caption: Some("A caption".to_owned()),
        })),
        slide(Layout::Code(CodeLayout {
            title: "Code".to_owned(),
            code: "fn main() {\n    println!(\"<hi>\");\n}".to_owned(),
        })),
        slide(Layout::Process(ProcessLayout {
            title: "Process".to_owned(),
            steps: vec![
                Step {
                    label: "Plan".to_owned(),
                    description: Some("Decide what to build".to_owned()),
                },
                Step {
                    label: "Build".to_owned(),
                    description: None,
                },
                Step {
                    label: "Ship".to_owned(),
                    description: Some("Release it".to_owned()),
                },
            ],
        })),
        slide(Layout::Blank),
    ]
}

fn every_shape() -> Vec<Shape> {
    vec![
        Shape::Text(TextBox {
            frame: frame(0.5, 0.5, 4.0, 1.0),
            paragraphs: strings(&["Text box"]),
            style: TextStyle {
                font_size: Some(20.0),
                color: Some("112233".to_owned()),
                bold: true,
                italic: true,
                font_family: Some("Georgia".to_owned()),
                align: Some(TextAlign::Center),
            },
            fill: Some("#EEEEEE".to_owned()),
            vertical_align: Some(VerticalAlign::Middle),
        }),
        Shape::Shape(ShapeBox {
            frame: frame(5.0, 0.5, 2.0, 1.0),
            geometry: Geometry::Hexagon,
            fill: None,
            opacity: Some(0.5),
            border: Some(Border {
                color: "000000".to_owned(),
                width: 2.0,
            }),
            text: Some("Hex".to_owned()),
            style: style(),
        }),
        Shape::Line(Connector {
            x1: 8.0,
            y1: 1.5,
            x2: 7.0,
            y2: 0.5,
            color: None,
            width: None,
            dash: Dash::Dot,
            arrowheads: Arrowheads::Both,
        }),
        Shape::List(ListBox {
            frame: frame(0.5, 2.0, 4.0, 2.0),
            items: strings(&["a", "b"]),
            numbered: true,
            style: style(),
            bullet_color: Some("FF00FF".to_owned()),
        }),
        Shape::Picture(Picture {
            frame: frame(5.0, 2.0, 2.0, 2.0),
            data: png(3, 1),
            fit: ImageFit::Cover,
            description: Some("Gradient".to_owned()),
        }),
        Shape::Table(Table {
            frame: frame(7.5, 2.0, 5.0, 1.5),
            headers: strings(&["A", "B"]),
            rows: vec![strings(&["1", "2"])],
            header_fill: Some("333333".to_owned()),
            font_size: Some(12.0),
        }),
        Shape::Chart(ChartBox {
            frame: frame(0.5, 4.5, 4.0, 2.4),
            chart: Chart::Pie(PieChart {
                title: None,
                labels: strings(&["x", "y"]),
                values: vec![1.0, 3.0],
                colors: Vec::new(),
                donut: true,
                show_percent: true,
                show_legend: true,
            }),
        }),
        Shape::Code(CodeBlock {
            frame: frame(5.0, 4.5, 4.0, 2.4),
            code: "let x = 1;\nlet y = 2;".to_owned(),
            font_size: None,
            line_numbers: true,
        }),
        Shape::Stat(StatBox {
            frame: frame(9.5, 4.5, 3.0, 2.4),
            value: "42".to_owned(),
            label: "Answer".to_owned(),
            fill: Some("2196F3".to_owned()),
        }),
    ]
}

fn error_of(deck: &Deck) -> String {
    package::build(deck).unwrap_err()
}

// ── Tests ────────────────────────────────────────────────────────────────

#[test]
fn every_layout_in_every_theme() {
    for theme in ALL_THEMES {
        let mut deck = deck(every_layout());
        deck.theme = theme;
        let bytes = package::build(&deck).unwrap();
        let parts = check_package(&bytes);
        assert!(parts.contains_key("ppt/slides/slide15.xml"));
        assert!(parts.contains_key("ppt/charts/chart1.xml"));
        assert!(parts.contains_key("ppt/media/image1.png"));
        assert!(!parts.contains_key("ppt/notesMasters/notesMaster1.xml"));
    }
}

#[test]
fn every_shape_with_notes_and_transitions() {
    let mut first = slide(Layout::Blank);
    first.shapes = every_shape();
    first.notes = Some("Speaker notes\n\nwith <markup> & paragraphs".to_owned());
    first.transition = Some(Transition::Fade);
    first.background = Some(Background::Gradient(Gradient {
        start: "000000".to_owned(),
        end: "#334455".to_owned(),
        angle: 90.0,
    }));
    let mut second = slide(Layout::Bullets(BulletsLayout {
        title: "Second".to_owned(),
        items: strings(&["x"]),
    }));
    second.transition = Some(Transition::Zoom);
    let mut third = slide(Layout::Blank);
    third.notes = Some("More notes".to_owned());
    third.background = Some(Background::Solid("FFFFFF".to_owned()));
    let bytes = package::build(&deck(vec![first, second, third])).unwrap();
    let parts = check_package(&bytes);

    assert!(parts.contains_key("ppt/notesMasters/notesMaster1.xml"));
    assert!(parts.contains_key("ppt/notesSlides/notesSlide2.xml"));
    assert!(!parts.contains_key("ppt/notesSlides/notesSlide3.xml"));
    let notes = text(&parts, "ppt/notesSlides/notesSlide2.xml");
    assert!(notes.contains("More notes"));
    let notes_rels = text(&parts, "ppt/notesSlides/_rels/notesSlide2.xml.rels");
    assert!(notes_rels.contains("../slides/slide3.xml"));

    let slide = text(&parts, "ppt/slides/slide1.xml");
    assert!(slide.contains("<p:fade/>"));
    assert!(text(&parts, "ppt/notesSlides/notesSlide1.xml").contains("&lt;markup&gt; &amp;"));
    assert!(slide.contains("Footer &amp; co"));
    let rels = text(&parts, "ppt/slides/_rels/slide1.xml.rels");
    assert!(rels.contains("notesSlide1.xml"));
    assert!(rels.contains("../media/image1.png"));
    assert!(rels.contains("../charts/chart1.xml"));
    let mut ids = attribute_values(&rels, "Id");
    let count = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), count, "relationship ids must be unique");
}

#[test]
fn deterministic_output() {
    let a = package::build(&deck(every_layout())).unwrap();
    let b = package::build(&deck(every_layout())).unwrap();
    assert_eq!(a, b);
}

#[test]
fn charts_of_every_kind() {
    let line = |area: bool| {
        Chart::Line(LineChart {
            title: None,
            categories: strings(&["a", "b"]),
            series: vec![Series {
                name: "s".to_owned(),
                values: vec![1.0, -2.0],
                color: None,
            }],
            smooth: true,
            area,
            show_values: false,
            show_legend: false,
        })
    };
    let mut stacked = bar();
    if let Chart::Bar(bar) = &mut stacked {
        bar.stacked = true;
        bar.horizontal = true;
    }
    let pie = Chart::Pie(PieChart {
        title: Some("Pie".to_owned()),
        labels: strings(&["a", "b", "c"]),
        values: vec![1.0, 0.0, 2.0],
        colors: strings(&["FF0000", "00FF00"]),
        donut: false,
        show_percent: true,
        show_legend: true,
    });
    let slides = [line(false), line(true), stacked, pie]
        .into_iter()
        .map(|chart| {
            slide(Layout::Chart(ChartLayout {
                title: "c".to_owned(),
                chart,
            }))
        })
        .collect();
    let parts = check_package(&package::build(&deck(slides)).unwrap());
    assert!(text(&parts, "ppt/charts/chart1.xml").contains("<c:lineChart>"));
    assert!(text(&parts, "ppt/charts/chart2.xml").contains("<c:areaChart>"));
    let stacked = text(&parts, "ppt/charts/chart3.xml");
    assert!(stacked.contains("<c:barDir val=\"bar\"/>"));
    assert!(stacked.contains("<c:overlap val=\"100\"/>"));
    assert!(text(&parts, "ppt/charts/chart4.xml").contains("<c:pieChart>"));
}

#[test]
fn errors() {
    assert!(error_of(&deck(Vec::new())).contains("at least one slide"));

    let mut bad_color = slide(Layout::Blank);
    bad_color.background = Some(Background::Solid("nope".to_owned()));
    let message = error_of(&deck(vec![slide(Layout::Blank), bad_color]));
    assert!(message.starts_with("slide 2:"), "{message}");

    let mut off_slide = slide(Layout::Blank);
    off_slide.shapes = vec![Shape::Text(TextBox {
        frame: frame(12.0, 0.0, 4.0, 1.0),
        paragraphs: Vec::new(),
        style: style(),
        fill: None,
        vertical_align: None,
    })];
    let message = error_of(&deck(vec![off_slide]));
    assert!(message.starts_with("slide 1: shape 1 (text):"), "{message}");

    let mut mismatched = bar();
    if let Chart::Bar(bar) = &mut mismatched {
        bar.series[0].values.pop();
    }
    let message = error_of(&deck(vec![slide(Layout::Chart(ChartLayout {
        title: "c".to_owned(),
        chart: mismatched,
    }))]));
    assert!(
        message.contains("values but there are 3 categories"),
        "{message}"
    );

    let message = error_of(&deck(vec![slide(Layout::Image(ImageLayout {
        title: None,
        data: b"not an image".to_vec(),
        caption: None,
    }))]));
    assert!(message.contains("not a PNG"), "{message}");

    let message = error_of(&deck(vec![slide(Layout::Stats(StatsLayout {
        title: None,
        stats: Vec::new(),
    }))]));
    assert!(message.contains("1 to 4 stats"), "{message}");

    let message = error_of(&deck(vec![slide(Layout::Table(TableLayout {
        title: "t".to_owned(),
        headers: strings(&["a", "b"]),
        rows: vec![strings(&["1"])],
    }))]));
    assert!(message.contains("row 1 has 1 cells"), "{message}");

    let message = error_of(&deck(vec![slide(Layout::Chart(ChartLayout {
        title: "c".to_owned(),
        chart: Chart::Pie(PieChart {
            title: None,
            labels: strings(&["a"]),
            values: vec![f64::NAN],
            colors: Vec::new(),
            donut: false,
            show_percent: false,
            show_legend: false,
        }),
    }))]));
    assert!(message.contains("finite"), "{message}");
}

#[test]
fn image_sizes() {
    let info = image::inspect(&png(7, 3)).unwrap();
    assert_eq!(info.format, image::ImageFormat::Png);
    assert_eq!(info.size, Some((7, 3)));

    let gif = b"GIF89a\x05\x00\x02\x00rest";
    assert_eq!(image::inspect(gif).unwrap().size, Some((5, 2)));

    // SOI, an APP0 segment, then SOF0 with height 2 and width 9.
    let jpeg = [
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x02,
        0x00, 0x09, 0x01, 0x01, 0x11, 0x00,
    ];
    let info = image::inspect(&jpeg).unwrap();
    assert_eq!(info.format, image::ImageFormat::Jpeg);
    assert_eq!(info.size, Some((9, 2)));

    assert!(image::inspect(b"BM").is_err());
}

#[test]
fn markdown_layouts() {
    let source = "\
# My talk

A subtitle

## Agenda

- One
- Two
  - Nested
- Three

<!-- remember to smile -->

## Story

Some text with *emphasis* and `code`.

---

> Programs must be written for people to read.
>
> — Harold Abelson

## Code

```rust
fn main() {}
```

## Data

| a | b |
|---|---|
| 1 | 2 |

# Part two

## Two lists

- a
- b

1. c
2. d
";
    let deck = markdown::parse(source, Theme::LightClean).unwrap();
    let layouts: Vec<_> = deck.slides.iter().map(|s| &s.layout).collect();
    assert_eq!(deck.title.as_deref(), Some("My talk"));
    assert!(matches!(layouts[0], Layout::Title(t) if t.subtitle.as_deref() == Some("A subtitle")));
    assert!(
        matches!(layouts[1], Layout::Bullets(b) if b.items == strings(&["One", "Two", "Nested", "Three"])),
        "{:?}",
        layouts[1]
    );
    assert_eq!(deck.slides[1].notes.as_deref(), Some("remember to smile"));
    assert!(
        matches!(layouts[2], Layout::Text(t) if t.paragraphs == strings(&["Some text with emphasis and code."]))
    );
    assert!(
        matches!(layouts[3], Layout::Quote(q) if q.author.as_deref() == Some("Harold Abelson")),
        "{:?}",
        layouts[3]
    );
    assert!(matches!(layouts[4], Layout::Code(c) if c.code == "fn main() {}"));
    assert!(matches!(layouts[5], Layout::Table(t) if t.rows == vec![strings(&["1", "2"])]));
    assert!(matches!(layouts[6], Layout::Section(s) if s.title == "Part two"));
    assert!(matches!(layouts[7], Layout::Columns(_)));
    assert_eq!(layouts.len(), 8, "{layouts:#?}");

    check_package(&package::build(&deck).unwrap());
}

#[test]
fn markdown_errors_when_empty() {
    assert!(markdown::parse("", Theme::Black).is_err());
    assert!(markdown::parse("---\n\n---", Theme::Black).is_err());
}

/// Write sample decks for manual inspection when `SLIDEDECK_SAMPLES` names
/// a directory.
#[test]
fn write_samples() {
    let Ok(dir) = std::env::var("SLIDEDECK_SAMPLES") else {
        return;
    };
    for theme in ALL_THEMES {
        let mut deck = deck(every_layout());
        deck.theme = theme;
        let mut extra = slide(Layout::Blank);
        extra.shapes = every_shape();
        extra.notes = Some("Notes".to_owned());
        deck.slides.push(extra);
        let bytes = package::build(&deck).unwrap();
        let name = format!("{theme:?}").replace("Theme::", "");
        std::fs::write(format!("{dir}/{name}.pptx"), bytes).unwrap();
    }
}
