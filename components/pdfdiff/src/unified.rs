//! Render a line diff in the unified diff format.

use crate::exports::yoshuawuyts::pdfdiff::types::Options;
use crate::extract::Line;
use similar::{Algorithm, DiffOp, DiffTag};
use std::fmt::Write;
use std::ops::Range;

/// Diff `old` against `new` and render the result as a unified diff.
///
/// Returns an empty string when both sides are identical.
pub(crate) fn render(old: &[Line], new: &[Line], options: &Options) -> String {
    let old_text: Vec<&str> = old.iter().map(|line| line.text.as_str()).collect();
    let new_text: Vec<&str> = new.iter().map(|line| line.text.as_str()).collect();
    let ops = similar::capture_diff_slices(Algorithm::Patience, &old_text, &new_text);
    if ops.iter().all(|op| op.tag() == DiffTag::Equal) {
        return String::new();
    }
    let context = usize::try_from(options.context_lines).unwrap_or(usize::MAX);

    let mut out = String::new();
    let _ = writeln!(out, "--- {}", label(&options.old_label));
    let _ = writeln!(out, "+++ {}", label(&options.new_label));
    for group in similar::group_diff_ops(ops, context) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old_range = first.old_range().start..last.old_range().end;
        let new_range = first.new_range().start..last.new_range().end;
        let _ = write!(out, "@@ -{} +{} @@", range(&old_range), range(&new_range));
        if let Some(page) = hunk_page(&group, old, new) {
            let _ = write!(out, " page {page}");
        }
        out.push('\n');
        for op in &group {
            let (tag, old_range, new_range) = op.as_tag_tuple();
            match tag {
                DiffTag::Equal => push_lines(&mut out, ' ', old, old_range),
                DiffTag::Delete => push_lines(&mut out, '-', old, old_range),
                DiffTag::Insert => push_lines(&mut out, '+', new, new_range),
                DiffTag::Replace => {
                    push_lines(&mut out, '-', old, old_range);
                    push_lines(&mut out, '+', new, new_range);
                }
            }
        }
    }
    out
}

/// The page the first change in a hunk is on.
///
/// Prefers the new document's page; falls back to the old document's page
/// for hunks that only remove lines.
fn hunk_page(group: &[DiffOp], old: &[Line], new: &[Line]) -> Option<usize> {
    let op = group.iter().find(|op| op.tag() != DiffTag::Equal)?;
    let (_, old_range, new_range) = op.as_tag_tuple();
    new.get(new_range.start)
        .filter(|_| !new_range.is_empty())
        .or_else(|| old.get(old_range.start))
        .map(|line| line.page)
}

/// Format a hunk range the way GNU `diff -u` does.
fn range(range: &Range<usize>) -> String {
    match range.len() {
        0 => format!("{},0", range.start),
        1 => format!("{}", range.start + 1),
        len => format!("{},{}", range.start + 1, len),
    }
}

/// Write every line in `range` prefixed with `prefix`.
fn push_lines(out: &mut String, prefix: char, lines: &[Line], range: Range<usize>) {
    for line in lines.get(range).unwrap_or_default() {
        out.push(prefix);
        out.push_str(&line.text);
        out.push('\n');
    }
}

/// Keep a header label on a single line.
fn label(label: &str) -> String {
    label.replace(['\r', '\n'], " ")
}
