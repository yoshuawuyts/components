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
        // Build a `Searcher` so we can attach the same match metadata
        // (positions / context) to the replacement result, and a separate
        // `Regex` to actually perform the substitution (so capture-group
        // references in `replacement` keep `rg --replace` semantics).
        let searcher = Searcher::new(&pattern, &options)?;
        let re = build_regex(&pattern, &options)?;
        let mut out = Vec::new();
        for file in files {
            let replaced = re.replace_all(&file.contents, replacement.as_str());
            // Only emit files whose contents actually changed, so callers
            // can iterate the result without diffing. The symmetry with
            // `search-files` (which omits files with no matches) is
            // documented in `world.wit`.
            if let std::borrow::Cow::Owned(contents) = replaced {
                let matches = searcher.run(&file.contents)?;
                out.push(FileReplacement {
                    path: file.path,
                    contents,
                    matches,
                });
            }
        }
        Ok(out)
    }
}

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{
    Searcher as GrepSearcher, SearcherBuilder, Sink, SinkContext, SinkContextKind, SinkMatch,
};
use regex::{Regex, RegexBuilder};
use std::convert::TryFrom;

/// Drives a single search invocation through ripgrep's matcher and searcher.
struct Searcher {
    matcher: RegexMatcher,
    multiline: bool,
    before_context: usize,
    after_context: usize,
    max_count: Option<usize>,
}

impl Searcher {
    /// Build a searcher configured for the given pattern and options.
    fn new(pattern: &str, options: &Options) -> Result<Self, String> {
        let matcher = build_matcher(pattern, options)?;
        Ok(Self {
            matcher,
            multiline: options.multiline,
            before_context: options.before_context as usize,
            after_context: options.after_context as usize,
            max_count: (options.max_count > 0).then_some(options.max_count as usize),
        })
    }

    /// Execute the search against the given haystack and collect all matches.
    fn run(&self, haystack: &str) -> Result<Vec<Match>, String> {
        let mut collector = Collector {
            matcher: &self.matcher,
            max_count: self.max_count,
            results: Vec::new(),
            pending_before: Vec::new(),
            last_match_first_line: None,
            err: None,
        };
        SearcherBuilder::new()
            .line_number(true)
            .multi_line(self.multiline)
            .before_context(self.before_context)
            .after_context(self.after_context)
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
    max_count: Option<usize>,
    results: Vec<Match>,
    /// Context lines reported via `Sink::context` that arrived *before*
    /// the next match — drained into that match's `before-context`.
    pending_before: Vec<ContextLine>,
    /// Index of the first sub-match emitted for the most recently
    /// reported matched region. Subsequent `Sink::context` calls of
    /// kind `After` append to that match's `after-context` until the
    /// next `matched()` arrives.
    last_match_first_line: Option<usize>,
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
        let absolute_offset = sm.absolute_byte_offset();

        let matcher = self.matcher;
        let results = &mut self.results;
        let err_slot = &mut self.err;
        let max_count = self.max_count;
        let pending_before = std::mem::take(&mut self.pending_before);
        let first_emitted_idx = results.len();
        let mut pending_before = Some(pending_before);
        let mut limit_reached = false;

        matcher
            .find_iter(bytes, |m| {
                if let Some(limit) = max_count {
                    if results.len() >= limit {
                        limit_reached = true;
                        return false;
                    }
                }
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
                let byte_start =
                    absolute_offset.saturating_add(u64::try_from(start).unwrap_or(u64::MAX));
                let byte_end =
                    absolute_offset.saturating_add(u64::try_from(end).unwrap_or(u64::MAX));
                let extra_lines = u64::try_from(count_newlines(slice)).unwrap_or(0);
                let line_end = line_number.saturating_add(extra_lines);
                let before = pending_before.take().unwrap_or_default();
                results.push(Match {
                    line_number,
                    line_end,
                    line: line_text.clone(),
                    column_start,
                    column_end,
                    byte_start,
                    byte_end,
                    text,
                    before_context: before,
                    after_context: Vec::new(),
                });
                true
            })
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        if let Some(unused) = pending_before {
            // No sub-matches were emitted — keep the pending context for
            // whichever match comes next.
            self.pending_before = unused;
        }
        if results.len() > first_emitted_idx {
            self.last_match_first_line = Some(first_emitted_idx);
        }

        if let Some(limit) = max_count {
            if results.len() >= limit || limit_reached {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &GrepSearcher,
        ctx: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        let trimmed = trim_line_terminator(ctx.bytes());
        let line_text = std::str::from_utf8(trimmed)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
            .to_owned();
        let line_number = ctx.line_number().unwrap_or(0);
        let context_line = ContextLine {
            line_number,
            line: line_text,
        };
        match ctx.kind() {
            SinkContextKind::Before => {
                self.pending_before.push(context_line);
            }
            SinkContextKind::After => {
                if let Some(idx) = self.last_match_first_line {
                    if let Some(m) = self.results.get_mut(idx) {
                        m.after_context.push(context_line);
                    }
                }
            }
            SinkContextKind::Other => {}
        }
        Ok(true)
    }
}

/// Build a `grep-regex` matcher that mirrors the requested options.
fn build_matcher(pattern: &str, opts: &Options) -> Result<RegexMatcher, String> {
    let case_insensitive = opts.case_insensitive || smart_case_active(pattern, opts);
    let mut builder = RegexMatcherBuilder::new();
    builder
        .case_insensitive(case_insensitive)
        .multi_line(opts.multiline)
        .dot_matches_new_line(opts.multiline)
        .word(opts.word)
        .fixed_strings(opts.fixed_strings);
    builder.build(pattern).map_err(|e| e.to_string())
}

/// Build a `regex::Regex` configured the same way as the grep matcher, for
/// use in [`Component::replace`] and [`Component::replace_files`].
fn build_regex(pattern: &str, opts: &Options) -> Result<Regex, String> {
    let case_insensitive = opts.case_insensitive || smart_case_active(pattern, opts);
    let mut effective = if opts.fixed_strings {
        regex::escape(pattern)
    } else {
        pattern.to_owned()
    };
    if opts.word {
        effective = format!(r"(?:\b(?:{effective})\b)");
    }
    RegexBuilder::new(&effective)
        .case_insensitive(case_insensitive)
        .multi_line(opts.multiline)
        .dot_matches_new_line(opts.multiline)
        .build()
        .map_err(|e| e.to_string())
}

/// `true` if `smart-case` was requested and the pattern contains no
/// uppercase characters outside of escapes (`\D`, `\S`, …) — mirroring
/// ripgrep's `--smart-case` heuristic.
fn smart_case_active(pattern: &str, opts: &Options) -> bool {
    if !opts.smart_case || opts.case_insensitive {
        return false;
    }
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // Skip the escaped character — `\D` etc. shouldn't disable
            // smart-case the way a literal `D` does.
            chars.next();
            continue;
        }
        if c.is_uppercase() {
            return false;
        }
    }
    true
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

/// Count `\n` bytes in `bytes`.
fn count_newlines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b == b'\n').count()
}
