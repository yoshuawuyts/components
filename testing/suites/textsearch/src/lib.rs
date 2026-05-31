//! Test suite for the `textsearch` component.
//!
//! Two `wit_bindgen::generate!` calls combine here, mirroring Lann's
//! `wasi-http-tests` example:
//!  * the `bindings` module below generates the *imports* (textsearch's bare
//!    funcs) that the tests call;
//!  * `wasi_test::suite!` generates the `wasi:test/tests` *export*.
//!
//! At composition time `wac compose` wires the textsearch component's exports
//! into these imports.

mod bindings {
    wit_bindgen::generate!({
        world: "imports",
        path: "wit",
        pub_export_macro: false,
    });
}

use bindings::{File, Options};
use wasi_test::TestContext;

/// Default options: plain regex search, no context, no limits.
fn opts() -> Options {
    Options {
        case_insensitive: false,
        smart_case: false,
        fixed_strings: false,
        word: false,
        multiline: false,
        before_context: 0,
        after_context: 0,
        max_count: 0,
    }
}

fn test_search_finds_single_match(ctx: &TestContext) -> Result<(), String> {
    ctx.log("searching for 'world' in 'hello world'");
    let matches = bindings::search("world", "hello world", opts())?;
    if matches.len() != 1 {
        return Err(format!("expected 1 match, got {}", matches.len()));
    }
    if matches[0].text != "world" {
        return Err(format!("expected match text 'world', got {:?}", matches[0].text));
    }
    Ok(())
}

fn test_search_multiple_lines(ctx: &TestContext) -> Result<(), String> {
    let haystack = "alpha\nbeta\nalpha\ngamma";
    ctx.log("searching for 'alpha' across multiple lines");
    let matches = bindings::search("alpha", haystack, opts())?;
    if matches.len() != 2 {
        return Err(format!("expected 2 matches, got {}", matches.len()));
    }
    let lines: Vec<u64> = matches.iter().map(|m| m.line_number).collect();
    if lines != [1, 3] {
        return Err(format!("expected matches on lines [1, 3], got {lines:?}"));
    }
    Ok(())
}

fn test_search_case_insensitive(ctx: &TestContext) -> Result<(), String> {
    let mut o = opts();
    o.case_insensitive = true;
    ctx.log("case-insensitive search for 'HELLO'");
    let matches = bindings::search("HELLO", "hello Hello HELLO", o)?;
    if matches.len() != 3 {
        return Err(format!("expected 3 case-insensitive matches, got {}", matches.len()));
    }
    Ok(())
}

fn test_search_no_match(ctx: &TestContext) -> Result<(), String> {
    ctx.log("searching for a pattern that is absent");
    let matches = bindings::search("zzz", "hello world", opts())?;
    if !matches.is_empty() {
        return Err(format!("expected 0 matches, got {}", matches.len()));
    }
    Ok(())
}

fn test_search_invalid_regex_errors(ctx: &TestContext) -> Result<(), String> {
    ctx.log("an invalid regex should return an Err, not panic");
    match bindings::search("(unclosed", "hello", opts()) {
        Ok(_) => Err("expected an error for invalid regex".to_string()),
        Err(_) => Ok(()),
    }
}

fn test_replace_basic(ctx: &TestContext) -> Result<(), String> {
    ctx.log("replacing 'world' with 'there'");
    let out = bindings::replace("world", "there", "hello world", opts())?;
    if out != "hello there" {
        return Err(format!("expected 'hello there', got {out:?}"));
    }
    Ok(())
}

fn test_replace_capture_group(ctx: &TestContext) -> Result<(), String> {
    ctx.log("replacing with a capture-group reference");
    let out = bindings::replace(r"(\w+)@(\w+)", "$2.$1", "user@host", opts())?;
    if out != "host.user" {
        return Err(format!("expected 'host.user', got {out:?}"));
    }
    Ok(())
}

fn test_search_files_filters_empty(ctx: &TestContext) -> Result<(), String> {
    ctx.log("search-files should omit files with no matches");
    let files = vec![
        File { path: "a.txt".to_string(), contents: "find me here".to_string() },
        File { path: "b.txt".to_string(), contents: "nothing relevant".to_string() },
    ];
    let results = bindings::search_files("find", &files, opts())?;
    if results.len() != 1 {
        return Err(format!("expected matches in 1 file, got {}", results.len()));
    }
    if results[0].path != "a.txt" {
        return Err(format!("expected match in 'a.txt', got {:?}", results[0].path));
    }
    Ok(())
}

fn test_replace_files_filters_unchanged(ctx: &TestContext) -> Result<(), String> {
    ctx.log("replace-files should omit files left unchanged");
    let files = vec![
        File { path: "a.txt".to_string(), contents: "cat".to_string() },
        File { path: "b.txt".to_string(), contents: "dog".to_string() },
    ];
    let results = bindings::replace_files("cat", "feline", &files, opts())?;
    if results.len() != 1 {
        return Err(format!("expected 1 changed file, got {}", results.len()));
    }
    if results[0].path != "a.txt" || results[0].contents != "feline" {
        return Err(format!(
            "expected 'a.txt' -> 'feline', got {:?} -> {:?}",
            results[0].path, results[0].contents
        ));
    }
    Ok(())
}

fn test_search_match_metadata(ctx: &TestContext) -> Result<(), String> {
    ctx.log("verifying byte offsets locate the match in the haystack");
    let haystack = "the quick brown fox";
    let matches = bindings::search("brown", haystack, opts())?;
    let m = matches.first().ok_or("expected a match")?;
    let start = usize::try_from(m.byte_start).map_err(|e| e.to_string())?;
    let end = usize::try_from(m.byte_end).map_err(|e| e.to_string())?;
    let slice = haystack.get(start..end).ok_or("byte offsets out of range")?;
    if slice != "brown" {
        return Err(format!("byte offsets point at {slice:?}, expected 'brown'"));
    }
    Ok(())
}

wasi_test::suite!(
    test_search_finds_single_match,
    test_search_multiple_lines,
    test_search_case_insensitive,
    test_search_no_match,
    test_search_invalid_regex_errors,
    test_replace_basic,
    test_replace_capture_group,
    test_search_files_filters_empty,
    test_replace_files_filters_unchanged,
    test_search_match_metadata,
);
