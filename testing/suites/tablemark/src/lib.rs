//! Test suite for the `tablemark` component.
//!
//! `tablemark` converts between GitHub-flavored Markdown tables and XLSX
//! workbooks. The XLSX side is binary (`list<u8>`) and an `.xlsx` is a ZIP
//! archive whose exact bytes are not deterministic, so we avoid byte-for-byte
//! assertions. Instead we assert on:
//!  * structural invariants (an `.xlsx` is a ZIP, so it starts with the `PK`
//!    local-file-header magic), and
//!  * **semantic round-trips** (markdown table -> xlsx -> markdown preserves the
//!    cell values).
//!
//! Two `wit_bindgen::generate!` calls combine here, mirroring the textsearch
//! suite: the `bindings` module generates the *imports* (tablemark's bare
//! funcs); `wasi_test::suite!` generates the `wasi:test/tests` *export*.

mod bindings {
    wit_bindgen::generate!({
        world: "imports",
        path: "wit",
        pub_export_macro: false,
    });
}

use wasi_test::TestContext;

/// A small GitHub-flavored Markdown table with a level-1 heading (used as the
/// sheet name) and two data rows.
const TABLE_MD: &str = "\
# People

| Name | Age |
| --- | --- |
| Alice | 30 |
| Bob | 25 |
";

fn test_to_xlsx_produces_zip(ctx: &TestContext) -> Result<(), String> {
    ctx.log("to-xlsx should produce a non-empty .xlsx (ZIP) container");
    let xlsx = bindings::to_xlsx(TABLE_MD)?;
    if xlsx.is_empty() {
        return Err("to-xlsx returned no bytes".to_string());
    }
    if !xlsx.starts_with(b"PK") {
        return Err("expected the .xlsx output to start with the ZIP magic 'PK'".to_string());
    }
    Ok(())
}

fn test_round_trip_preserves_cells(ctx: &TestContext) -> Result<(), String> {
    ctx.log("markdown table -> xlsx -> markdown should preserve the cell values");
    let xlsx = bindings::to_xlsx(TABLE_MD)?;
    let back = bindings::to_markdown(&xlsx)?;
    for needle in ["Name", "Age", "Alice", "30", "Bob", "25"] {
        if !back.contains(needle) {
            return Err(format!("round-trip dropped {needle:?}; got: {back:?}"));
        }
    }
    Ok(())
}

fn test_round_trip_preserves_sheet_name(ctx: &TestContext) -> Result<(), String> {
    ctx.log("the level-1 heading should become the sheet name and survive the round-trip");
    let xlsx = bindings::to_xlsx(TABLE_MD)?;
    let back = bindings::to_markdown(&xlsx)?;
    if !back.contains("People") {
        return Err(format!("round-trip dropped the sheet name 'People'; got: {back:?}"));
    }
    Ok(())
}

fn test_to_markdown_rejects_invalid_xlsx(ctx: &TestContext) -> Result<(), String> {
    ctx.log("to-markdown of non-xlsx bytes should return Err, not panic");
    match bindings::to_markdown(&[0u8, 1, 2, 3, 4]) {
        Ok(_) => Err("expected an error for invalid .xlsx input".to_string()),
        Err(_) => Ok(()),
    }
}

wasi_test::suite!(
    test_to_xlsx_produces_zip,
    test_round_trip_preserves_cells,
    test_round_trip_preserves_sheet_name,
    test_to_markdown_rejects_invalid_xlsx,
);
