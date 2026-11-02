//! Extract and normalize the text of a PDF.

/// A single line of extracted text, tagged with the page it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    /// The text of the line, without a line terminator.
    pub(crate) text: String,
    /// The 1-based page number the line is on.
    pub(crate) page: usize,
}

/// Extract the text of every page of `pdf`, in page order.
///
/// Line endings are normalized to `\n`, trailing whitespace is trimmed from
/// every line, and leading and trailing blank lines are removed from every
/// page.
pub(crate) fn pages(pdf: &[u8]) -> Result<Vec<String>, String> {
    let pages = pdf_extract::extract_text_from_mem_by_pages(pdf).map_err(|e| e.to_string())?;
    Ok(pages.iter().map(|page| normalize_page(page)).collect())
}

/// Normalize the text of a single page.
fn normalize_page(page: &str) -> String {
    let page = page.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = page.split('\n').map(str::trim_end).collect();
    lines.join("\n").trim_matches('\n').to_string()
}

/// Split extracted pages into lines.
///
/// When `ignore_whitespace` is set, runs of whitespace within a line are
/// collapsed into a single space, leading whitespace is dropped, and blank
/// lines are skipped.
pub(crate) fn lines(pages: &[String], ignore_whitespace: bool) -> Vec<Line> {
    let mut out = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        if page.is_empty() {
            continue;
        }
        for line in page.split('\n') {
            let text = if ignore_whitespace {
                line.split_whitespace().collect::<Vec<_>>().join(" ")
            } else {
                line.to_string()
            };
            if ignore_whitespace && text.is_empty() {
                continue;
            }
            out.push(Line {
                text,
                page: index + 1,
            });
        }
    }
    out
}
