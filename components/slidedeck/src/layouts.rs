//! Slide templates: each layout expands into a background and shapes.

use crate::shapes::validate_frame;
use crate::theme::{Palette, Rgb};
use crate::types::{
    Arrowheads, Background, BigNumberLayout, ChartBox, CodeBlock, ColumnsLayout, ComparisonLayout,
    Connector, Dash, Frame, Geometry, ImageFit, ImageLayout, Layout, ListBox, Picture,
    ProcessLayout, QuoteLayout, Shape, ShapeBox, StatBox, StatsLayout, Table, TextAlign, TextBox,
    TextStyle, VerticalAlign,
};
use crate::xml::{SLIDE_HEIGHT, SLIDE_WIDTH};

/// Left and right slide margin.
const MARGIN: f32 = 0.5;

/// Width of the content area between the margins.
const CONTENT_WIDTH: f32 = SLIDE_WIDTH - 2.0 * MARGIN;

/// Top of the body area below a slide title.
const BODY_TOP: f32 = 1.5;

/// Bottom of the body area, leaving room for footers.
const BODY_BOTTOM: f32 = 6.8;

/// Most stats a stats layout may hold.
const MAX_STATS: usize = 4;

/// Fewest and most steps a process layout may hold.
const PROCESS_STEPS: std::ops::RangeInclusive<usize> = 2..=6;

/// A resolved slide background.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Fill {
    Solid(Rgb),
    Gradient { start: Rgb, end: Rgb, angle: f32 },
}

impl Fill {
    /// Parse a user-supplied background.
    pub(crate) fn parse(background: &Background) -> Result<Self, String> {
        match background {
            Background::Solid(color) => Ok(Self::Solid(Rgb::parse(color, "background")?)),
            Background::Gradient(gradient) => {
                if !gradient.angle.is_finite() {
                    return Err("background gradient angle must be a finite number".to_owned());
                }
                Ok(Self::Gradient {
                    start: Rgb::parse(&gradient.start, "background gradient start")?,
                    end: Rgb::parse(&gradient.end, "background gradient end")?,
                    angle: gradient.angle,
                })
            }
        }
    }

    /// A single color representative of the fill, for picking text colors.
    pub(crate) fn average(self) -> Rgb {
        match self {
            Self::Solid(color) => color,
            Self::Gradient { start, end, .. } => start.mix(end, 0.5),
        }
    }
}

/// The background a layout uses unless the slide overrides it.
pub(crate) fn default_background(layout: &Layout, palette: &Palette) -> Fill {
    match layout {
        Layout::Title(_) => Fill::Gradient {
            start: palette.background,
            end: palette.background.mix(palette.accent(), 0.35),
            angle: 45.0,
        },
        Layout::Section(_) => Fill::Solid(palette.accent()),
        _ => Fill::Solid(palette.background),
    }
}

/// Expand a layout into shapes drawn on a slide whose background averages
/// to `background`.
pub(crate) fn expand(
    layout: &Layout,
    palette: &Palette,
    background: Rgb,
) -> Result<Vec<Shape>, String> {
    let mut out = Vec::new();
    match layout {
        Layout::Title(title) => {
            cover(
                &mut out,
                palette,
                background,
                &title.title,
                title.subtitle.as_deref(),
                60.0,
                true,
            );
        }
        Layout::Section(section) => {
            cover(
                &mut out,
                palette,
                background,
                &section.title,
                section.subtitle.as_deref(),
                48.0,
                false,
            );
        }
        Layout::Text(text) => {
            heading(&mut out, palette, &text.title);
            let frame = body();
            let size = fit_font_size(&text.paragraphs, &frame, 24.0, 12.0, 0.6);
            out.push(Shape::Text(TextBox {
                frame,
                paragraphs: text.paragraphs.clone(),
                style: TextStyle {
                    font_size: Some(size),
                    ..style()
                },
                fill: None,
                vertical_align: None,
            }));
        }
        Layout::Bullets(bullets) => {
            heading(&mut out, palette, &bullets.title);
            out.push(list(body(), &bullets.items, 24.0));
        }
        Layout::Columns(columns) => columns_layout(&mut out, palette, columns),
        Layout::Comparison(comparison) => {
            comparison_layout(&mut out, palette, background, comparison);
        }
        Layout::Quote(quote) => quote_layout(&mut out, palette, background, quote),
        Layout::BigNumber(number) => big_number_layout(&mut out, palette, background, number),
        Layout::Stats(stats) => stats_layout(&mut out, palette, background, stats)?,
        Layout::Chart(chart) => {
            heading(&mut out, palette, &chart.title);
            out.push(Shape::Chart(ChartBox {
                frame: body(),
                chart: chart.chart.clone(),
            }));
        }
        Layout::Table(table) => {
            heading(&mut out, palette, &table.title);
            #[allow(clippy::cast_precision_loss, reason = "row counts are small")]
            let rows = (table.rows.len() + 1) as f32;
            let height = (rows * 0.55).min(BODY_BOTTOM - BODY_TOP);
            let font_size = if rows * 0.55 > BODY_BOTTOM - BODY_TOP {
                ((BODY_BOTTOM - BODY_TOP) / rows * 72.0 * 0.45).clamp(8.0, 14.0)
            } else {
                16.0
            };
            out.push(Shape::Table(Table {
                frame: Frame {
                    x: MARGIN,
                    y: BODY_TOP,
                    width: CONTENT_WIDTH,
                    height,
                },
                headers: table.headers.clone(),
                rows: table.rows.clone(),
                header_fill: None,
                font_size: Some(font_size),
            }));
        }
        Layout::Image(image) => image_layout(&mut out, palette, background, image),
        Layout::Code(code) => {
            heading(&mut out, palette, &code.title);
            let lines = code.code.lines().count().max(1);
            #[allow(clippy::cast_precision_loss, reason = "line counts are small")]
            let fit = (BODY_BOTTOM - BODY_TOP - 0.4) / lines as f32 * 72.0 / 1.2;
            out.push(Shape::Code(CodeBlock {
                frame: body(),
                code: code.code.clone(),
                font_size: Some(fit.clamp(8.0, 18.0).floor()),
                line_numbers: false,
            }));
        }
        Layout::Process(process) => process_layout(&mut out, palette, background, process)?,
        Layout::Blank => {}
    }
    for shape in &out {
        if let Some(frame) = frame_of(shape) {
            validate_frame(frame).map_err(|e| format!("layout produced an invalid frame: {e}"))?;
        }
    }
    Ok(out)
}

// ── Building blocks ──────────────────────────────────────────────────────

fn frame_of(shape: &Shape) -> Option<&Frame> {
    match shape {
        Shape::Text(s) => Some(&s.frame),
        Shape::Shape(s) => Some(&s.frame),
        Shape::List(s) => Some(&s.frame),
        Shape::Picture(s) => Some(&s.frame),
        Shape::Table(s) => Some(&s.frame),
        Shape::Chart(s) => Some(&s.frame),
        Shape::Code(s) => Some(&s.frame),
        Shape::Stat(s) => Some(&s.frame),
        Shape::Line(_) => None,
    }
}

/// The default text style: theme font, automatic color.
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

fn body() -> Frame {
    Frame {
        x: MARGIN,
        y: BODY_TOP,
        width: CONTENT_WIDTH,
        height: BODY_BOTTOM - BODY_TOP,
    }
}

fn text(frame: Frame, paragraphs: Vec<String>, style: TextStyle, anchor: VerticalAlign) -> Shape {
    Shape::Text(TextBox {
        frame,
        paragraphs,
        style,
        fill: None,
        vertical_align: Some(anchor),
    })
}

fn rectangle(frame: Frame, geometry: Geometry, fill: Rgb, opacity: Option<f32>) -> Shape {
    Shape::Shape(ShapeBox {
        frame,
        geometry,
        fill: Some(fill.hex()),
        opacity,
        border: None,
        text: None,
        style: style(),
    })
}

fn list(frame: Frame, items: &[String], max: f32) -> Shape {
    let size = fit_font_size(items, &frame, max, 12.0, 0.9);
    Shape::List(ListBox {
        frame,
        items: items.to_vec(),
        numbered: false,
        style: TextStyle {
            font_size: Some(size),
            ..style()
        },
        bullet_color: None,
    })
}

/// A slide title with an accent bar underneath.
fn heading(out: &mut Vec<Shape>, palette: &Palette, title: &str) {
    let frame = Frame {
        x: MARGIN,
        y: 0.3,
        width: CONTENT_WIDTH,
        height: 0.9,
    };
    let size = fit_font_size(&[title.to_owned()], &frame, 32.0, 18.0, 0.0);
    out.push(text(
        frame,
        vec![title.to_owned()],
        TextStyle {
            font_size: Some(size),
            bold: true,
            font_family: Some(palette.title_font.to_owned()),
            ..style()
        },
        VerticalAlign::Bottom,
    ));
    out.push(rectangle(
        Frame {
            x: MARGIN,
            y: 1.2,
            width: 1.5,
            height: 0.06,
        },
        Geometry::Rectangle,
        palette.accent(),
        None,
    ));
}

/// Estimate the largest font size, stepping down from `max` to `min`, at
/// which `paragraphs` fit inside `frame`. `gap` is the space between
/// paragraphs, in lines.
pub(crate) fn fit_font_size(
    paragraphs: &[String],
    frame: &Frame,
    max: f32,
    min: f32,
    gap: f32,
) -> f32 {
    let width = (frame.width - 0.2).max(0.1);
    let height = (frame.height - 0.1).max(0.1);
    let mut size = max;
    while size > min {
        let char_width = size * 0.5 / 72.0;
        let per_line = (width / char_width).floor().max(1.0);
        let mut lines = 0.0;
        for paragraph in paragraphs {
            for line in paragraph.split('\n') {
                #[allow(clippy::cast_precision_loss, reason = "text lengths are small")]
                let chars = line.chars().count().max(1) as f32;
                lines += (chars / per_line).ceil();
            }
            lines += gap;
        }
        if lines * size * 1.2 / 72.0 <= height {
            return size;
        }
        size -= 2.0;
    }
    min
}

// ── Layouts ──────────────────────────────────────────────────────────────

/// A centered title over a subtitle, for title and section slides.
fn cover(
    out: &mut Vec<Shape>,
    palette: &Palette,
    background: Rgb,
    title: &str,
    subtitle: Option<&str>,
    max: f32,
    accent_bar: bool,
) {
    let frame = Frame {
        x: 1.0,
        y: 1.6,
        width: SLIDE_WIDTH - 2.0,
        height: 2.4,
    };
    let size = fit_font_size(&[title.to_owned()], &frame, max, 24.0, 0.0);
    out.push(text(
        frame,
        vec![title.to_owned()],
        TextStyle {
            font_size: Some(size),
            bold: true,
            font_family: Some(palette.title_font.to_owned()),
            align: Some(TextAlign::Center),
            ..style()
        },
        VerticalAlign::Bottom,
    ));
    if accent_bar {
        out.push(rectangle(
            Frame {
                x: (SLIDE_WIDTH - 1.5) / 2.0,
                y: 4.2,
                width: 1.5,
                height: 0.06,
            },
            Geometry::Rectangle,
            palette.accent(),
            None,
        ));
    }
    if let Some(subtitle) = subtitle {
        let frame = Frame {
            x: 1.0,
            y: 4.45,
            width: SLIDE_WIDTH - 2.0,
            height: 1.4,
        };
        let size = fit_font_size(&[subtitle.to_owned()], &frame, 24.0, 14.0, 0.0);
        out.push(text(
            frame,
            vec![subtitle.to_owned()],
            TextStyle {
                font_size: Some(size),
                color: Some(palette.muted_on(background).hex()),
                align: Some(TextAlign::Center),
                ..style()
            },
            VerticalAlign::Top,
        ));
    }
}

fn columns_layout(out: &mut Vec<Shape>, palette: &Palette, columns: &ColumnsLayout) {
    heading(out, palette, &columns.title);
    let width = (CONTENT_WIDTH - 0.5) / 2.0;
    for (i, items) in [&columns.left, &columns.right].into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss, reason = "i is 0 or 1")]
        let x = MARGIN + i as f32 * (width + 0.5);
        let frame = Frame {
            x,
            y: BODY_TOP,
            width,
            height: BODY_BOTTOM - BODY_TOP,
        };
        out.push(list(frame, items, 22.0));
    }
}

fn comparison_layout(
    out: &mut Vec<Shape>,
    palette: &Palette,
    background: Rgb,
    comparison: &ComparisonLayout,
) {
    heading(out, palette, &comparison.title);
    let width = (CONTENT_WIDTH - 0.5) / 2.0;
    let sides = [
        (&comparison.left_heading, &comparison.left, palette.accent()),
        (
            &comparison.right_heading,
            &comparison.right,
            palette.series(1),
        ),
    ];
    for (i, (title, items, color)) in sides.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss, reason = "i is 0 or 1")]
        let x = MARGIN + i as f32 * (width + 0.5);
        out.push(rectangle(
            Frame {
                x,
                y: BODY_TOP,
                width,
                height: BODY_BOTTOM - BODY_TOP,
            },
            Geometry::RoundedRectangle,
            background.mix(color, 0.08),
            None,
        ));
        out.push(Shape::Shape(ShapeBox {
            frame: Frame {
                x,
                y: BODY_TOP,
                width,
                height: 0.7,
            },
            geometry: Geometry::Rectangle,
            fill: Some(color.hex()),
            opacity: None,
            border: None,
            text: Some(title.clone()),
            style: TextStyle {
                font_size: Some(20.0),
                bold: true,
                ..style()
            },
        }));
        out.push(list(
            Frame {
                x: x + 0.2,
                y: BODY_TOP + 0.9,
                width: width - 0.4,
                height: BODY_BOTTOM - BODY_TOP - 1.0,
            },
            items,
            20.0,
        ));
    }
}

fn quote_layout(out: &mut Vec<Shape>, palette: &Palette, background: Rgb, quote: &QuoteLayout) {
    out.push(text(
        Frame {
            x: 0.8,
            y: 0.4,
            width: 2.0,
            height: 2.0,
        },
        vec!["\u{201C}".to_owned()],
        TextStyle {
            font_size: Some(160.0),
            color: Some(palette.accent().hex()),
            font_family: Some("Georgia".to_owned()),
            ..style()
        },
        VerticalAlign::Top,
    ));
    let frame = Frame {
        x: 1.5,
        y: 1.6,
        width: SLIDE_WIDTH - 3.0,
        height: 3.4,
    };
    let size = fit_font_size(std::slice::from_ref(&quote.quote), &frame, 36.0, 16.0, 0.0);
    out.push(text(
        frame,
        vec![quote.quote.clone()],
        TextStyle {
            font_size: Some(size),
            italic: true,
            ..style()
        },
        VerticalAlign::Middle,
    ));
    if let Some(author) = &quote.author {
        out.push(text(
            Frame {
                x: 1.5,
                y: 5.2,
                width: SLIDE_WIDTH - 3.0,
                height: 0.6,
            },
            vec![format!("\u{2014} {author}")],
            TextStyle {
                font_size: Some(22.0),
                color: Some(palette.accent().hex()),
                bold: true,
                ..style()
            },
            VerticalAlign::Top,
        ));
    }
    if let Some(role) = &quote.role {
        out.push(text(
            Frame {
                x: 1.5,
                y: 5.8,
                width: SLIDE_WIDTH - 3.0,
                height: 0.5,
            },
            vec![role.clone()],
            TextStyle {
                font_size: Some(16.0),
                color: Some(palette.muted_on(background).hex()),
                ..style()
            },
            VerticalAlign::Top,
        ));
    }
}

fn big_number_layout(
    out: &mut Vec<Shape>,
    palette: &Palette,
    background: Rgb,
    number: &BigNumberLayout,
) {
    let frame = Frame {
        x: MARGIN,
        y: 0.9,
        width: CONTENT_WIDTH,
        height: 3.2,
    };
    let size = fit_font_size(
        std::slice::from_ref(&number.number),
        &frame,
        180.0,
        40.0,
        0.0,
    );
    out.push(text(
        frame,
        vec![number.number.clone()],
        TextStyle {
            font_size: Some(size),
            color: Some(palette.accent().hex()),
            bold: true,
            font_family: Some(palette.title_font.to_owned()),
            align: Some(TextAlign::Center),
            ..style()
        },
        VerticalAlign::Bottom,
    ));
    if let Some(unit) = &number.unit {
        out.push(text(
            Frame {
                x: MARGIN,
                y: 4.2,
                width: CONTENT_WIDTH,
                height: 0.9,
            },
            vec![unit.clone()],
            TextStyle {
                font_size: Some(36.0),
                bold: true,
                align: Some(TextAlign::Center),
                ..style()
            },
            VerticalAlign::Top,
        ));
    }
    if let Some(label) = &number.label {
        let frame = Frame {
            x: 1.5,
            y: 5.2,
            width: SLIDE_WIDTH - 3.0,
            height: 1.3,
        };
        let size = fit_font_size(std::slice::from_ref(label), &frame, 22.0, 12.0, 0.0);
        out.push(text(
            frame,
            vec![label.clone()],
            TextStyle {
                font_size: Some(size),
                color: Some(palette.muted_on(background).hex()),
                align: Some(TextAlign::Center),
                ..style()
            },
            VerticalAlign::Top,
        ));
    }
}

fn stats_layout(
    out: &mut Vec<Shape>,
    palette: &Palette,
    background: Rgb,
    stats: &StatsLayout,
) -> Result<(), String> {
    let count = stats.stats.len();
    if !(1..=MAX_STATS).contains(&count) {
        return Err(format!(
            "stats layout needs 1 to {MAX_STATS} stats, got {count}"
        ));
    }
    let top = match &stats.title {
        Some(title) => {
            heading(out, palette, title);
            2.2
        }
        None => 1.8,
    };
    let gap = 0.4;
    #[allow(clippy::cast_precision_loss, reason = "count is at most 4")]
    let count = count as f32;
    let width = (CONTENT_WIDTH - gap * (count - 1.0)) / count;
    for (i, stat) in stats.stats.iter().enumerate() {
        #[allow(clippy::cast_precision_loss, reason = "i is at most 3")]
        let x = MARGIN + i as f32 * (width + gap);
        let frame = Frame {
            x,
            y: top,
            width,
            height: 3.2,
        };
        out.push(rectangle(
            frame,
            Geometry::RoundedRectangle,
            background.mix(palette.series(i), 0.1),
            None,
        ));
        out.push(Shape::Stat(StatBox {
            frame,
            value: stat.value.clone(),
            label: stat.label.clone(),
            fill: None,
        }));
    }
    Ok(())
}

fn image_layout(out: &mut Vec<Shape>, palette: &Palette, background: Rgb, image: &ImageLayout) {
    let mut top = MARGIN;
    if let Some(title) = &image.title {
        heading(out, palette, title);
        top = BODY_TOP;
    }
    let mut bottom = SLIDE_HEIGHT - MARGIN;
    if let Some(caption) = &image.caption {
        bottom -= 0.6;
        out.push(text(
            Frame {
                x: MARGIN,
                y: bottom + 0.05,
                width: CONTENT_WIDTH,
                height: 0.5,
            },
            vec![caption.clone()],
            TextStyle {
                font_size: Some(16.0),
                color: Some(palette.muted_on(background).hex()),
                align: Some(TextAlign::Center),
                ..style()
            },
            VerticalAlign::Top,
        ));
    }
    out.push(Shape::Picture(Picture {
        frame: Frame {
            x: MARGIN,
            y: top,
            width: CONTENT_WIDTH,
            height: bottom - top,
        },
        data: image.data.clone(),
        fit: ImageFit::Contain,
        description: image.caption.clone().or_else(|| image.title.clone()),
    }));
}

fn process_layout(
    out: &mut Vec<Shape>,
    palette: &Palette,
    background: Rgb,
    process: &ProcessLayout,
) -> Result<(), String> {
    let count = process.steps.len();
    if !PROCESS_STEPS.contains(&count) {
        return Err(format!(
            "process layout needs {} to {} steps, got {count}",
            PROCESS_STEPS.start(),
            PROCESS_STEPS.end()
        ));
    }
    heading(out, palette, &process.title);
    let gap = 0.6;
    #[allow(clippy::cast_precision_loss, reason = "count is at most 6")]
    let count = count as f32;
    let width = (CONTENT_WIDTH - gap * (count - 1.0)) / count;
    let (top, height) = (2.4, 1.3);
    for (i, step) in process.steps.iter().enumerate() {
        #[allow(clippy::cast_precision_loss, reason = "i is at most 5")]
        let x = MARGIN + i as f32 * (width + gap);
        let frame = Frame {
            x,
            y: top,
            width,
            height,
        };
        let size = fit_font_size(std::slice::from_ref(&step.label), &frame, 20.0, 11.0, 0.0);
        out.push(Shape::Shape(ShapeBox {
            frame,
            geometry: Geometry::RoundedRectangle,
            fill: Some(palette.series(i).hex()),
            opacity: None,
            border: None,
            text: Some(step.label.clone()),
            style: TextStyle {
                font_size: Some(size),
                bold: true,
                ..style()
            },
        }));
        if i + 1 < process.steps.len() {
            out.push(Shape::Line(Connector {
                x1: x + width + 0.1,
                y1: top + height / 2.0,
                x2: x + width + gap - 0.1,
                y2: top + height / 2.0,
                color: Some(palette.muted_on(background).hex()),
                width: Some(2.0),
                dash: Dash::Solid,
                arrowheads: Arrowheads::End,
            }));
        }
        if let Some(description) = &step.description {
            let frame = Frame {
                x,
                y: top + height + 0.3,
                width,
                height: BODY_BOTTOM - top - height - 0.3,
            };
            let size = fit_font_size(std::slice::from_ref(description), &frame, 16.0, 10.0, 0.0);
            out.push(text(
                frame,
                vec![description.clone()],
                TextStyle {
                    font_size: Some(size),
                    color: Some(palette.muted_on(background).hex()),
                    align: Some(TextAlign::Center),
                    ..style()
                },
                VerticalAlign::Top,
            ));
        }
    }
    Ok(())
}
