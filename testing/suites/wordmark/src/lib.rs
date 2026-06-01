//! Test suite for the `wordmark` component.
//!
//! `wordmark` converts between Markdown and Word (`.docx`). Its API is binary
//! (`list<u8>` for the `.docx` side), and `.docx` is a ZIP archive whose exact
//! bytes are not deterministic, so we avoid byte-for-byte assertions. Instead we
//! assert on:
//!  * structural invariants (a `.docx` is a ZIP, so it starts with the `PK`
//!    local-file-header magic), and
//!  * **semantic round-trips** (markdown -> docx -> markdown preserves the
//!    visible text).
//!
//! Two `wit_bindgen::generate!` calls combine here, mirroring the textsearch
//! suite: the `bindings` module generates the *imports* (wordmark's bare funcs);
//! `wasi_test::suite!` generates the `wasi:test/tests` *export*.

mod bindings {
    wit_bindgen::generate!({
        world: "imports",
        path: "wit",
        pub_export_macro: false,
    });
}

use wasi_test::TestContext;

fn test_to_word_produces_docx_zip(ctx: &TestContext) -> Result<(), String> {
    ctx.log("to-word should produce a non-empty .docx (ZIP) container");
    let docx = bindings::to_word("# Hello\n\nworld")?;
    if docx.is_empty() {
        return Err("to-word returned no bytes".to_string());
    }
    if !docx.starts_with(b"PK") {
        return Err("expected the .docx output to start with the ZIP magic 'PK'".to_string());
    }
    Ok(())
}

fn test_round_trip_preserves_text(ctx: &TestContext) -> Result<(), String> {
    let markdown = "# Title\n\nThe quick brown fox.";
    ctx.log("markdown -> docx -> markdown should preserve the visible text");
    let docx = bindings::to_word(markdown)?;
    let back = bindings::to_markdown(&docx)?;
    for needle in ["Title", "quick brown fox"] {
        if !back.contains(needle) {
            return Err(format!("round-trip dropped {needle:?}; got: {back:?}"));
        }
    }
    Ok(())
}

fn test_round_trip_paragraph_text(ctx: &TestContext) -> Result<(), String> {
    let markdown = "Just a plain paragraph of text.";
    ctx.log("a plain paragraph should survive the round-trip");
    let docx = bindings::to_word(markdown)?;
    let back = bindings::to_markdown(&docx)?;
    if !back.contains("plain paragraph of text") {
        return Err(format!("round-trip dropped the paragraph text; got: {back:?}"));
    }
    Ok(())
}

fn test_to_markdown_rejects_invalid_docx(ctx: &TestContext) -> Result<(), String> {
    ctx.log("to-markdown of non-docx bytes should return Err, not panic");
    match bindings::to_markdown(&[0u8, 1, 2, 3, 4]) {
        Ok(_) => Err("expected an error for invalid .docx input".to_string()),
        Err(_) => Ok(()),
    }
}

wasi_test::suite!(
    test_to_word_produces_docx_zip,
    test_round_trip_preserves_text,
    test_round_trip_paragraph_text,
    test_to_markdown_rejects_invalid_docx,
);
