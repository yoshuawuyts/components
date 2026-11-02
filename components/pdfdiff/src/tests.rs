//! Native tests: build small PDFs, diff them, and check the output.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests may panic on unexpected input"
)]

use crate::exports::yoshuawuyts::pdfdiff::types::Options;
use crate::{compare, extract};
use std::fmt::Write;

// ── Helpers ──────────────────────────────────────────────────────────────

/// Build a PDF with one page per entry in `pages`, each line of text drawn
/// in 12pt Helvetica on its own line.
fn pdf(pages: &[&[&str]]) -> Vec<u8> {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            (0..pages.len())
                .map(|i| format!("{} 0 R", 4 + 2 * i))
                .collect::<Vec<_>>()
                .join(" "),
            pages.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_string(),
    ];
    for (i, lines) in pages.iter().enumerate() {
        let mut content = String::from("BT /F1 12 Tf 14 TL 72 720 Td\n");
        for line in *lines {
            let escaped = line
                .replace('\\', "\\\\")
                .replace('(', "\\(")
                .replace(')', "\\)");
            writeln!(content, "({escaped}) Tj T*").unwrap();
        }
        content.push_str("ET");
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + 2 * i
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ));
    }

    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        write!(out, "{} 0 obj\n{body}\nendobj\n", i + 1).unwrap();
    }
    let xref = out.len();
    write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).unwrap();
    for offset in offsets {
        writeln!(out, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    )
    .unwrap();
    out.into_bytes()
}

fn options() -> Options {
    Options {
        context_lines: 3,
        old_label: "old.pdf".to_string(),
        new_label: "new.pdf".to_string(),
        ignore_whitespace: false,
    }
}

// ── Extraction ───────────────────────────────────────────────────────────

#[test]
fn extracts_text_per_page() {
    let doc = pdf(&[&["Hello world", "Second line"], &["Page two"]]);
    let pages = extract::pages(&doc).unwrap();
    assert_eq!(pages, vec!["Hello world\nSecond line", "Page two"]);
}

#[test]
fn rejects_invalid_pdf() {
    let err = extract::pages(b"not a pdf").unwrap_err();
    assert!(!err.is_empty());
    let err = compare(b"not a pdf", &pdf(&[&["x"]]), &options()).unwrap_err();
    assert!(err.starts_with("old: "), "{err}");
    let err = compare(&pdf(&[&["x"]]), b"nope", &options()).unwrap_err();
    assert!(err.starts_with("new: "), "{err}");
}

// ── Diffing ──────────────────────────────────────────────────────────────

#[test]
fn identical_documents_produce_empty_diff() {
    let doc = pdf(&[&["alpha", "beta"]]);
    assert_eq!(compare(&doc, &doc, &options()).unwrap(), "");
}

#[test]
fn changed_line_is_reported() {
    let old = pdf(&[&["one", "two", "three", "four", "five"]]);
    let new = pdf(&[&["one", "two", "THREE", "four", "five"]]);
    let diff = compare(&old, &new, &options()).unwrap();
    assert_eq!(
        diff,
        "--- old.pdf\n\
         +++ new.pdf\n\
         @@ -1,5 +1,5 @@ page 1\n \
         one\n \
         two\n\
         -three\n\
         +THREE\n \
         four\n \
         five\n"
    );
}

#[test]
fn context_lines_are_respected() {
    let old = pdf(&[&["a", "b", "c", "d", "e"]]);
    let new = pdf(&[&["a", "b", "x", "d", "e"]]);
    let mut opts = options();
    opts.context_lines = 0;
    let diff = compare(&old, &new, &opts).unwrap();
    assert_eq!(
        diff,
        "--- old.pdf\n+++ new.pdf\n@@ -3 +3 @@ page 1\n-c\n+x\n"
    );
}

#[test]
fn hunks_report_their_page() {
    let old = pdf(&[&["intro"], &["body", "middle", "end"]]);
    let new = pdf(&[&["intro"], &["body", "middle", "ending"]]);
    let mut opts = options();
    opts.context_lines = 1;
    let diff = compare(&old, &new, &opts).unwrap();
    assert!(diff.contains("@@ -3,2 +3,2 @@ page 2\n"), "{diff}");
}

#[test]
fn insertions_and_deletions() {
    let old = pdf(&[&["keep", "drop me", "stay"]]);
    let new = pdf(&[&["keep", "stay", "added"]]);
    let diff = compare(&old, &new, &options()).unwrap();
    assert_eq!(
        diff,
        "--- old.pdf\n+++ new.pdf\n@@ -1,3 +1,3 @@ page 1\n keep\n-drop me\n stay\n+added\n"
    );
}

#[test]
fn pure_deletion_uses_old_page() {
    let old = pdf(&[&["a"], &["b", "gone"]]);
    let new = pdf(&[&["a"], &["b"]]);
    let mut opts = options();
    opts.context_lines = 0;
    let diff = compare(&old, &new, &opts).unwrap();
    assert_eq!(
        diff,
        "--- old.pdf\n+++ new.pdf\n@@ -3 +2,0 @@ page 2\n-gone\n"
    );
}

#[test]
fn ignore_whitespace_hides_spacing_changes() {
    let old = pdf(&[&["hello  world", "", "next"]]);
    let new = pdf(&[&["hello world", "next"]]);
    let mut opts = options();
    opts.ignore_whitespace = true;
    assert_eq!(compare(&old, &new, &opts).unwrap(), "");
    opts.ignore_whitespace = false;
    assert!(!compare(&old, &new, &opts).unwrap().is_empty());
}

#[test]
fn labels_stay_on_one_line() {
    let old = pdf(&[&["a"]]);
    let new = pdf(&[&["b"]]);
    let mut opts = options();
    opts.old_label = "evil\n+++ label".to_string();
    let diff = compare(&old, &new, &opts).unwrap();
    assert!(
        diff.starts_with("--- evil +++ label\n+++ new.pdf\n"),
        "{diff}"
    );
}
