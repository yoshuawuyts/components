//! Test suite for the `pdfdiff` component.
//!
//! Builds small PDFs in memory, drives `compare` and `extract-text` across
//! the component boundary, and asserts on the unified diff output.
//!
//! The `bindings` module generates the *imports* (the `diff` interface);
//! `wasi_test::suite!` generates the `wasi:test/tests` *export*.

mod bindings {
    wit_bindgen::generate!({
        world: "imports",
        path: "wit",
        pub_export_macro: false,
    });
}

use bindings::yoshuawuyts::pdfdiff::diff::{compare, extract_text};
use bindings::yoshuawuyts::pdfdiff::types::Options;
use std::fmt::Write;
use wasi_test::TestContext;

/// Build a PDF with one page per entry in `pages`, each line of text drawn
/// in 12pt Helvetica on its own line.
fn pdf(pages: &[&[&str]]) -> Vec<u8> {
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", 4 + 2 * i))
        .collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
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
            let _ = writeln!(content, "({escaped}) Tj T*");
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
        let _ = write!(out, "{} 0 obj\n{body}\nendobj\n", i + 1);
    }
    let xref = out.len();
    let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for offset in offsets {
        let _ = writeln!(out, "{offset:010} 00000 n ");
    }
    let _ = write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    );
    out.into_bytes()
}

fn options(context_lines: u32) -> Options {
    Options {
        context_lines,
        old_label: "a/report.pdf".to_string(),
        new_label: "b/report.pdf".to_string(),
        ignore_whitespace: false,
    }
}

fn expect_eq(got: &str, want: &str) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!("expected:\n{want}\ngot:\n{got}"))
    }
}

fn test_extract_text(ctx: &TestContext) -> Result<(), String> {
    ctx.log("extract-text should return one string per page");
    let pages = extract_text(&pdf(&[&["Hello", "world"], &["Second page"]]))?;
    if pages == ["Hello\nworld", "Second page"] {
        Ok(())
    } else {
        Err(format!("unexpected pages: {pages:?}"))
    }
}

fn test_identical(ctx: &TestContext) -> Result<(), String> {
    ctx.log("identical documents should produce an empty diff");
    let doc = pdf(&[&["same", "text"]]);
    expect_eq(&compare(&doc, &doc, &options(3))?, "")
}

fn test_changed_line(ctx: &TestContext) -> Result<(), String> {
    ctx.log("a changed line should be reported as a unified diff hunk");
    let old = pdf(&[&["one", "two", "three"]]);
    let new = pdf(&[&["one", "2", "three"]]);
    expect_eq(
        &compare(&old, &new, &options(3))?,
        "--- a/report.pdf\n+++ b/report.pdf\n@@ -1,3 +1,3 @@ page 1\n one\n-two\n+2\n three\n",
    )
}

fn test_page_in_hunk_header(ctx: &TestContext) -> Result<(), String> {
    ctx.log("hunk headers should name the page of the change");
    let old = pdf(&[&["cover"], &["intro", "body"]]);
    let new = pdf(&[&["cover"], &["intro", "new body"]]);
    expect_eq(
        &compare(&old, &new, &options(0))?,
        "--- a/report.pdf\n+++ b/report.pdf\n@@ -3 +3 @@ page 2\n-body\n+new body\n",
    )
}

fn test_invalid_pdf_errors(ctx: &TestContext) -> Result<(), String> {
    ctx.log("an unparsable document should be named in the error");
    match compare(&pdf(&[&["ok"]]), b"not a pdf", &options(3)) {
        Ok(diff) => Err(format!("expected an error, got {diff:?}")),
        Err(e) if e.starts_with("new: ") => Ok(()),
        Err(e) => Err(format!("unexpected error: {e}")),
    }
}

wasi_test::suite!(
    test_extract_text,
    test_identical,
    test_changed_line,
    test_page_in_hunk_header,
    test_invalid_pdf_errors,
);
