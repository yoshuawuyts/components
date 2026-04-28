//! Tablemark WIT component: convert tabular data from XLSX to Markdown.
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_docs_in_private_items,
    reason = "wit-bindgen generates unsafe FFI glue and undocumented items"
)]

wit_bindgen::generate!({
    world: "tablemark",
    path: "wit",
});

/// The WIT component implementation.
struct Component;

export!(Component);

impl Guest for Component {
    fn to_markdown(xlsx: Vec<u8>) -> Result<String, String> {
        Converter::new(xlsx).convert()
    }

    fn to_xlsx(markdown: String) -> Result<Vec<u8>, String> {
        Inverter::new(&markdown).convert()
    }
}

use calamine::{Data, Reader, Xlsx};
use std::convert::TryFrom;
use std::io::Cursor;

/// Converts an XLSX workbook to a Markdown document.
///
/// Each worksheet becomes a level-1 heading followed by a
/// GitHub-flavored Markdown table containing the sheet's cells.
struct Converter {
    xlsx: Vec<u8>,
    out: String,
}

impl Converter {
    /// Create a new converter wrapping the given XLSX bytes.
    fn new(xlsx: Vec<u8>) -> Self {
        Self {
            xlsx,
            out: String::new(),
        }
    }

    /// Consume the converter and return the rendered Markdown string.
    fn convert(mut self) -> Result<String, String> {
        let cursor = Cursor::new(std::mem::take(&mut self.xlsx));
        let mut workbook: Xlsx<_> = Xlsx::new(cursor).map_err(|e| e.to_string())?;

        let sheet_names = workbook.sheet_names().clone();
        for (idx, name) in sheet_names.iter().enumerate() {
            let range = workbook
                .worksheet_range(name)
                .map_err(|e| format!("failed to read sheet `{name}`: {e}"))?;

            if idx > 0 {
                self.out.push('\n');
            }
            self.write_heading(name);

            if range.is_empty() {
                self.out.push_str("_(empty)_\n");
                continue;
            }

            self.write_table(&range);
        }

        Ok(self.out)
    }

    /// Write a level-1 heading containing the sheet name.
    fn write_heading(&mut self, name: &str) {
        self.out.push_str("# ");
        self.out.push_str(name);
        self.out.push_str("\n\n");
    }

    /// Write the rows of a worksheet as a GitHub-flavored Markdown table.
    fn write_table(&mut self, range: &calamine::Range<Data>) {
        let width = range.width();
        let mut rows = range.rows();

        let header: Vec<String> = match rows.next() {
            Some(row) => row.iter().map(Self::cell_to_string).collect(),
            None => return,
        };
        Self::write_row(&mut self.out, &header, width);
        Self::write_separator(&mut self.out, width);

        for row in rows {
            let cells: Vec<String> = row.iter().map(Self::cell_to_string).collect();
            Self::write_row(&mut self.out, &cells, width);
        }
    }

    /// Append a single Markdown table row, padding to `width` columns.
    fn write_row(out: &mut String, cells: &[String], width: usize) {
        out.push('|');
        for i in 0..width {
            out.push(' ');
            if let Some(cell) = cells.get(i) {
                out.push_str(&escape(cell));
            }
            out.push_str(" |");
        }
        out.push('\n');
    }

    /// Append the Markdown table header separator row.
    fn write_separator(out: &mut String, width: usize) {
        out.push('|');
        for _ in 0..width {
            out.push_str(" --- |");
        }
        out.push('\n');
    }

    /// Render a single cell value as a string suitable for Markdown.
    fn cell_to_string(cell: &Data) -> String {
        match cell {
            Data::Empty => String::new(),
            Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
            Data::Float(f) => f.to_string(),
            Data::Int(i) => i.to_string(),
            Data::Bool(b) => b.to_string(),
            Data::DateTime(d) => d.to_string(),
            Data::Error(e) => format!("#ERR({e:?})"),
        }
    }
}

/// Escape characters that have special meaning inside a Markdown table cell.
fn escape(input: &str) -> String {
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

/// Converts a Markdown document containing GitHub-flavored tables to an XLSX
/// workbook.
///
/// Walks a stream of pulldown-cmark events and groups text under tables and
/// the most recent level-1 heading. Each table becomes a worksheet, with the
/// preceding heading used as the sheet name when present.
struct Inverter<'a> {
    markdown: &'a str,
}

/// In-memory representation of a single markdown table while it is being
/// parsed.
#[derive(Default)]
struct Sheet {
    name: Option<String>,
    rows: Vec<Vec<String>>,
}

impl<'a> Inverter<'a> {
    /// Create a new inverter for the given markdown string.
    fn new(markdown: &'a str) -> Self {
        Self { markdown }
    }

    /// Parse the markdown and emit an XLSX workbook.
    fn convert(self) -> Result<Vec<u8>, String> {
        let sheets = self.parse_sheets();

        let mut workbook = rust_xlsxwriter::Workbook::new();

        // Excel requires at least one worksheet in the workbook.
        if sheets.is_empty() {
            workbook.add_worksheet();
        } else {
            for (idx, sheet) in sheets.into_iter().enumerate() {
                let ws = workbook.add_worksheet();
                let name = sheet
                    .name
                    .as_deref()
                    .map_or_else(|| format!("Sheet{}", idx + 1), sanitize_sheet_name);
                ws.set_name(&name).map_err(|e| e.to_string())?;

                for (row_idx, row) in sheet.rows.iter().enumerate() {
                    for (col_idx, cell) in row.iter().enumerate() {
                        let row_u32 = u32::try_from(row_idx).map_err(|e| e.to_string())?;
                        let col_u16 = u16::try_from(col_idx).map_err(|e| e.to_string())?;
                        ws.write(row_u32, col_u16, cell)
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
        }

        workbook.save_to_buffer().map_err(|e| e.to_string())
    }

    /// Walk the markdown event stream and collect each table into a `Sheet`.
    fn parse_sheets(&self) -> Vec<Sheet> {
        use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

        let parser = Parser::new_ext(self.markdown, Options::ENABLE_TABLES);

        let mut sheets: Vec<Sheet> = Vec::new();
        let mut pending_heading: Option<String> = None;

        let mut in_h1 = false;
        let mut in_table = false;
        let mut current: Option<Sheet> = None;
        let mut current_row: Option<Vec<String>> = None;
        let mut current_cell: Option<String> = None;
        let mut heading_buf = String::new();

        for event in parser {
            match event {
                Event::Start(Tag::Heading {
                    level: HeadingLevel::H1,
                    ..
                }) => {
                    in_h1 = true;
                    heading_buf.clear();
                }
                Event::End(TagEnd::Heading(HeadingLevel::H1)) => {
                    in_h1 = false;
                    pending_heading = Some(heading_buf.trim().to_owned());
                }
                Event::Start(Tag::Table(_)) => {
                    in_table = true;
                    current = Some(Sheet {
                        name: pending_heading.take(),
                        rows: Vec::new(),
                    });
                }
                Event::End(TagEnd::Table) => {
                    in_table = false;
                    if let Some(sheet) = current.take() {
                        sheets.push(sheet);
                    }
                }
                Event::Start(Tag::TableHead | Tag::TableRow) => {
                    current_row = Some(Vec::new());
                }
                Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                    if let (Some(row), Some(sheet)) = (current_row.take(), current.as_mut()) {
                        sheet.rows.push(row);
                    }
                }
                Event::Start(Tag::TableCell) => {
                    current_cell = Some(String::new());
                }
                Event::End(TagEnd::TableCell) => {
                    if let (Some(cell), Some(row)) = (current_cell.take(), current_row.as_mut()) {
                        row.push(unescape_cell(&cell));
                    }
                }
                Event::Text(t) | Event::Code(t) => {
                    if in_h1 {
                        heading_buf.push_str(&t);
                    } else if in_table {
                        if let Some(cell) = current_cell.as_mut() {
                            cell.push_str(&t);
                        }
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

        sheets
    }
}

/// Reverse the cell-escaping performed by [`escape`].
fn unescape_cell(input: &str) -> String {
    input.replace("\\|", "|")
}

/// Coerce a string into a valid Excel sheet name.
///
/// Excel sheet names cannot exceed 31 characters and cannot contain any of
/// the characters `[ ] * ? : / \`.
fn sanitize_sheet_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '[' | ']' | '*' | '?' | ':' | '/' | '\\' => '_',
            _ => c,
        })
        .collect();
    if out.is_empty() {
        out.push_str("Sheet");
    }
    if out.chars().count() > 31 {
        out = out.chars().take(31).collect();
    }
    out
}
