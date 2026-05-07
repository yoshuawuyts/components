//! Feedmark WIT component: parse RSS, Atom and JSON Feed documents.
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_docs_in_private_items,
    reason = "wit-bindgen generates unsafe FFI glue and undocumented items"
)]

wit_bindgen::generate!({
    world: "feedmark",
    path: "wit",
});

/// The WIT component implementation.
struct Component;

export!(Component);

impl Guest for Component {
    fn parse(input: String) -> Result<Feed, String> {
        Parser::new(&input).parse()
    }

    fn to_markdown(input: String) -> Result<String, String> {
        let feed = Parser::new(&input).parse()?;
        Ok(render_markdown(&feed))
    }
}

use feed_rs::model;

/// Parses RSS, Atom and JSON Feed documents into a normalized [`Feed`].
///
/// The underlying [`feed_rs`] parser auto-detects the input format,
/// so callers do not need to know which syndication format they have
/// before calling [`Parser::parse`].
struct Parser<'a> {
    input: &'a str,
}

impl<'a> Parser<'a> {
    /// Create a new parser for the given feed source string.
    fn new(input: &'a str) -> Self {
        Self { input }
    }

    /// Parse the input into a normalized [`Feed`].
    fn parse(self) -> Result<Feed, String> {
        let parsed = feed_rs::parser::parse(self.input.as_bytes()).map_err(|e| e.to_string())?;
        Ok(normalize_feed(parsed))
    }
}

/// Convert a [`feed_rs::model::Feed`] into the WIT-facing [`Feed`] record.
fn normalize_feed(feed: model::Feed) -> Feed {
    let title = feed.title.map(|t| t.content).unwrap_or_default();
    let description = feed.description.map(|t| t.content);
    let link = first_link(&feed.links);
    let entries = feed.entries.into_iter().map(normalize_entry).collect();
    Feed {
        title,
        description,
        link,
        entries,
    }
}

/// Convert a [`feed_rs::model::Entry`] into the WIT-facing [`Entry`] record.
fn normalize_entry(entry: model::Entry) -> Entry {
    let title = entry.title.map(|t| t.content).unwrap_or_default();
    let link = first_link(&entry.links);
    let summary = entry.summary.map(|t| t.content);
    let content = entry.content.and_then(|c| c.body);
    let published = entry.published.or(entry.updated).map(|dt| dt.to_rfc3339());
    let author = entry.authors.into_iter().next().map(|p| p.name);
    Entry {
        title,
        link,
        summary,
        content,
        published,
        author,
    }
}

/// Return the first link's `href`, if any.
fn first_link(links: &[model::Link]) -> Option<String> {
    links.first().map(|l| l.href.clone())
}

/// Render a normalized [`Feed`] as a Markdown digest.
///
/// The digest contains a level-1 heading with the feed title, an optional
/// description paragraph, and a level-2 entry for each item with its link,
/// publication date, author and summary, in that order.
fn render_markdown(feed: &Feed) -> String {
    let mut out = String::new();

    let title = if feed.title.is_empty() {
        "Feed"
    } else {
        feed.title.as_str()
    };
    out.push_str("# ");
    out.push_str(title);
    out.push_str("\n\n");

    if let Some(description) = &feed.description {
        if !description.is_empty() {
            out.push_str(description);
            out.push_str("\n\n");
        }
    }

    for entry in &feed.entries {
        let entry_title = if entry.title.is_empty() {
            "(untitled)"
        } else {
            entry.title.as_str()
        };
        out.push_str("## ");
        match &entry.link {
            Some(link) if !link.is_empty() => {
                out.push('[');
                out.push_str(entry_title);
                out.push_str("](");
                out.push_str(link);
                out.push(')');
            }
            _ => out.push_str(entry_title),
        }
        out.push_str("\n\n");

        let mut meta: Vec<String> = Vec::new();
        if let Some(published) = &entry.published {
            meta.push(published.clone());
        }
        if let Some(author) = &entry.author {
            if !author.is_empty() {
                meta.push(format!("by {author}"));
            }
        }
        if !meta.is_empty() {
            out.push('*');
            out.push_str(&meta.join(" — "));
            out.push_str("*\n\n");
        }

        if let Some(summary) = entry.summary.as_ref().or(entry.content.as_ref()) {
            if !summary.is_empty() {
                out.push_str(summary);
                out.push_str("\n\n");
            }
        }
    }

    // Trim trailing blank lines so the output ends with a single newline.
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}
