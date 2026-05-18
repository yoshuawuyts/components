//! Textsearch WIT component: regex search and replace, powered by the
//! same `grep-*` crates that back `ripgrep`.
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_docs_in_private_items,
    reason = "wit-bindgen generates unsafe FFI glue and undocumented items"
)]

wit_bindgen::generate!({
    world: "textsearch",
    path: "wit",
});

/// The WIT component implementation.
struct Component;

export!(Component);

impl Guest for Component {
    fn search(pattern: String, haystack: String, options: Options) -> Result<Vec<Match>, String> {
        Searcher::new(&pattern, &options)?.run(&haystack)
    }

    fn replace(
        pattern: String,
        replacement: String,
        haystack: String,
        options: Options,
    ) -> Result<String, String> {
        let re = build_regex(&pattern, &options)?;
        Ok(re.replace_all(&haystack, replacement.as_str()).into_owned())
    }

    fn search_files(
        pattern: String,
        files: Vec<File>,
        options: Options,
    ) -> Result<Vec<FileMatches>, String> {
        let searcher = Searcher::new(&pattern, &options)?;
        let mut out = Vec::new();
        for file in files {
            let matches = searcher.run(&file.contents)?;
            if !matches.is_empty() {
                out.push(FileMatches {
                    path: file.path,
                    matches,
                });
            }
        }
        Ok(out)
    }

    fn replace_files(
        pattern: String,
        replacement: String,
        files: Vec<File>,
        options: Options,
    ) -> Result<Vec<FileReplacement>, String> {
        let re = build_regex(&pattern, &options)?;
        let mut out = Vec::new();
        for file in files {
            let replaced = re.replace_all(&file.contents, replacement.as_str());
            // Only emit files whose contents actually changed, so callers
            // can iterate the result without diffing.
            if let std::borrow::Cow::Owned(contents) = replaced {
                out.push(FileReplacement {
                    path: file.path,
                    contents,
                });
            }
        }
        Ok(out)
    }
}

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{Searcher as GrepSearcher, SearcherBuilder, Sink, SinkMatch};
use regex::{Regex, RegexBuilder};
use std::convert::TryFrom;

/// Drives a single search invocation through ripgrep's matcher and searcher.
struct Searcher {
    matcher: RegexMatcher,
    multiline: bool,
}

impl Searcher {
    /// Build a searcher configured for the given pattern and options.
    fn new(pattern: &str, options: &Options) -> Result<Self, String> {
        let matcher = build_matcher(pattern, options)?;
        Ok(Self {
            matcher,
            multiline: options.multiline,
        })
    }

    /// Execute the search against the given haystack and collect all matches.
    fn run(&self, haystack: &str) -> Result<Vec<Match>, String> {
        let mut collector = Collector {
            matcher: &self.matcher,
            results: Vec::new(),
            err: None,
        };
        SearcherBuilder::new()
            .line_number(true)
            .multi_line(self.multiline)
            .build()
            .search_slice(&self.matcher, haystack.as_bytes(), &mut collector)
            .map_err(|e| e.to_string())?;
        if let Some(err) = collector.err {
            return Err(err);
        }
        Ok(collector.results)
    }
}

/// Sink that accumulates matches as they are reported by the searcher.
struct Collector<'a> {
    matcher: &'a RegexMatcher,
    results: Vec<Match>,
    err: Option<String>,
}

impl Sink for Collector<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &GrepSearcher,
        sm: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        let bytes = sm.bytes();
        let trimmed = trim_line_terminator(bytes);
        let line_text = std::str::from_utf8(trimmed)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
            .to_owned();
        let line_number = sm.line_number().unwrap_or(0);

        let matcher = self.matcher;
        let results = &mut self.results;
        let err_slot = &mut self.err;

        matcher
            .find_iter(bytes, |m| {
                let start = m.start();
                let end = m.end();
                let Some(slice) = bytes.get(start..end) else {
                    *err_slot = Some("match offsets out of range".to_owned());
                    return false;
                };
                let text = match std::str::from_utf8(slice) {
                    Ok(s) => s.to_owned(),
                    Err(_) => {
                        *err_slot = Some("non-utf8 match".to_owned());
                        return false;
                    }
                };
                let column_start = u32::try_from(start).unwrap_or(u32::MAX);
                let column_end = u32::try_from(end).unwrap_or(u32::MAX);
                results.push(Match {
                    line_number,
                    line: line_text.clone(),
                    column_start,
                    column_end,
                    text,
                });
                true
            })
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

        Ok(true)
    }
}

/// Build a `grep-regex` matcher that mirrors the requested options.
fn build_matcher(pattern: &str, opts: &Options) -> Result<RegexMatcher, String> {
    let mut builder = RegexMatcherBuilder::new();
    builder
        .case_insensitive(opts.case_insensitive)
        .multi_line(opts.multiline)
        .dot_matches_new_line(opts.multiline)
        .word(opts.word)
        .fixed_strings(opts.fixed_strings);
    builder.build(pattern).map_err(|e| e.to_string())
}

/// Build a `regex::Regex` configured the same way as the grep matcher, for
/// use in [`Guest::replace`].
fn build_regex(pattern: &str, opts: &Options) -> Result<Regex, String> {
    let mut effective = if opts.fixed_strings {
        regex::escape(pattern)
    } else {
        pattern.to_owned()
    };
    if opts.word {
        effective = format!(r"(?:\b(?:{effective})\b)");
    }
    RegexBuilder::new(&effective)
        .case_insensitive(opts.case_insensitive)
        .multi_line(opts.multiline)
        .dot_matches_new_line(opts.multiline)
        .build()
        .map_err(|e| e.to_string())
}

/// Strip a single trailing `\n` and optional preceding `\r` from `bytes`.
fn trim_line_terminator(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    if end > 0 && bytes.get(end - 1).copied() == Some(b'\n') {
        end -= 1;
    }
    if end > 0 && bytes.get(end - 1).copied() == Some(b'\r') {
        end -= 1;
    }
    bytes.get(..end).unwrap_or(bytes)
}
