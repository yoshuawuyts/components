//! Rendering individual shapes into slide XML.

use crate::charts;
use crate::image::{self, ImageFormat};
use crate::theme::{Palette, Rgb};
use crate::types::{
    Arrowheads, ChartBox, CodeBlock, Connector, Dash, Frame, Geometry, ImageFit, ListBox, Picture,
    Shape, ShapeBox, StatBox, Table, TextAlign, TextBox, TextStyle, VerticalAlign,
};
use crate::xml::{
    centipoints, emu, emu_points, escape, per_mille_percent, SLIDE_HEIGHT, SLIDE_WIDTH,
};
use std::fmt::Write as _;

/// Relationship type of an image part.
const REL_IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
/// Relationship type of a chart part.
const REL_CHART: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";

/// How far (in inches) a frame may poke past the slide edge before it's
/// rejected. Absorbs rounding in callers' layout arithmetic.
const BOUNDS_TOLERANCE: f32 = 0.01;

/// Monospaced font used for code.
pub(crate) const CODE_FONT: &str = "Consolas";

/// Background of code panels.
pub(crate) const CODE_BACKGROUND: Rgb = Rgb::new(0x16, 0x1B, 0x22);

/// Text color of code panels.
const CODE_TEXT: Rgb = Rgb::new(0xE6, 0xED, 0xF3);

/// Line-number color of code panels.
const CODE_GUTTER: Rgb = Rgb::new(0x8B, 0x94, 0x9E);

/// An image stored in the package's `ppt/media/` folder.
#[derive(Debug)]
pub(crate) struct MediaImage {
    /// File name within `ppt/media/`.
    pub(crate) name: String,
    pub(crate) format: ImageFormat,
    pub(crate) data: Vec<u8>,
}

/// Parts shared across every slide in the package.
#[derive(Debug, Default)]
pub(crate) struct Media {
    pub(crate) images: Vec<MediaImage>,
    /// Chart parts: the XML of `ppt/charts/chart{n}.xml`, 1-based.
    pub(crate) charts: Vec<String>,
}

/// A relationship from a slide to another part.
#[derive(Debug)]
pub(crate) struct Relationship {
    pub(crate) id: String,
    pub(crate) kind: &'static str,
    pub(crate) target: String,
}

/// Accumulates the shape tree and relationships of one slide.
#[derive(Debug)]
pub(crate) struct SlideBuilder<'a> {
    pub(crate) palette: &'a Palette,
    /// The slide's (approximate) background, used to pick readable colors.
    pub(crate) background: Rgb,
    media: &'a mut Media,
    reserved_relationships: usize,
    relationships: Vec<Relationship>,
    xml: String,
    next_id: u32,
}

impl<'a> SlideBuilder<'a> {
    /// Start a slide. `reserved_relationships` counts relationship ids
    /// (`rId1`, `rId2`, ...) the caller has already claimed for the layout,
    /// notes, and so on; shapes number theirs after those.
    pub(crate) fn new(
        palette: &'a Palette,
        background: Rgb,
        media: &'a mut Media,
        reserved_relationships: usize,
    ) -> Self {
        Self {
            palette,
            background,
            media,
            reserved_relationships,
            relationships: Vec::new(),
            xml: String::new(),
            next_id: 2,
        }
    }

    /// The shape tree XML and the relationships shapes added.
    pub(crate) fn into_parts(self) -> (String, Vec<Relationship>) {
        (self.xml, self.relationships)
    }

    fn add_relationship(&mut self, kind: &'static str, target: String) -> String {
        let id = format!(
            "rId{}",
            self.reserved_relationships + self.relationships.len() + 1
        );
        self.relationships.push(Relationship {
            id: id.clone(),
            kind,
            target,
        });
        id
    }

    fn next_id(&mut self, kind: &str) -> (u32, String) {
        let id = self.next_id;
        self.next_id += 1;
        (id, format!("{kind} {id}"))
    }
}

/// Render any shape onto the slide.
pub(crate) fn render(builder: &mut SlideBuilder<'_>, shape: &Shape) -> Result<(), String> {
    match shape {
        Shape::Text(text) => text_box(builder, text),
        Shape::Shape(shape) => shape_box(builder, shape),
        Shape::Line(line) => connector(builder, line),
        Shape::List(list) => list_box(builder, list),
        Shape::Picture(picture) => picture_shape(builder, picture),
        Shape::Table(table) => table_shape(builder, table),
        Shape::Chart(chart) => chart_shape(builder, chart),
        Shape::Code(code) => code_block(builder, code),
        Shape::Stat(stat) => stat_box(builder, stat),
    }
}

/// A short human-readable name for a shape, used in error messages.
pub(crate) fn kind(shape: &Shape) -> &'static str {
    match shape {
        Shape::Text(_) => "text",
        Shape::Shape(_) => "shape",
        Shape::Line(_) => "line",
        Shape::List(_) => "list",
        Shape::Picture(_) => "picture",
        Shape::Table(_) => "table",
        Shape::Chart(_) => "chart",
        Shape::Code(_) => "code",
        Shape::Stat(_) => "stat",
    }
}

// ── Validation ───────────────────────────────────────────────────────────

/// Check that a frame has a positive size and lies on the slide.
pub(crate) fn validate_frame(frame: &Frame) -> Result<(), String> {
    let Frame {
        x,
        y,
        width,
        height,
    } = *frame;
    if ![x, y, width, height].iter().all(|v| v.is_finite()) {
        return Err("frame coordinates must be finite numbers".to_owned());
    }
    if width <= 0.0 || height <= 0.0 {
        return Err(format!(
            "frame must have a positive size, got {width} × {height} inches"
        ));
    }
    check_point(x, y)?;
    check_point(x + width, y + height).map_err(|_| {
        format!(
            "frame extends past the slide: it spans ({x}, {y}) to ({}, {}) inches but the slide is {SLIDE_WIDTH} × {SLIDE_HEIGHT} inches",
            x + width,
            y + height,
        )
    })
}

fn check_point(x: f32, y: f32) -> Result<(), String> {
    let on_slide = |v: f32, max: f32| (-BOUNDS_TOLERANCE..=max + BOUNDS_TOLERANCE).contains(&v);
    if on_slide(x, SLIDE_WIDTH) && on_slide(y, SLIDE_HEIGHT) {
        Ok(())
    } else {
        Err(format!(
            "point ({x}, {y}) lies off the slide, which is {SLIDE_WIDTH} × {SLIDE_HEIGHT} inches"
        ))
    }
}

fn validate_font_size(size: f32, field: &str) -> Result<f32, String> {
    if size.is_finite() && (1.0..=400.0).contains(&size) {
        Ok(size)
    } else {
        Err(format!(
            "{field}: font size must be between 1 and 400 points, got {size}"
        ))
    }
}

// ── Text ─────────────────────────────────────────────────────────────────

/// A fully resolved text style.
#[derive(Debug, Clone)]
pub(crate) struct Style {
    pub(crate) size: f32,
    pub(crate) color: Rgb,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) font: Option<String>,
    pub(crate) align: &'static str,
}

impl Style {
    /// Resolve a WIT text style against defaults.
    fn resolve(
        style: &TextStyle,
        default_size: f32,
        default_color: Rgb,
        default_align: &'static str,
    ) -> Result<Self, String> {
        Ok(Self {
            size: validate_font_size(style.font_size.unwrap_or(default_size), "style")?,
            color: Rgb::parse_or(style.color.as_deref(), "style.color", default_color)?,
            bold: style.bold,
            italic: style.italic,
            font: style.font_family.clone(),
            align: style.align.map_or(default_align, align),
        })
    }
}

fn align(align: TextAlign) -> &'static str {
    match align {
        TextAlign::Left => "l",
        TextAlign::Center => "ctr",
        TextAlign::Right => "r",
        TextAlign::Justify => "just",
    }
}

fn anchor(align: VerticalAlign) -> &'static str {
    match align {
        VerticalAlign::Top => "t",
        VerticalAlign::Middle => "ctr",
        VerticalAlign::Bottom => "b",
    }
}

/// `<a:rPr>` for a run.
fn run_properties(style: &Style, tag: &str) -> String {
    let mut out = format!("<a:{tag} lang=\"en-US\" sz=\"{}\"", centipoints(style.size));
    if style.bold {
        out.push_str(" b=\"1\"");
    }
    if style.italic {
        out.push_str(" i=\"1\"");
    }
    let _ = write!(
        out,
        " dirty=\"0\"><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>",
        style.color.hex()
    );
    if let Some(font) = &style.font {
        let font = escape(font);
        let _ = write!(
            out,
            "<a:latin typeface=\"{font}\"/><a:cs typeface=\"{font}\"/>"
        );
    }
    let _ = write!(out, "</a:{tag}>");
    out
}

/// One paragraph. Newlines in `text` become line breaks. `properties` is
/// extra content for `<a:pPr>` (spacing and bullets).
fn paragraph(text: &str, style: &Style, attributes: &str, properties: &str) -> String {
    let mut out = format!(
        "<a:p><a:pPr algn=\"{}\"{attributes}>{properties}</a:pPr>",
        style.align
    );
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            let _ = write!(out, "<a:br>{}</a:br>", run_properties(style, "rPr"));
        }
        if !line.is_empty() {
            let _ = write!(
                out,
                "<a:r>{}<a:t>{}</a:t></a:r>",
                run_properties(style, "rPr"),
                escape(line)
            );
        }
    }
    out.push_str(&run_properties(style, "endParaRPr"));
    out.push_str("</a:p>");
    out
}

/// `<p:txBody>` wrapping the given paragraphs.
fn text_body(paragraphs: &str, anchor: &str, inset: f32, autofit: bool) -> String {
    let inset = emu(inset);
    let fit = if autofit { "<a:normAutofit/>" } else { "" };
    let paragraphs = if paragraphs.is_empty() {
        "<a:p><a:endParaRPr lang=\"en-US\" dirty=\"0\"/></a:p>"
    } else {
        paragraphs
    };
    format!(
        "<p:txBody><a:bodyPr wrap=\"square\" lIns=\"{inset}\" tIns=\"{inset}\" rIns=\"{inset}\" bIns=\"{inset}\" anchor=\"{anchor}\" rtlCol=\"0\">{fit}</a:bodyPr><a:lstStyle/>{paragraphs}</p:txBody>"
    )
}

fn transform(frame: &Frame) -> String {
    format!(
        "<a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>",
        emu(frame.x),
        emu(frame.y),
        emu(frame.width),
        emu(frame.height)
    )
}

fn solid_fill(color: Rgb, opacity: Option<f32>) -> String {
    match opacity {
        Some(opacity) if opacity < 1.0 => format!(
            "<a:solidFill><a:srgbClr val=\"{}\"><a:alpha val=\"{}\"/></a:srgbClr></a:solidFill>",
            color.hex(),
            per_mille_percent(f64::from(opacity))
        ),
        _ => format!(
            "<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>",
            color.hex()
        ),
    }
}

/// Emit a `<p:sp>` element.
#[allow(
    clippy::too_many_arguments,
    reason = "internal helper mirroring OOXML structure"
)]
fn sp(
    builder: &mut SlideBuilder<'_>,
    kind: &str,
    text_box: bool,
    frame: &Frame,
    geometry: &str,
    fill: &str,
    line: &str,
    body: &str,
) {
    let (id, name) = builder.next_id(kind);
    let tx = if text_box { " txBox=\"1\"" } else { "" };
    let _ = write!(
        builder.xml,
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvSpPr{tx}/><p:nvPr/></p:nvSpPr><p:spPr>{}<a:prstGeom prst=\"{geometry}\"><a:avLst/></a:prstGeom>{fill}{line}</p:spPr>{body}</p:sp>",
        transform(frame)
    );
}

fn text_box(builder: &mut SlideBuilder<'_>, text: &TextBox) -> Result<(), String> {
    validate_frame(&text.frame)?;
    let fill = text
        .fill
        .as_deref()
        .map(|fill| Rgb::parse(fill, "fill"))
        .transpose()?;
    let background = fill.unwrap_or(builder.background);
    let style = Style::resolve(&text.style, 18.0, builder.palette.text_on(background), "l")?;
    let paragraphs: String = text
        .paragraphs
        .iter()
        .map(|p| paragraph(p, &style, "", ""))
        .collect();
    let anchor = text.vertical_align.map_or("t", anchor);
    let body = text_body(&paragraphs, anchor, 0.1, true);
    let fill = fill.map_or_else(|| "<a:noFill/>".to_owned(), |fill| solid_fill(fill, None));
    sp(
        builder,
        "TextBox",
        true,
        &text.frame,
        "rect",
        &fill,
        "",
        &body,
    );
    Ok(())
}

fn geometry(geometry: Geometry) -> &'static str {
    match geometry {
        Geometry::Rectangle => "rect",
        Geometry::RoundedRectangle => "roundRect",
        Geometry::Ellipse => "ellipse",
        Geometry::Triangle => "triangle",
        Geometry::Diamond => "diamond",
        Geometry::Hexagon => "hexagon",
        Geometry::Chevron => "chevron",
        Geometry::RightArrow => "rightArrow",
    }
}

fn shape_box(builder: &mut SlideBuilder<'_>, shape: &ShapeBox) -> Result<(), String> {
    validate_frame(&shape.frame)?;
    let fill = Rgb::parse_or(shape.fill.as_deref(), "fill", builder.palette.accent())?;
    if let Some(opacity) = shape.opacity {
        if !(0.0..=1.0).contains(&opacity) {
            return Err(format!("opacity must be between 0 and 1, got {opacity}"));
        }
    }
    let line = match &shape.border {
        Some(border) => {
            if !border.width.is_finite() || !(0.0..=100.0).contains(&border.width) {
                return Err(format!(
                    "border width must be between 0 and 100 points, got {}",
                    border.width
                ));
            }
            let color = Rgb::parse(&border.color, "border.color")?;
            format!(
                "<a:ln w=\"{}\">{}</a:ln>",
                emu_points(border.width),
                solid_fill(color, None)
            )
        }
        None => "<a:ln><a:noFill/></a:ln>".to_owned(),
    };
    // Translucent shapes show the slide through them, so pick text color
    // against a blend of the two.
    let effective = fill.mix(
        builder.background,
        1.0 - f64::from(shape.opacity.unwrap_or(1.0)),
    );
    let style = Style::resolve(&shape.style, 16.0, effective.readable_text(), "ctr")?;
    let body = shape.text.as_deref().map_or_else(String::new, |text| {
        text_body(&paragraph(text, &style, "", ""), "ctr", 0.1, false)
    });
    sp(
        builder,
        "Shape",
        false,
        &shape.frame,
        geometry(shape.geometry),
        &solid_fill(fill, shape.opacity),
        &line,
        &body,
    );
    Ok(())
}

fn connector(builder: &mut SlideBuilder<'_>, line: &Connector) -> Result<(), String> {
    let Connector { x1, y1, x2, y2, .. } = *line;
    if ![x1, y1, x2, y2].iter().all(|v| v.is_finite()) {
        return Err("line coordinates must be finite numbers".to_owned());
    }
    check_point(x1, y1)?;
    check_point(x2, y2)?;
    let width = line.width.unwrap_or(1.5);
    if !width.is_finite() || !(0.1..=100.0).contains(&width) {
        return Err(format!(
            "line width must be between 0.1 and 100 points, got {width}"
        ));
    }
    let color = Rgb::parse_or(line.color.as_deref(), "color", builder.palette.subtle)?;
    let dash = match line.dash {
        Dash::Solid => "",
        Dash::Dash => "<a:prstDash val=\"dash\"/>",
        Dash::Dot => "<a:prstDash val=\"sysDot\"/>",
        Dash::DashDot => "<a:prstDash val=\"dashDot\"/>",
    };
    let head = "<a:headEnd type=\"triangle\" w=\"med\" len=\"med\"/>";
    let tail = "<a:tailEnd type=\"triangle\" w=\"med\" len=\"med\"/>";
    let ends = match line.arrowheads {
        Arrowheads::None => String::new(),
        Arrowheads::End => tail.to_owned(),
        Arrowheads::Both => format!("{head}{tail}"),
    };
    let (left, top) = (x1.min(x2), y1.min(y2));
    let flip_h = if x2 < x1 { " flipH=\"1\"" } else { "" };
    let flip_v = if y2 < y1 { " flipV=\"1\"" } else { "" };
    let (id, name) = builder.next_id("Connector");
    let _ = write!(
        builder.xml,
        "<p:cxnSp><p:nvCxnSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm{flip_h}{flip_v}><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm><a:prstGeom prst=\"line\"><a:avLst/></a:prstGeom><a:ln w=\"{}\">{}{dash}{ends}</a:ln></p:spPr></p:cxnSp>",
        emu(left),
        emu(top),
        emu((x2 - x1).abs()),
        emu((y2 - y1).abs()),
        emu_points(width),
        solid_fill(color, None),
    );
    Ok(())
}

fn list_box(builder: &mut SlideBuilder<'_>, list: &ListBox) -> Result<(), String> {
    validate_frame(&list.frame)?;
    let style = Style::resolve(
        &list.style,
        20.0,
        builder.palette.text_on(builder.background),
        "l",
    )?;
    let bullet_color = Rgb::parse_or(
        list.bullet_color.as_deref(),
        "bullet-color",
        builder.palette.accent(),
    )?;
    let paragraphs = list_paragraphs(&list.items, list.numbered, &style, bullet_color);
    let body = text_body(&paragraphs, "t", 0.1, true);
    sp(
        builder,
        "List",
        true,
        &list.frame,
        "rect",
        "<a:noFill/>",
        "",
        &body,
    );
    Ok(())
}

/// Paragraphs for a bulleted or numbered list.
pub(crate) fn list_paragraphs(
    items: &[String],
    numbered: bool,
    style: &Style,
    bullet: Rgb,
) -> String {
    let indent = emu_points(style.size * 1.4);
    let spacing = centipoints(style.size * 0.5);
    let attributes = format!(" marL=\"{indent}\" indent=\"-{indent}\"");
    let marker = if numbered {
        "<a:buAutoNum type=\"arabicPeriod\"/>"
    } else {
        "<a:buFont typeface=\"Arial\"/><a:buChar char=\"&#8226;\"/>"
    };
    let properties = format!(
        "<a:spcBef><a:spcPts val=\"{spacing}\"/></a:spcBef><a:buClr><a:srgbClr val=\"{}\"/></a:buClr>{marker}",
        bullet.hex()
    );
    items
        .iter()
        .map(|item| paragraph(item, style, &attributes, &properties))
        .collect()
}

fn picture_shape(builder: &mut SlideBuilder<'_>, picture: &Picture) -> Result<(), String> {
    validate_frame(&picture.frame)?;
    let info = image::inspect(&picture.data)?;
    let index = builder.media.images.len() + 1;
    let name = format!("image{index}.{}", info.format.extension());
    builder.media.images.push(MediaImage {
        name: name.clone(),
        format: info.format,
        data: picture.data.clone(),
    });
    let rel = builder.add_relationship(REL_IMAGE, format!("../media/{name}"));

    let mut frame = picture.frame;
    let mut crop = String::new();
    if let Some((width, height)) = info.size {
        let image_aspect = f64::from(width) / f64::from(height);
        let frame_aspect = f64::from(frame.width) / f64::from(frame.height);
        match picture.fit {
            ImageFit::Stretch => {}
            ImageFit::Contain => frame = contain(&frame, image_aspect),
            ImageFit::Cover => {
                // Crop the overflowing axis equally from both sides.
                let (horizontal, vertical) = if image_aspect > frame_aspect {
                    ((1.0 - frame_aspect / image_aspect) / 2.0, 0.0)
                } else {
                    (0.0, (1.0 - image_aspect / frame_aspect) / 2.0)
                };
                let (h, v) = (per_mille_percent(horizontal), per_mille_percent(vertical));
                crop = format!("<a:srcRect l=\"{h}\" t=\"{v}\" r=\"{h}\" b=\"{v}\"/>");
            }
        }
    }

    let (id, name) = builder.next_id("Picture");
    let description = picture
        .description
        .as_deref()
        .map(|d| format!(" descr=\"{}\"", escape(d)))
        .unwrap_or_default();
    let _ = write!(
        builder.xml,
        "<p:pic><p:nvPicPr><p:cNvPr id=\"{id}\" name=\"{name}\"{description}/><p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed=\"{rel}\"/>{crop}<a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>",
        transform(&frame)
    );
    Ok(())
}

/// The largest frame with the given aspect ratio centered inside `frame`.
#[allow(
    clippy::cast_possible_truncation,
    reason = "values are bounded by the slide size"
)]
pub(crate) fn contain(frame: &Frame, aspect: f64) -> Frame {
    let (w, h) = (f64::from(frame.width), f64::from(frame.height));
    let (width, height) = if aspect > w / h {
        (w, w / aspect)
    } else {
        (h * aspect, h)
    };
    Frame {
        x: frame.x + ((w - width) / 2.0) as f32,
        y: frame.y + ((h - height) / 2.0) as f32,
        width: width as f32,
        height: height as f32,
    }
}

fn table_shape(builder: &mut SlideBuilder<'_>, table: &Table) -> Result<(), String> {
    validate_frame(&table.frame)?;
    let columns = table.headers.len();
    if columns == 0 {
        return Err("table must have at least one header".to_owned());
    }
    for (i, row) in table.rows.iter().enumerate() {
        if row.len() != columns {
            return Err(format!(
                "table row {} has {} cells but there are {columns} headers",
                i + 1,
                row.len()
            ));
        }
    }
    let size = validate_font_size(table.font_size.unwrap_or(14.0), "font-size")?;
    let header_fill = Rgb::parse_or(
        table.header_fill.as_deref(),
        "header-fill",
        builder.palette.accent(),
    )?;
    let base = builder.background;
    let band = base.mix(builder.palette.foreground, 0.08);
    let border = base.mix(builder.palette.foreground, 0.25);

    let row_count = u32::try_from(table.rows.len() + 1).map_err(|_| "table has too many rows")?;
    let column_count = u32::try_from(columns).map_err(|_| "table has too many columns")?;
    let total_width = emu(table.frame.width);
    let column_width = total_width / i64::from(column_count);
    let row_height = emu(table.frame.height) / i64::from(row_count);

    let border_xml = |side: &str| {
        format!(
            "<a:{side} w=\"9525\"><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill></a:{side}>",
            border.hex()
        )
    };
    let borders = ["lnL", "lnR", "lnT", "lnB"].map(border_xml).concat();
    let cell = |text: &str, fill: Rgb, bold: bool, size: f32| {
        let style = Style {
            size,
            color: fill.readable_text(),
            bold,
            italic: false,
            font: None,
            align: "l",
        };
        format!(
            "<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{}</a:txBody><a:tcPr marL=\"91440\" marR=\"91440\" marT=\"45720\" marB=\"45720\" anchor=\"ctr\">{borders}{}</a:tcPr></a:tc>",
            paragraph(text, &style, "", ""),
            solid_fill(fill, None)
        )
    };

    let mut grid = String::new();
    for _ in 0..columns {
        let _ = write!(grid, "<a:gridCol w=\"{column_width}\"/>");
    }
    let mut rows = format!("<a:tr h=\"{row_height}\">");
    for header in &table.headers {
        rows.push_str(&cell(header, header_fill, true, size));
    }
    rows.push_str("</a:tr>");
    for (i, row) in table.rows.iter().enumerate() {
        let fill = if i % 2 == 1 { band } else { base };
        let _ = write!(rows, "<a:tr h=\"{row_height}\">");
        for value in row {
            rows.push_str(&cell(value, fill, false, size));
        }
        rows.push_str("</a:tr>");
    }

    let (id, name) = builder.next_id("Table");
    let _ = write!(
        builder.xml,
        "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp=\"1\"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr>{}<a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/table\"><a:tbl><a:tblPr firstRow=\"1\" bandRow=\"1\"/><a:tblGrid>{grid}</a:tblGrid>{rows}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>",
        graphic_transform(&table.frame)
    );
    Ok(())
}

fn graphic_transform(frame: &Frame) -> String {
    format!(
        "<p:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></p:xfrm>",
        emu(frame.x),
        emu(frame.y),
        emu(frame.width),
        emu(frame.height)
    )
}

fn chart_shape(builder: &mut SlideBuilder<'_>, chart: &ChartBox) -> Result<(), String> {
    validate_frame(&chart.frame)?;
    let text = builder.palette.text_on(builder.background);
    let grid = builder.background.mix(text, 0.2);
    let xml = charts::chart_xml(&chart.chart, builder.palette, text, grid)?;
    builder.media.charts.push(xml);
    let index = builder.media.charts.len();
    let rel = builder.add_relationship(REL_CHART, format!("../charts/chart{index}.xml"));
    let (id, name) = builder.next_id("Chart");
    let _ = write!(
        builder.xml,
        "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp=\"1\"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr>{}<a:graphic><a:graphicData uri=\"{ns}\"><c:chart xmlns:c=\"{ns}\" r:id=\"{rel}\"/></a:graphicData></a:graphic></p:graphicFrame>",
        graphic_transform(&chart.frame),
        ns = crate::xml::NS_C,
    );
    Ok(())
}

fn code_block(builder: &mut SlideBuilder<'_>, code: &CodeBlock) -> Result<(), String> {
    validate_frame(&code.frame)?;
    let size = validate_font_size(code.font_size.unwrap_or(14.0), "font-size")?;
    let style = Style {
        size,
        color: CODE_TEXT,
        bold: false,
        italic: false,
        font: Some(CODE_FONT.to_owned()),
        align: "l",
    };
    let gutter = Style {
        color: CODE_GUTTER,
        ..style.clone()
    };
    let source = code.code.trim_end_matches('\n');
    let lines: Vec<&str> = source.split('\n').collect();
    let digits = lines.len().to_string().len();
    let mut paragraphs = String::new();
    for (i, line) in lines.iter().enumerate() {
        let line = line.replace('\t', "    ");
        paragraphs.push_str("<a:p><a:pPr algn=\"l\"/>");
        if code.line_numbers {
            let _ = write!(
                paragraphs,
                "<a:r>{}<a:t>{:>digits$}  </a:t></a:r>",
                run_properties(&gutter, "rPr"),
                i + 1
            );
        }
        if !line.is_empty() {
            let _ = write!(
                paragraphs,
                "<a:r>{}<a:t>{}</a:t></a:r>",
                run_properties(&style, "rPr"),
                escape(&line)
            );
        }
        paragraphs.push_str(&run_properties(&style, "endParaRPr"));
        paragraphs.push_str("</a:p>");
    }
    let body = text_body(&paragraphs, "t", 0.2, true);
    sp(
        builder,
        "Code",
        true,
        &code.frame,
        "rect",
        &solid_fill(CODE_BACKGROUND, None),
        "",
        &body,
    );
    Ok(())
}

fn stat_box(builder: &mut SlideBuilder<'_>, stat: &StatBox) -> Result<(), String> {
    validate_frame(&stat.frame)?;
    let fill = stat
        .fill
        .as_deref()
        .map(|fill| Rgb::parse(fill, "fill"))
        .transpose()?;
    let background = fill.unwrap_or(builder.background);
    let value_color = if fill.is_some() {
        background.readable_text()
    } else {
        builder.palette.accent()
    };
    let height = stat.frame.height;
    let value = Style {
        size: (height * 72.0 * 0.4).clamp(12.0, 72.0),
        color: value_color,
        bold: true,
        italic: false,
        font: None,
        align: "ctr",
    };
    let label = Style {
        size: (height * 72.0 * 0.12).clamp(10.0, 24.0),
        color: builder.palette.muted_on(background),
        bold: false,
        ..value.clone()
    };
    let paragraphs =
        paragraph(&stat.value, &value, "", "") + &paragraph(&stat.label, &label, "", "");
    let body = text_body(&paragraphs, "ctr", 0.1, false);
    let fill = fill.map_or_else(|| "<a:noFill/>".to_owned(), |fill| solid_fill(fill, None));
    sp(
        builder,
        "Stat",
        true,
        &stat.frame,
        "roundRect",
        &fill,
        "",
        &body,
    );
    Ok(())
}

/// Emit a borderless text box with an already-resolved style. Used for
/// decorations (footers, slide numbers) that don't go through validation.
pub(crate) fn plain_text(builder: &mut SlideBuilder<'_>, frame: &Frame, text: &str, style: &Style) {
    let body = text_body(&paragraph(text, style, "", ""), "ctr", 0.05, false);
    sp(
        builder,
        "TextBox",
        true,
        frame,
        "rect",
        "<a:noFill/>",
        "",
        &body,
    );
}
