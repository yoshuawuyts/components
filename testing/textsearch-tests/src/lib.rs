//! Test suite for the `textsearch` component, authored as a Wasm component.
//!
//! This crate compiles to a `wasm32-wasip2` component that exports
//! `wasi:test/tests` and imports the surface of the `textsearch` component.
//! Composed with the real `textsearch.wasm` and the generic test runner via
//! `wac plug`, the tests below run against the actual component artifact —
//! exercising the WIT boundary and the `wit-bindgen` ABI glue, not just
//! native Rust.
//!
//! See `testing/README.md` for the full workflow.

wit_bindgen::generate!({
    world: "textsearch-tests",
    path: "wit",
    generate_all,
});

use exports::wasi::test::tests::{Guest, TestOutcome};

struct Suite;
export!(Suite);

/// Default search options (everything off / unlimited), mirroring an
/// all-`false`/`0` `options` record.
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

/// Run a single named test and capture its outcome.
fn case(name: &str, body: impl FnOnce() -> Result<(), String>) -> TestOutcome {
    TestOutcome {
        name: name.to_string(),
        outcome: body(),
    }
}

/// Assert helper that produces a descriptive `Err` on failure.
fn check(cond: bool, msg: impl FnOnce() -> String) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(msg())
    }
}

impl Guest for Suite {
    fn run_all() -> Vec<TestOutcome> {
        vec![
            case("search-finds-single-match", || {
                let matches = search("world".into(), "hello world".into(), opts())?;
                check(matches.len() == 1, || format!("expected 1 match, got {}", matches.len()))?;
                let m = &matches[0];
                check(m.text == "world", || format!("text was {:?}", m.text))?;
                check(m.line == "hello world", || format!("line was {:?}", m.line))?;
                check(m.line_number == 1, || format!("line_number was {}", m.line_number))?;
                check(
                    m.column_start == 6 && m.column_end == 11,
                    || format!("columns were {}..{}", m.column_start, m.column_end),
                )
            }),
            case("search-no-match-is-empty", || {
                let matches = search("zzz".into(), "hello world".into(), opts())?;
                check(matches.is_empty(), || format!("expected no matches, got {}", matches.len()))
            }),
            case("search-case-insensitive", || {
                let mut o = opts();
                o.case_insensitive = true;
                let matches = search("WORLD".into(), "hello world".into(), o)?;
                check(matches.len() == 1, || format!("expected 1 match, got {}", matches.len()))
            }),
            case("search-smart-case-lowercase-pattern", || {
                // A lowercase pattern with smart-case enabled matches case-insensitively.
                let mut o = opts();
                o.smart_case = true;
                let matches = search("hello".into(), "HELLO WORLD".into(), o)?;
                check(matches.len() == 1, || format!("expected 1 match, got {}", matches.len()))
            }),
            case("search-word-boundary", || {
                let mut o = opts();
                o.word = true;
                let matches = search("cat".into(), "cat category cats".into(), o)?;
                check(matches.len() == 1, || format!("expected 1 whole-word match, got {}", matches.len()))
            }),
            case("search-fixed-strings-literal", || {
                let mut o = opts();
                o.fixed_strings = true;
                let matches = search("a.b".into(), "a.b axb".into(), o)?;
                check(matches.len() == 1, || format!("expected 1 literal match, got {}", matches.len()))?;
                check(matches[0].text == "a.b", || format!("text was {:?}", matches[0].text))
            }),
            case("search-max-count-limits-results", || {
                let mut o = opts();
                o.max_count = 2;
                let matches = search("a".into(), "a a a a".into(), o)?;
                check(matches.len() == 2, || format!("expected 2 (capped) matches, got {}", matches.len()))
            }),
            case("search-context-lines", || {
                let mut o = opts();
                o.before_context = 1;
                o.after_context = 1;
                let matches = search("l3".into(), "l1\nl2\nl3\nl4".into(), o)?;
                check(matches.len() == 1, || format!("expected 1 match, got {}", matches.len()))?;
                let m = &matches[0];
                check(
                    m.before_context.len() == 1 && m.before_context[0].line == "l2",
                    || format!("before-context was {:?}", m.before_context),
                )?;
                check(
                    m.after_context.len() == 1 && m.after_context[0].line == "l4",
                    || format!("after-context was {:?}", m.after_context),
                )
            }),
            case("search-multiline-dot-matches-newline", || {
                let mut o = opts();
                o.multiline = true;
                let matches = search("foo.bar".into(), "foo\nbar".into(), o)?;
                check(matches.len() == 1, || format!("expected 1 match, got {}", matches.len()))?;
                check(matches[0].text == "foo\nbar", || format!("text was {:?}", matches[0].text))
            }),
            case("replace-basic", || {
                let got = replace("world".into(), "there".into(), "hello world".into(), opts())?;
                check(got == "hello there", || format!("got {got:?}"))
            }),
            case("replace-capture-groups", || {
                let got = replace(
                    r"(\w+)\s(\w+)".into(),
                    "$2 $1".into(),
                    "hello world".into(),
                    opts(),
                )?;
                check(got == "world hello", || format!("got {got:?}"))
            }),
            case("replace-no-match-unchanged", || {
                let got = replace("zzz".into(), "y".into(), "hello world".into(), opts())?;
                check(got == "hello world", || format!("got {got:?}"))
            }),
            case("search-files-omits-files-without-matches", || {
                let files = vec![
                    File { path: "a.txt".into(), contents: "foo".into() },
                    File { path: "b.txt".into(), contents: "bar".into() },
                ];
                let results = search_files("foo".into(), &files, opts())?;
                check(results.len() == 1, || format!("expected 1 file, got {}", results.len()))?;
                check(results[0].path == "a.txt", || format!("path was {:?}", results[0].path))
            }),
            case("replace-files-only-changed-files", || {
                let files = vec![
                    File { path: "a.txt".into(), contents: "foo".into() },
                    File { path: "b.txt".into(), contents: "bar".into() },
                ];
                let results = replace_files("foo".into(), "baz".into(), &files, opts())?;
                check(results.len() == 1, || format!("expected 1 changed file, got {}", results.len()))?;
                check(results[0].path == "a.txt", || format!("path was {:?}", results[0].path))?;
                check(results[0].contents == "baz", || format!("contents were {:?}", results[0].contents))
            }),
            case("invalid-regex-returns-error", || {
                // An unbalanced group is a regex error; the component should
                // surface it as `err`, not trap.
                match search("(".into(), "anything".into(), opts()) {
                    Err(_) => Ok(()),
                    Ok(_) => Err("expected an error for an invalid pattern".into()),
                }
            }),
        ]
    }
}
