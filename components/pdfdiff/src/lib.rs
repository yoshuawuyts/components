//! Pdfdiff WIT component: compare the text of two PDF documents.
//!
//! Both documents are parsed and their text is extracted page by page. The
//! text is then split into lines and diffed with the patience algorithm, and
//! the result is rendered as a unified diff (the format produced by
//! `diff -u`). Each hunk header carries the page number the change is on.
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_docs_in_private_items,
    reason = "wit-bindgen generates unsafe FFI glue and undocumented items"
)]

wit_bindgen::generate!({
    world: "pdfdiff",
    path: "wit",
});

mod extract;
mod unified;

#[cfg(test)]
mod tests;

use exports::yoshuawuyts::pdfdiff::diff::Guest;
use exports::yoshuawuyts::pdfdiff::types::Options;

struct Component;

impl Guest for Component {
    fn compare(old: Vec<u8>, new: Vec<u8>, options: Options) -> Result<String, String> {
        compare(&old, &new, &options)
    }

    fn extract_text(pdf: Vec<u8>) -> Result<Vec<String>, String> {
        extract::pages(&pdf)
    }
}

/// Diff the text of two PDFs, returning a unified diff.
fn compare(old: &[u8], new: &[u8], options: &Options) -> Result<String, String> {
    let old_pages = extract::pages(old).map_err(|e| format!("old: {e}"))?;
    let new_pages = extract::pages(new).map_err(|e| format!("new: {e}"))?;
    let old_lines = extract::lines(&old_pages, options.ignore_whitespace);
    let new_lines = extract::lines(&new_pages, options.ignore_whitespace);
    Ok(unified::render(&old_lines, &new_lines, options))
}

export!(Component);
