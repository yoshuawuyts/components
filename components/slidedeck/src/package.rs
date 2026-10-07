//! Assembling the OOXML package.

use crate::layouts::{self, Fill};
use crate::shapes::{self, Media, Relationship, SlideBuilder, Style};
use crate::theme::{Palette, Rgb};
use crate::types::{Deck, Frame, Slide, Transition};
use crate::xml::{
    angle, emu, escape, DECLARATION, NS_A, NS_P, NS_R, SLIDE_HEIGHT_EMU, SLIDE_WIDTH,
    SLIDE_WIDTH_EMU,
};
use crate::zip::ZipWriter;
use std::fmt::Write as _;

/// Most slides a deck may contain.
const MAX_SLIDES: usize = 1000;

const REL_BASE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL_PACKAGE: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const CT_BASE: &str = "application/vnd.openxmlformats-officedocument";

/// A finished slide, waiting to be written.
struct RenderedSlide {
    xml: String,
    rels: Vec<Relationship>,
    notes: Option<String>,
}

/// Render a deck to the bytes of a `.pptx` file.
pub(crate) fn build(deck: &Deck) -> Result<Vec<u8>, String> {
    if deck.slides.is_empty() {
        return Err("deck must contain at least one slide".to_owned());
    }
    if deck.slides.len() > MAX_SLIDES {
        return Err(format!(
            "deck has {} slides, but at most {MAX_SLIDES} are supported",
            deck.slides.len()
        ));
    }
    let palette = Palette::of(deck.theme);
    let mut media = Media::default();
    let mut slides = Vec::with_capacity(deck.slides.len());
    let mut notes_count = 0;
    for (i, slide) in deck.slides.iter().enumerate() {
        let notes = slide
            .notes
            .as_ref()
            .filter(|notes| !notes.trim().is_empty())
            .cloned();
        if notes.is_some() {
            notes_count += 1;
        }
        let rendered = render_slide(
            deck,
            &palette,
            &mut media,
            slide,
            i + 1,
            notes.is_some().then_some(notes_count),
        )
        .map_err(|e| format!("slide {}: {e}", i + 1))?;
        slides.push(RenderedSlide { notes, ..rendered });
    }

    let has_notes = notes_count > 0;
    let mut zip = ZipWriter::new();
    zip.add(
        "[Content_Types].xml",
        content_types(&slides, &media, has_notes).as_bytes(),
    )?;
    zip.add("_rels/.rels", root_rels().as_bytes())?;
    zip.add("docProps/core.xml", core_props(deck).as_bytes())?;
    zip.add("docProps/app.xml", app_props(slides.len()).as_bytes())?;
    zip.add(
        "ppt/presentation.xml",
        presentation(slides.len(), has_notes).as_bytes(),
    )?;
    zip.add(
        "ppt/_rels/presentation.xml.rels",
        presentation_rels(slides.len(), has_notes).as_bytes(),
    )?;
    zip.add("ppt/presProps.xml", pres_props().as_bytes())?;
    zip.add("ppt/viewProps.xml", view_props().as_bytes())?;
    zip.add("ppt/tableStyles.xml", table_styles().as_bytes())?;
    zip.add("ppt/theme/theme1.xml", theme(&palette).as_bytes())?;
    zip.add(
        "ppt/slideMasters/slideMaster1.xml",
        slide_master(&palette).as_bytes(),
    )?;
    zip.add(
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        rels(&[
            rel("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"),
            rel("rId2", "theme", "../theme/theme1.xml"),
        ])
        .as_bytes(),
    )?;
    zip.add(
        "ppt/slideLayouts/slideLayout1.xml",
        slide_layout().as_bytes(),
    )?;
    zip.add(
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        rels(&[rel(
            "rId1",
            "slideMaster",
            "../slideMasters/slideMaster1.xml",
        )])
        .as_bytes(),
    )?;

    let mut notes_index = 0;
    for (i, slide) in slides.iter().enumerate() {
        let n = i + 1;
        zip.add(&format!("ppt/slides/slide{n}.xml"), slide.xml.as_bytes())?;
        zip.add(
            &format!("ppt/slides/_rels/slide{n}.xml.rels"),
            rels(&slide.rels).as_bytes(),
        )?;
        if let Some(notes) = &slide.notes {
            notes_index += 1;
            zip.add(
                &format!("ppt/notesSlides/notesSlide{notes_index}.xml"),
                notes_slide(notes).as_bytes(),
            )?;
            zip.add(
                &format!("ppt/notesSlides/_rels/notesSlide{notes_index}.xml.rels"),
                rels(&[
                    rel("rId1", "notesMaster", "../notesMasters/notesMaster1.xml"),
                    rel("rId2", "slide", &format!("../slides/slide{n}.xml")),
                ])
                .as_bytes(),
            )?;
        }
    }
    if has_notes {
        zip.add(
            "ppt/notesMasters/notesMaster1.xml",
            notes_master().as_bytes(),
        )?;
        zip.add(
            "ppt/notesMasters/_rels/notesMaster1.xml.rels",
            rels(&[rel("rId1", "theme", "../theme/theme2.xml")]).as_bytes(),
        )?;
        zip.add(
            "ppt/theme/theme2.xml",
            theme(&Palette::of(crate::types::Theme::LightClean)).as_bytes(),
        )?;
    }
    for image in &media.images {
        zip.add(&format!("ppt/media/{}", image.name), &image.data)?;
    }
    for (i, chart) in media.charts.iter().enumerate() {
        zip.add(&format!("ppt/charts/chart{}.xml", i + 1), chart.as_bytes())?;
    }
    zip.finish()
}

fn render_slide(
    deck: &Deck,
    palette: &Palette,
    media: &mut Media,
    slide: &Slide,
    number: usize,
    notes_index: Option<usize>,
) -> Result<RenderedSlide, String> {
    let fill = match &slide.background {
        Some(background) => Fill::parse(background)?,
        None => layouts::default_background(&slide.layout, palette),
    };
    let background = fill.average();
    let layout_shapes = layouts::expand(&slide.layout, palette, background)?;

    let mut reserved = vec![rel(
        "rId1",
        "slideLayout",
        "../slideLayouts/slideLayout1.xml",
    )];
    if let Some(index) = notes_index {
        reserved.push(rel(
            "rId2",
            "notesSlide",
            &format!("../notesSlides/notesSlide{index}.xml"),
        ));
    }
    let mut builder = SlideBuilder::new(palette, background, media, reserved.len());
    for shape in &layout_shapes {
        shapes::render(&mut builder, shape)?;
    }
    for (i, shape) in slide.shapes.iter().enumerate() {
        shapes::render(&mut builder, shape)
            .map_err(|e| format!("shape {} ({}): {e}", i + 1, shapes::kind(shape)))?;
    }
    decorations(&mut builder, deck, palette, background, number);
    let (tree, shape_rels) = builder.into_parts();
    reserved.extend(shape_rels);

    let transition = slide.transition.map_or_else(String::new, transition);
    let xml = format!(
        "{DECLARATION}<p:sld xmlns:a=\"{NS_A}\" xmlns:r=\"{NS_R}\" xmlns:p=\"{NS_P}\"><p:cSld>{}<p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr>{tree}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>{transition}</p:sld>",
        background_xml(fill)
    );
    Ok(RenderedSlide {
        xml,
        rels: reserved,
        notes: None,
    })
}

/// Footer text and slide numbers.
fn decorations(
    builder: &mut SlideBuilder<'_>,
    deck: &Deck,
    palette: &Palette,
    background: Rgb,
    number: usize,
) {
    let style = Style {
        size: 11.0,
        color: palette.muted_on(background),
        bold: false,
        italic: false,
        font: None,
        align: "l",
    };
    if let Some(footer) = deck.footer.as_deref().filter(|f| !f.trim().is_empty()) {
        let frame = Frame {
            x: 0.5,
            y: 7.0,
            width: 9.0,
            height: 0.4,
        };
        shapes::plain_text(builder, &frame, footer, &style);
    }
    if deck.slide_numbers {
        let frame = Frame {
            x: SLIDE_WIDTH - 2.0,
            y: 7.0,
            width: 1.5,
            height: 0.4,
        };
        let style = Style {
            align: "r",
            ..style
        };
        shapes::plain_text(builder, &frame, &number.to_string(), &style);
    }
}

fn background_xml(fill: Fill) -> String {
    let fill = match fill {
        Fill::Solid(color) => format!("<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>", color.hex()),
        Fill::Gradient { start, end, angle: degrees } => format!(
            "<a:gradFill rotWithShape=\"1\"><a:gsLst><a:gs pos=\"0\"><a:srgbClr val=\"{}\"/></a:gs><a:gs pos=\"100000\"><a:srgbClr val=\"{}\"/></a:gs></a:gsLst><a:lin ang=\"{}\" scaled=\"1\"/></a:gradFill>",
            start.hex(),
            end.hex(),
            angle(degrees.rem_euclid(360.0))
        ),
    };
    format!("<p:bg><p:bgPr>{fill}<a:effectLst/></p:bgPr></p:bg>")
}

fn transition(transition: Transition) -> String {
    let effect = match transition {
        Transition::Fade => "<p:fade/>",
        Transition::Push => "<p:push dir=\"l\"/>",
        Transition::Wipe => "<p:wipe dir=\"d\"/>",
        Transition::Split => "<p:split orient=\"horz\" dir=\"out\"/>",
        Transition::Cover => "<p:cover dir=\"l\"/>",
        Transition::Dissolve => "<p:dissolve/>",
        Transition::Zoom => "<p:zoom/>",
    };
    format!("<p:transition spd=\"med\">{effect}</p:transition>")
}

// ── Relationships ────────────────────────────────────────────────────────

fn rel(id: &str, kind: &str, target: &str) -> Relationship {
    let kind: &'static str = match kind {
        "slideLayout" => "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout",
        "slideMaster" => "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster",
        "theme" => "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme",
        "notesSlide" => "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide",
        "notesMaster" => "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesMaster",
        "slide" => "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide",
        "presProps" => "http://schemas.openxmlformats.org/officeDocument/2006/relationships/presProps",
        "viewProps" => "http://schemas.openxmlformats.org/officeDocument/2006/relationships/viewProps",
        "tableStyles" => "http://schemas.openxmlformats.org/officeDocument/2006/relationships/tableStyles",
        "officeDocument" => "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
        "extended-properties" => "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties",
        "core-properties" => "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties",
        _ => REL_BASE,
    };
    Relationship {
        id: id.to_owned(),
        kind,
        target: target.to_owned(),
    }
}

fn rels(relationships: &[Relationship]) -> String {
    let mut out = format!("{DECLARATION}<Relationships xmlns=\"{REL_PACKAGE}\">");
    for r in relationships {
        let _ = write!(
            out,
            "<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"/>",
            r.id,
            r.kind,
            escape(&r.target)
        );
    }
    out.push_str("</Relationships>");
    out
}

fn root_rels() -> String {
    rels(&[
        rel("rId1", "officeDocument", "ppt/presentation.xml"),
        rel("rId2", "core-properties", "docProps/core.xml"),
        rel("rId3", "extended-properties", "docProps/app.xml"),
    ])
}

fn presentation_rels(slides: usize, notes: bool) -> String {
    let mut list: Vec<Relationship> = (1..=slides)
        .map(|n| rel(&format!("rId{n}"), "slide", &format!("slides/slide{n}.xml")))
        .collect();
    let others = [
        ("slideMaster", "slideMasters/slideMaster1.xml"),
        ("theme", "theme/theme1.xml"),
        ("presProps", "presProps.xml"),
        ("viewProps", "viewProps.xml"),
        ("tableStyles", "tableStyles.xml"),
    ];
    for (kind, target) in others {
        let id = format!("rId{}", list.len() + 1);
        list.push(rel(&id, kind, target));
    }
    if notes {
        let id = format!("rId{}", list.len() + 1);
        list.push(rel(&id, "notesMaster", "notesMasters/notesMaster1.xml"));
    }
    rels(&list)
}

// ── Package-level parts ──────────────────────────────────────────────────

fn content_types(slides: &[RenderedSlide], media: &Media, notes: bool) -> String {
    let mut out = format!(
        "{DECLARATION}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>"
    );
    let mut formats: Vec<_> = media.images.iter().map(|image| image.format).collect();
    formats.dedup();
    let mut seen = Vec::new();
    for format in formats {
        if !seen.contains(&format) {
            seen.push(format);
            let _ = write!(
                out,
                "<Default Extension=\"{}\" ContentType=\"{}\"/>",
                format.extension(),
                format.content_type()
            );
        }
    }
    let mut overrides = vec![
        (
            "/ppt/presentation.xml",
            format!("{CT_BASE}.presentationml.presentation.main+xml"),
        ),
        (
            "/ppt/presProps.xml",
            format!("{CT_BASE}.presentationml.presProps+xml"),
        ),
        (
            "/ppt/viewProps.xml",
            format!("{CT_BASE}.presentationml.viewProps+xml"),
        ),
        (
            "/ppt/tableStyles.xml",
            format!("{CT_BASE}.presentationml.tableStyles+xml"),
        ),
        ("/ppt/theme/theme1.xml", format!("{CT_BASE}.theme+xml")),
        (
            "/ppt/slideMasters/slideMaster1.xml",
            format!("{CT_BASE}.presentationml.slideMaster+xml"),
        ),
        (
            "/ppt/slideLayouts/slideLayout1.xml",
            format!("{CT_BASE}.presentationml.slideLayout+xml"),
        ),
        (
            "/docProps/core.xml",
            "application/vnd.openxmlformats-package.core-properties+xml".to_owned(),
        ),
        (
            "/docProps/app.xml",
            format!("{CT_BASE}.extended-properties+xml"),
        ),
    ]
    .into_iter()
    .map(|(part, kind)| (part.to_owned(), kind))
    .collect::<Vec<_>>();
    for n in 1..=slides.len() {
        overrides.push((
            format!("/ppt/slides/slide{n}.xml"),
            format!("{CT_BASE}.presentationml.slide+xml"),
        ));
    }
    if notes {
        overrides.push((
            "/ppt/notesMasters/notesMaster1.xml".to_owned(),
            format!("{CT_BASE}.presentationml.notesMaster+xml"),
        ));
        overrides.push((
            "/ppt/theme/theme2.xml".to_owned(),
            format!("{CT_BASE}.theme+xml"),
        ));
        let count = slides.iter().filter(|s| s.notes.is_some()).count();
        for n in 1..=count {
            overrides.push((
                format!("/ppt/notesSlides/notesSlide{n}.xml"),
                format!("{CT_BASE}.presentationml.notesSlide+xml"),
            ));
        }
    }
    for n in 1..=media.charts.len() {
        overrides.push((
            format!("/ppt/charts/chart{n}.xml"),
            format!("{CT_BASE}.drawingml.chart+xml"),
        ));
    }
    for (part, kind) in overrides {
        let _ = write!(
            out,
            "<Override PartName=\"{part}\" ContentType=\"{kind}\"/>"
        );
    }
    out.push_str("</Types>");
    out
}

fn core_props(deck: &Deck) -> String {
    let title = deck.title.as_deref().map(escape).unwrap_or_default();
    let creator = deck.author.as_deref().map(escape).unwrap_or_default();
    format!(
        "{DECLARATION}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:dcmitype=\"http://purl.org/dc/dcmitype/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:title>{title}</dc:title><dc:creator>{creator}</dc:creator><cp:lastModifiedBy>{creator}</cp:lastModifiedBy><cp:revision>1</cp:revision></cp:coreProperties>"
    )
}

fn app_props(slides: usize) -> String {
    format!(
        "{DECLARATION}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\" xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\"><Application>slidedeck</Application><PresentationFormat>Widescreen</PresentationFormat><Slides>{slides}</Slides><AppVersion>16.0000</AppVersion></Properties>"
    )
}

fn presentation(slides: usize, notes: bool) -> String {
    let master_rel = slides + 1;
    let mut ids = String::new();
    for n in 1..=slides {
        let _ = write!(ids, "<p:sldId id=\"{}\" r:id=\"rId{n}\"/>", 255 + n);
    }
    let notes_master = if notes {
        format!(
            "<p:notesMasterIdLst><p:notesMasterId r:id=\"rId{}\"/></p:notesMasterIdLst>",
            slides + 6
        )
    } else {
        String::new()
    };
    let mut levels = String::new();
    for level in 1..=9 {
        let _ = write!(
            levels,
            "<a:lvl{level}pPr marL=\"{}\" algn=\"l\" defTabSz=\"914400\" rtl=\"0\" eaLnBrk=\"1\" latinLnBrk=\"0\" hangingPunct=\"1\"><a:defRPr sz=\"1800\" kern=\"1200\"><a:solidFill><a:schemeClr val=\"tx1\"/></a:solidFill><a:latin typeface=\"+mn-lt\"/><a:ea typeface=\"+mn-ea\"/><a:cs typeface=\"+mn-cs\"/></a:defRPr></a:lvl{level}pPr>",
            (level - 1) * 457_200
        );
    }
    format!(
        "{DECLARATION}<p:presentation xmlns:a=\"{NS_A}\" xmlns:r=\"{NS_R}\" xmlns:p=\"{NS_P}\" saveSubsetFonts=\"1\"><p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId{master_rel}\"/></p:sldMasterIdLst>{notes_master}<p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx=\"{SLIDE_WIDTH_EMU}\" cy=\"{SLIDE_HEIGHT_EMU}\"/><p:notesSz cx=\"{SLIDE_HEIGHT_EMU}\" cy=\"{SLIDE_WIDTH_EMU}\"/><p:defaultTextStyle><a:defPPr><a:defRPr lang=\"en-US\"/></a:defPPr>{levels}</p:defaultTextStyle></p:presentation>"
    )
}

fn pres_props() -> String {
    format!(
        "{DECLARATION}<p:presentationPr xmlns:a=\"{NS_A}\" xmlns:r=\"{NS_R}\" xmlns:p=\"{NS_P}\"/>"
    )
}

fn view_props() -> String {
    format!(
        "{DECLARATION}<p:viewPr xmlns:a=\"{NS_A}\" xmlns:r=\"{NS_R}\" xmlns:p=\"{NS_P}\"><p:normalViewPr><p:restoredLeft sz=\"15620\"/><p:restoredTop sz=\"94660\"/></p:normalViewPr><p:gridSpacing cx=\"76200\" cy=\"76200\"/></p:viewPr>"
    )
}

fn table_styles() -> String {
    format!(
        "{DECLARATION}<a:tblStyleLst xmlns:a=\"{NS_A}\" def=\"{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}\"/>"
    )
}

// ── Theme, master, and layout ────────────────────────────────────────────

fn theme(palette: &Palette) -> String {
    let dark = palette.background.is_dark();
    let (dk1, lt1) = if dark {
        (palette.background, palette.foreground)
    } else {
        (palette.foreground, palette.background)
    };
    let (dk2, lt2) = if dark {
        (
            palette.background.mix(palette.foreground, 0.15),
            palette.subtle,
        )
    } else {
        (
            palette.subtle,
            palette.background.mix(palette.foreground, 0.08),
        )
    };
    let color = |name: &str, color: Rgb| {
        format!("<a:{name}><a:srgbClr val=\"{}\"/></a:{name}>", color.hex())
    };
    let [a1, a2, a3, a4] = palette.accents;
    let colors = [
        color("dk1", dk1),
        color("lt1", lt1),
        color("dk2", dk2),
        color("lt2", lt2),
        color("accent1", a1),
        color("accent2", a2),
        color("accent3", a3),
        color("accent4", a4),
        color("accent5", palette.series(4)),
        color("accent6", palette.series(5)),
        color("hlink", a1),
        color("folHlink", a2),
    ]
    .concat();
    let fonts = |font: &str| {
        let font = escape(font);
        format!("<a:latin typeface=\"{font}\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/>")
    };
    let name = escape(palette.name);
    format!(
        "{DECLARATION}<a:theme xmlns:a=\"{NS_A}\" name=\"{name}\"><a:themeElements><a:clrScheme name=\"{name}\">{colors}</a:clrScheme><a:fontScheme name=\"{name}\"><a:majorFont>{}</a:majorFont><a:minorFont>{}</a:minorFont></a:fontScheme><a:fmtScheme name=\"{name}\"><a:fillStyleLst><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w=\"6350\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln><a:ln w=\"12700\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln><a:ln w=\"19050\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>",
        fonts(palette.title_font),
        fonts(palette.body_font),
    )
}

fn color_map(dark: bool) -> &'static str {
    if dark {
        "bg1=\"dk1\" tx1=\"lt1\" bg2=\"dk2\" tx2=\"lt2\""
    } else {
        "bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\""
    }
}

const COLOR_MAP_ACCENTS: &str = "accent1=\"accent1\" accent2=\"accent2\" accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"";

const EMPTY_TREE: &str = "<p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr></p:spTree>";

fn slide_master(palette: &Palette) -> String {
    let map = color_map(palette.background.is_dark());
    let background = background_xml(Fill::Solid(palette.background));
    format!(
        "{DECLARATION}<p:sldMaster xmlns:a=\"{NS_A}\" xmlns:r=\"{NS_R}\" xmlns:p=\"{NS_P}\"><p:cSld>{background}{EMPTY_TREE}</p:cSld><p:clrMap {map} {COLOR_MAP_ACCENTS}/><p:sldLayoutIdLst><p:sldLayoutId id=\"2147483649\" r:id=\"rId1\"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr><a:defRPr sz=\"4400\"><a:solidFill><a:schemeClr val=\"tx1\"/></a:solidFill><a:latin typeface=\"+mj-lt\"/></a:defRPr></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr><a:defRPr sz=\"2000\"><a:solidFill><a:schemeClr val=\"tx1\"/></a:solidFill><a:latin typeface=\"+mn-lt\"/></a:defRPr></a:lvl1pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz=\"1800\"><a:solidFill><a:schemeClr val=\"tx1\"/></a:solidFill><a:latin typeface=\"+mn-lt\"/></a:defRPr></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"
    )
}

fn slide_layout() -> String {
    format!(
        "{DECLARATION}<p:sldLayout xmlns:a=\"{NS_A}\" xmlns:r=\"{NS_R}\" xmlns:p=\"{NS_P}\" type=\"blank\" preserve=\"1\"><p:cSld name=\"Blank\">{EMPTY_TREE}</p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"
    )
}

// ── Notes ────────────────────────────────────────────────────────────────

fn notes_master() -> String {
    format!(
        "{DECLARATION}<p:notesMaster xmlns:a=\"{NS_A}\" xmlns:r=\"{NS_R}\" xmlns:p=\"{NS_P}\"><p:cSld>{}{EMPTY_TREE}</p:cSld><p:clrMap {} {COLOR_MAP_ACCENTS}/></p:notesMaster>",
        background_xml(Fill::Solid(crate::theme::WHITE)),
        color_map(false)
    )
}

fn notes_slide(notes: &str) -> String {
    let mut paragraphs = String::new();
    for line in notes.trim().lines() {
        let line = escape(line);
        if line.is_empty() {
            paragraphs.push_str("<a:p><a:endParaRPr lang=\"en-US\" dirty=\"0\"/></a:p>");
        } else {
            let _ = write!(
                paragraphs,
                "<a:p><a:r><a:rPr lang=\"en-US\" dirty=\"0\"/><a:t>{line}</a:t></a:r></a:p>"
            );
        }
    }
    let (x, y, cx, cy) = (emu(0.75), emu(5.0), emu(6.0), emu(4.5));
    format!(
        "{DECLARATION}<p:notes xmlns:a=\"{NS_A}\" xmlns:r=\"{NS_R}\" xmlns:p=\"{NS_P}\"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Slide Image Placeholder 1\"/><p:cNvSpPr><a:spLocks noGrp=\"1\" noRot=\"1\" noChangeAspect=\"1\"/></p:cNvSpPr><p:nvPr><p:ph type=\"sldImg\"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp><p:sp><p:nvSpPr><p:cNvPr id=\"3\" name=\"Notes Placeholder 2\"/><p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr><p:nvPr><p:ph type=\"body\" idx=\"1\"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/>{paragraphs}</p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"
    )
}
