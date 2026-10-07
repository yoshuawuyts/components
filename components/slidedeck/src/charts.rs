//! Native DrawingML chart parts.
//!
//! Charts carry their data in cached values only, without an embedded
//! workbook: PowerPoint renders and restyles them, but "Edit Data" won't have
//! a spreadsheet to open.

use crate::theme::{Palette, Rgb};
use crate::types::{BarChart, Chart, LineChart, PieChart, Series};
use crate::xml::{escape, DECLARATION, NS_A, NS_C, NS_R};
use std::fmt::Write as _;

/// Most series a chart may have: one per spreadsheet column B through Y.
const MAX_SERIES: usize = 24;

/// Most categories a chart may have.
const MAX_CATEGORIES: usize = 500;

/// Render a chart to the XML of a chart part. `text` colors titles, axes,
/// and legends; `grid` colors gridlines.
pub(crate) fn chart_xml(
    chart: &Chart,
    palette: &Palette,
    text: Rgb,
    grid: Rgb,
) -> Result<String, String> {
    let (title, legend, plot) = match chart {
        Chart::Bar(bar) => (
            bar.title.as_deref(),
            bar.show_legend,
            bar_plot(bar, palette, grid)?,
        ),
        Chart::Line(line) => (
            line.title.as_deref(),
            line.show_legend,
            line_plot(line, palette, grid)?,
        ),
        Chart::Pie(pie) => (
            pie.title.as_deref(),
            pie.show_legend,
            pie_plot(pie, palette)?,
        ),
    };
    let text = text.hex();
    let title = title.map_or_else(String::new, |title| {
        format!(
            "<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz=\"1800\" b=\"1\"/></a:pPr><a:r><a:rPr lang=\"en-US\" sz=\"1800\" b=\"1\"><a:solidFill><a:srgbClr val=\"{text}\"/></a:solidFill></a:rPr><a:t>{}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val=\"0\"/></c:title>",
            escape(title)
        )
    });
    let auto_title_deleted = u8::from(title.is_empty());
    let legend = if legend {
        "<c:legend><c:legendPos val=\"b\"/><c:overlay val=\"0\"/></c:legend>"
    } else {
        ""
    };
    Ok(format!(
        "{DECLARATION}<c:chartSpace xmlns:c=\"{NS_C}\" xmlns:a=\"{NS_A}\" xmlns:r=\"{NS_R}\"><c:roundedCorners val=\"0\"/><c:chart>{title}<c:autoTitleDeleted val=\"{auto_title_deleted}\"/><c:plotArea><c:layout/>{plot}</c:plotArea>{legend}<c:plotVisOnly val=\"1\"/><c:dispBlanksAs val=\"gap\"/></c:chart><c:spPr><a:noFill/><a:ln><a:noFill/></a:ln></c:spPr><c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz=\"1200\"><a:solidFill><a:srgbClr val=\"{text}\"/></a:solidFill></a:defRPr></a:pPr><a:endParaRPr lang=\"en-US\"/></a:p></c:txPr></c:chartSpace>"
    ))
}

// ── Validation ───────────────────────────────────────────────────────────

fn validate_categories(categories: &[String], field: &str) -> Result<(), String> {
    if categories.is_empty() {
        return Err(format!("chart {field} must not be empty"));
    }
    if categories.len() > MAX_CATEGORIES {
        return Err(format!(
            "chart has {} {field}, but at most {MAX_CATEGORIES} are supported",
            categories.len()
        ));
    }
    Ok(())
}

fn validate_values(values: &[f64], field: &str) -> Result<(), String> {
    match values.iter().position(|v| !v.is_finite()) {
        Some(i) => Err(format!("{field}: value {} is not a finite number", i + 1)),
        None => Ok(()),
    }
}

fn validate_series(series: &[Series], categories: usize) -> Result<(), String> {
    if series.is_empty() {
        return Err("chart must have at least one series".to_owned());
    }
    if series.len() > MAX_SERIES {
        return Err(format!(
            "chart has {} series, but at most {MAX_SERIES} are supported",
            series.len()
        ));
    }
    for (i, s) in series.iter().enumerate() {
        let field = format!("series {} (`{}`)", i + 1, s.name);
        if s.name.trim().is_empty() {
            return Err(format!("series {} must have a name", i + 1));
        }
        if s.values.len() != categories {
            return Err(format!(
                "{field} has {} values but there are {categories} categories",
                s.values.len()
            ));
        }
        validate_values(&s.values, &field)?;
    }
    Ok(())
}

// ── Shared pieces ────────────────────────────────────────────────────────

/// The spreadsheet column letter for series `index` (B, C, ...).
fn column(index: usize) -> char {
    let offset = u8::try_from(index).unwrap_or(0).min(24);
    char::from(b'B' + offset)
}

fn categories_xml(categories: &[String]) -> String {
    let mut points = String::new();
    for (i, category) in categories.iter().enumerate() {
        let _ = write!(
            points,
            "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>",
            escape(category)
        );
    }
    format!(
        "<c:cat><c:strRef><c:f>Sheet1!$A$2:$A${}</c:f><c:strCache><c:ptCount val=\"{}\"/>{points}</c:strCache></c:strRef></c:cat>",
        categories.len() + 1,
        categories.len()
    )
}

fn values_xml(values: &[f64], column: char) -> String {
    let mut points = String::new();
    for (i, value) in values.iter().enumerate() {
        let _ = write!(points, "<c:pt idx=\"{i}\"><c:v>{value}</c:v></c:pt>");
    }
    format!(
        "<c:val><c:numRef><c:f>Sheet1!${column}$2:${column}${}</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val=\"{}\"/>{points}</c:numCache></c:numRef></c:val>",
        values.len() + 1,
        values.len()
    )
}

fn series_name_xml(name: &str, column: char) -> String {
    format!(
        "<c:tx><c:strRef><c:f>Sheet1!${column}$1</c:f><c:strCache><c:ptCount val=\"1\"/><c:pt idx=\"0\"><c:v>{}</c:v></c:pt></c:strCache></c:strRef></c:tx>",
        escape(name)
    )
}

fn series_color(series: &Series, index: usize, palette: &Palette) -> Result<Rgb, String> {
    Rgb::parse_or(
        series.color.as_deref(),
        &format!("series {} color", index + 1),
        palette.series(index),
    )
}

fn data_labels(show_values: bool) -> &'static str {
    if show_values {
        "<c:dLbls><c:showLegendKey val=\"0\"/><c:showVal val=\"1\"/><c:showCatName val=\"0\"/><c:showSerName val=\"0\"/><c:showPercent val=\"0\"/><c:showBubbleSize val=\"0\"/></c:dLbls>"
    } else {
        ""
    }
}

/// A category axis and a value axis crossing each other.
fn axes(category_position: &str, value_position: &str, grid: Rgb) -> String {
    let grid = grid.hex();
    format!(
        "<c:catAx><c:axId val=\"1\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"{category_position}\"/><c:numFmt formatCode=\"General\" sourceLinked=\"0\"/><c:majorTickMark val=\"none\"/><c:minorTickMark val=\"none\"/><c:tickLblPos val=\"nextTo\"/><c:spPr><a:ln w=\"9525\"><a:solidFill><a:srgbClr val=\"{grid}\"/></a:solidFill></a:ln></c:spPr><c:crossAx val=\"2\"/><c:crosses val=\"autoZero\"/><c:auto val=\"1\"/><c:lblAlgn val=\"ctr\"/><c:lblOffset val=\"100\"/><c:noMultiLvlLbl val=\"0\"/></c:catAx>\
<c:valAx><c:axId val=\"2\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"{value_position}\"/><c:majorGridlines><c:spPr><a:ln w=\"9525\"><a:solidFill><a:srgbClr val=\"{grid}\"/></a:solidFill></a:ln></c:spPr></c:majorGridlines><c:numFmt formatCode=\"General\" sourceLinked=\"1\"/><c:majorTickMark val=\"none\"/><c:minorTickMark val=\"none\"/><c:tickLblPos val=\"nextTo\"/><c:spPr><a:ln><a:noFill/></a:ln></c:spPr><c:crossAx val=\"1\"/><c:crosses val=\"autoZero\"/><c:crossBetween val=\"between\"/></c:valAx>"
    )
}

// ── Chart kinds ──────────────────────────────────────────────────────────

fn bar_plot(chart: &BarChart, palette: &Palette, grid: Rgb) -> Result<String, String> {
    validate_categories(&chart.categories, "categories")?;
    validate_series(&chart.series, chart.categories.len())?;
    let mut series = String::new();
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(s, i, palette)?.hex();
        let _ = write!(
            series,
            "<c:ser><c:idx val=\"{i}\"/><c:order val=\"{i}\"/>{}<c:spPr><a:solidFill><a:srgbClr val=\"{color}\"/></a:solidFill></c:spPr><c:invertIfNegative val=\"0\"/>{}{}{}</c:ser>",
            series_name_xml(&s.name, column(i)),
            data_labels(chart.show_values),
            categories_xml(&chart.categories),
            values_xml(&s.values, column(i)),
        );
    }
    let (direction, category_position, value_position) = if chart.horizontal {
        ("bar", "l", "b")
    } else {
        ("col", "b", "l")
    };
    let (grouping, overlap) = if chart.stacked {
        ("stacked", "<c:overlap val=\"100\"/>")
    } else {
        ("clustered", "")
    };
    Ok(format!(
        "<c:barChart><c:barDir val=\"{direction}\"/><c:grouping val=\"{grouping}\"/><c:varyColors val=\"0\"/>{series}<c:gapWidth val=\"80\"/>{overlap}<c:axId val=\"1\"/><c:axId val=\"2\"/></c:barChart>{}",
        axes(category_position, value_position, grid)
    ))
}

fn line_plot(chart: &LineChart, palette: &Palette, grid: Rgb) -> Result<String, String> {
    validate_categories(&chart.categories, "categories")?;
    validate_series(&chart.series, chart.categories.len())?;
    let mut series = String::new();
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(s, i, palette)?.hex();
        let name = series_name_xml(&s.name, column(i));
        let labels = data_labels(chart.show_values);
        let categories = categories_xml(&chart.categories);
        let values = values_xml(&s.values, column(i));
        if chart.area {
            let _ = write!(
                series,
                "<c:ser><c:idx val=\"{i}\"/><c:order val=\"{i}\"/>{name}<c:spPr><a:solidFill><a:srgbClr val=\"{color}\"><a:alpha val=\"75000\"/></a:srgbClr></a:solidFill></c:spPr>{labels}{categories}{values}</c:ser>"
            );
        } else {
            let smooth = u8::from(chart.smooth);
            let _ = write!(
                series,
                "<c:ser><c:idx val=\"{i}\"/><c:order val=\"{i}\"/>{name}<c:spPr><a:ln w=\"31750\" cap=\"rnd\"><a:solidFill><a:srgbClr val=\"{color}\"/></a:solidFill><a:round/></a:ln></c:spPr><c:marker><c:symbol val=\"circle\"/><c:size val=\"6\"/><c:spPr><a:solidFill><a:srgbClr val=\"{color}\"/></a:solidFill><a:ln><a:noFill/></a:ln></c:spPr></c:marker>{labels}{categories}{values}<c:smooth val=\"{smooth}\"/></c:ser>"
            );
        }
    }
    let plot = if chart.area {
        format!("<c:areaChart><c:grouping val=\"standard\"/><c:varyColors val=\"0\"/>{series}<c:axId val=\"1\"/><c:axId val=\"2\"/></c:areaChart>")
    } else {
        format!("<c:lineChart><c:grouping val=\"standard\"/><c:varyColors val=\"0\"/>{series}<c:marker val=\"1\"/><c:axId val=\"1\"/><c:axId val=\"2\"/></c:lineChart>")
    };
    Ok(plot + &axes("b", "l", grid))
}

fn pie_plot(chart: &PieChart, palette: &Palette) -> Result<String, String> {
    validate_categories(&chart.labels, "labels")?;
    if chart.values.len() != chart.labels.len() {
        return Err(format!(
            "pie chart has {} values but {} labels",
            chart.values.len(),
            chart.labels.len()
        ));
    }
    validate_values(&chart.values, "pie chart")?;
    if chart.values.iter().any(|v| *v < 0.0) {
        return Err("pie chart values must not be negative".to_owned());
    }
    let mut points = String::new();
    for i in 0..chart.values.len() {
        let color = match chart.colors.get(i % chart.colors.len().max(1)) {
            Some(color) => Rgb::parse(color, &format!("pie chart color {}", i + 1))?,
            None => palette.series(i),
        };
        let _ = write!(
            points,
            "<c:dPt><c:idx val=\"{i}\"/><c:bubble3D val=\"0\"/><c:spPr><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill><a:ln><a:noFill/></a:ln></c:spPr></c:dPt>",
            color.hex()
        );
    }
    let labels = if chart.show_percent {
        let position = if chart.donut {
            ""
        } else {
            "<c:dLblPos val=\"bestFit\"/>"
        };
        format!(
            "<c:dLbls><c:numFmt formatCode=\"0%\" sourceLinked=\"0\"/>{position}<c:showLegendKey val=\"0\"/><c:showVal val=\"0\"/><c:showCatName val=\"0\"/><c:showSerName val=\"0\"/><c:showPercent val=\"1\"/><c:showBubbleSize val=\"0\"/><c:showLeaderLines val=\"1\"/></c:dLbls>"
        )
    } else {
        String::new()
    };
    let series = format!(
        "<c:ser><c:idx val=\"0\"/><c:order val=\"0\"/>{}{points}{labels}{}{}</c:ser>",
        series_name_xml("Values", 'B'),
        categories_xml(&chart.labels),
        values_xml(&chart.values, 'B'),
    );
    Ok(if chart.donut {
        format!("<c:doughnutChart><c:varyColors val=\"1\"/>{series}<c:firstSliceAng val=\"0\"/><c:holeSize val=\"55\"/></c:doughnutChart>")
    } else {
        format!("<c:pieChart><c:varyColors val=\"1\"/>{series}<c:firstSliceAng val=\"0\"/></c:pieChart>")
    })
}
