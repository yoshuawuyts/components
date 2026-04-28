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
}

use calamine::{Data, Reader, Xlsx};
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
