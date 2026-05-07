//! Csvmark WIT component: convert between CSV and Markdown tables.
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_docs_in_private_items,
    reason = "wit-bindgen generates unsafe FFI glue and undocumented items"
)]

wit_bindgen::generate!({
    world: "csvmark",
    path: "wit",
});

/// The WIT component implementation.
struct Component;

export!(Component);

impl Guest for Component {
    fn csv_to_md(input: String) -> Result<String, String> {
        csv_to_md(&input)
    }

    fn md_to_csv(input: String) -> Result<String, String> {
        md_to_csv(&input)
    }
}

/// Convert a CSV string to a GitHub-flavored Markdown table.
///
/// The first record is treated as the header row.
fn csv_to_md(input: &str) -> Result<String, String> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(input.as_bytes());

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut width = 0;
    for record in reader.records() {
        let record = record.map_err(|e| e.to_string())?;
        let row: Vec<String> = record.iter().map(str::to_owned).collect();
        width = width.max(row.len());
        rows.push(row);
    }

    if rows.is_empty() {
        return Ok(String::new());
    }
    // Ensure at least one column so the resulting table is well-formed.
    width = width.max(1);

    let mut out = String::new();
    let mut iter = rows.iter();
    let header = iter.next().expect("rows is non-empty");
    write_md_row(&mut out, header, width);
    write_md_separator(&mut out, width);
    for row in iter {
        write_md_row(&mut out, row, width);
    }
    Ok(out)
}

/// Append a single Markdown table row, padding to `width` columns.
fn write_md_row(out: &mut String, cells: &[String], width: usize) {
    out.push('|');
    for i in 0..width {
        out.push(' ');
        if let Some(cell) = cells.get(i) {
            out.push_str(&escape_md_cell(cell));
        }
        out.push_str(" |");
    }
    out.push('\n');
}

/// Append the Markdown table header separator row.
fn write_md_separator(out: &mut String, width: usize) {
    out.push('|');
    for _ in 0..width {
        out.push_str(" --- |");
    }
    out.push('\n');
}

/// Escape characters that have special meaning inside a Markdown table cell.
fn escape_md_cell(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '|' => out.push_str("\\|"),
            '\n' | '\r' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

/// Convert a Markdown document containing a GitHub-flavored table to CSV.
///
/// The first table encountered in the document is used.
fn md_to_csv(input: &str) -> Result<String, String> {
    let rows = parse_first_table(input);
    if rows.is_empty() {
        return Ok(String::new());
    }

    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    for row in &rows {
        writer.write_record(row).map_err(|e| e.to_string())?;
    }
    let bytes = writer.into_inner().map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

/// Walk the markdown event stream and collect the rows of the first table.
fn parse_first_table(input: &str) -> Vec<Vec<String>> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let parser = Parser::new_ext(input, Options::ENABLE_TABLES);

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut in_table = false;
    let mut done = false;
    let mut current_row: Option<Vec<String>> = None;
    let mut current_cell: Option<String> = None;

    for event in parser {
        if done {
            break;
        }
        match event {
            Event::Start(Tag::Table(_)) => in_table = true,
            Event::End(TagEnd::Table) => {
                in_table = false;
                done = true;
            }
            Event::Start(Tag::TableHead | Tag::TableRow) if in_table => {
                current_row = Some(Vec::new());
            }
            Event::End(TagEnd::TableHead | TagEnd::TableRow) if in_table => {
                if let Some(row) = current_row.take() {
                    rows.push(row);
                }
            }
            Event::Start(Tag::TableCell) if in_table => {
                current_cell = Some(String::new());
            }
            Event::End(TagEnd::TableCell) if in_table => {
                if let (Some(cell), Some(row)) = (current_cell.take(), current_row.as_mut()) {
                    row.push(unescape_md_cell(&cell));
                }
            }
            Event::Text(t) | Event::Code(t) if in_table => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.push_str(&t);
                }
            }
            Event::SoftBreak | Event::HardBreak if in_table => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.push(' ');
                }
            }
            _ => {}
        }
    }

    rows
}

/// Reverse the cell-escaping performed by [`escape_md_cell`].
fn unescape_md_cell(input: &str) -> String {
    input.replace("\\|", "|")
}

#[cfg(test)]
mod tests {
    use super::{csv_to_md, md_to_csv};

    #[test]
    fn csv_to_md_simple() {
        let csv = "name,age\nAlice,30\nBob,25\n";
        let md = csv_to_md(csv).unwrap();
        assert_eq!(
            md,
            "| name | age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |\n"
        );
    }

    #[test]
    fn csv_to_md_with_embedded_commas_and_quotes() {
        // Quoted fields with embedded commas and escaped double quotes.
        let csv = "name,note\n\"Doe, John\",\"He said \"\"hi\"\"\"\n";
        let md = csv_to_md(csv).unwrap();
        assert_eq!(
            md,
            "| name | note |\n| --- | --- |\n| Doe, John | He said \"hi\" |\n"
        );
    }

    #[test]
    fn csv_to_md_escapes_pipes_and_newlines() {
        // CSV field with an embedded newline and a pipe character.
        let csv = "a,b\n\"x|y\",\"line1\nline2\"\n";
        let md = csv_to_md(csv).unwrap();
        assert_eq!(md, "| a | b |\n| --- | --- |\n| x\\|y | line1 line2 |\n");
    }

    #[test]
    fn csv_to_md_utf8() {
        let csv = "город,страна\nМосква,Россия\n";
        let md = csv_to_md(csv).unwrap();
        assert_eq!(
            md,
            "| город | страна |\n| --- | --- |\n| Москва | Россия |\n"
        );
    }

    #[test]
    fn csv_to_md_empty_input() {
        assert_eq!(csv_to_md("").unwrap(), "");
    }

    #[test]
    fn csv_to_md_pads_short_rows() {
        let csv = "a,b,c\n1,2\n";
        let md = csv_to_md(csv).unwrap();
        assert_eq!(md, "| a | b | c |\n| --- | --- | --- |\n| 1 | 2 |  |\n");
    }

    #[test]
    fn md_to_csv_simple() {
        let md = "| name | age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |\n";
        let csv = md_to_csv(md).unwrap();
        assert_eq!(csv, "name,age\nAlice,30\nBob,25\n");
    }

    #[test]
    fn md_to_csv_quotes_special_fields() {
        let md = "| a | b |\n| --- | --- |\n| Doe, John | He said \"hi\" |\n";
        let csv = md_to_csv(md).unwrap();
        assert_eq!(csv, "a,b\n\"Doe, John\",\"He said \"\"hi\"\"\"\n");
    }

    #[test]
    fn md_to_csv_unescapes_pipes() {
        let md = "| a | b |\n| --- | --- |\n| x\\|y | z |\n";
        let csv = md_to_csv(md).unwrap();
        assert_eq!(csv, "a,b\nx|y,z\n");
    }

    #[test]
    fn md_to_csv_empty_input() {
        assert_eq!(md_to_csv("").unwrap(), "");
        assert_eq!(md_to_csv("just some prose").unwrap(), "");
    }

    #[test]
    fn round_trip_csv_md_csv() {
        let csv = "name,age\nAlice,30\nBob,25\n";
        let md = csv_to_md(csv).unwrap();
        let back = md_to_csv(&md).unwrap();
        assert_eq!(back, csv);
    }
}
