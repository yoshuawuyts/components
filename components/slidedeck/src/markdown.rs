//! Converting Markdown documents into decks.

use crate::types::{
    BulletsLayout, CodeLayout, ColumnsLayout, Deck, Layout, QuoteLayout, SectionLayout, Slide,
    TableLayout, TextLayout, Theme, TitleLayout,
};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// A block of slide content.
#[derive(Debug)]
enum Block {
    Paragraph(String),
    List(Vec<String>),
    Quote(Vec<String>),
    Code(String),
    Table(Vec<String>, Vec<Vec<String>>),
}

/// A slide being collected.
#[derive(Debug, Default)]
struct Draft {
    title: Option<String>,
    level: Option<HeadingLevel>,
    blocks: Vec<Block>,
    notes: Vec<String>,
}

impl Draft {
    fn is_empty(&self) -> bool {
        self.title.is_none() && self.blocks.is_empty() && self.notes.is_empty()
    }
}

/// Where inline text is currently going.
#[derive(Debug, Default)]
struct Collector {
    /// Inline text of the innermost open block.
    text: String,
    /// Open list items, innermost last; flattened when they close.
    items: Vec<Vec<String>>,
    /// Nesting depth of lists.
    list_depth: usize,
    /// Paragraphs of an open block quote.
    quote: Option<Vec<String>>,
    /// Nesting depth of block quotes.
    quote_depth: usize,
    /// Cells of the table being read.
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    in_head: bool,
    /// Nesting depth of images, whose alt text is skipped.
    image: usize,
}

/// Build a deck from a Markdown document.
pub(crate) fn parse(markdown: &str, theme: Theme) -> Result<Deck, String> {
    let mut drafts: Vec<Draft> = Vec::new();
    let mut draft = Draft::default();
    let mut c = Collector::default();
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);

    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                if !draft.is_empty() {
                    drafts.push(std::mem::take(&mut draft));
                }
                draft.level = Some(level);
                c.text.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                draft.title = Some(take_text(&mut c.text));
            }
            Event::Rule => {
                if !draft.is_empty() {
                    drafts.push(std::mem::take(&mut draft));
                }
            }
            Event::Start(Tag::Paragraph | Tag::CodeBlock(_) | Tag::TableCell) => c.text.clear(),
            Event::End(TagEnd::Paragraph) => {
                let text = take_text(&mut c.text);
                if text.is_empty() {
                } else if let Some(item) = c.items.last_mut() {
                    item.push(text);
                } else if let Some(quote) = &mut c.quote {
                    quote.push(text);
                } else {
                    draft.blocks.push(Block::Paragraph(text));
                }
            }
            Event::Start(Tag::List(_)) => {
                // A nested list ends the parent item's own text.
                flush_item_text(&mut c);
                c.list_depth += 1;
                if c.items.is_empty() {
                    c.items.push(Vec::new());
                }
            }
            Event::End(TagEnd::List(_)) => {
                c.list_depth = c.list_depth.saturating_sub(1);
                if c.list_depth == 0 {
                    let items = c.items.pop().unwrap_or_default();
                    if !items.is_empty() {
                        draft.blocks.push(Block::List(items));
                    }
                }
            }
            Event::Start(Tag::Item) => {
                flush_item_text(&mut c);
            }
            Event::End(TagEnd::Item) => flush_item_text(&mut c),
            Event::Start(Tag::BlockQuote(_)) => {
                c.quote_depth += 1;
                if c.quote.is_none() {
                    c.quote = Some(Vec::new());
                }
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                c.quote_depth = c.quote_depth.saturating_sub(1);
                if c.quote_depth == 0 {
                    if let Some(quote) = c.quote.take().filter(|q| !q.is_empty()) {
                        if let Some(item) = c.items.last_mut() {
                            item.extend(quote);
                        } else {
                            draft.blocks.push(Block::Quote(quote));
                        }
                    }
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                let code = std::mem::take(&mut c.text);
                let code = code.trim_end_matches('\n').to_owned();
                if !code.is_empty() {
                    draft.blocks.push(Block::Code(code));
                }
            }
            Event::Start(Tag::TableHead) => c.in_head = true,
            Event::End(TagEnd::TableHead) => {
                c.in_head = false;
                c.header = std::mem::take(&mut c.row);
            }
            Event::End(TagEnd::TableRow) => {
                let row = std::mem::take(&mut c.row);
                c.rows.push(row);
            }
            Event::End(TagEnd::TableCell) => {
                let cell = take_text(&mut c.text);
                c.row.push(cell);
            }
            Event::End(TagEnd::Table) => {
                let header = std::mem::take(&mut c.header);
                let rows = std::mem::take(&mut c.rows);
                draft.blocks.push(Block::Table(header, rows));
            }
            Event::Start(Tag::Image { .. }) => c.image += 1,
            Event::End(TagEnd::Image) => c.image = c.image.saturating_sub(1),
            Event::Text(text) | Event::Code(text) => {
                if c.image == 0 {
                    c.text.push_str(&text);
                }
            }
            Event::SoftBreak => c.text.push(' '),
            Event::HardBreak => c.text.push('\n'),
            Event::Html(html) | Event::InlineHtml(html) => {
                collect_comments(&html, &mut draft.notes);
            }
            _ => {}
        }
    }
    if !draft.is_empty() {
        drafts.push(draft);
    }

    let title = drafts
        .iter()
        .find(|d| d.level == Some(HeadingLevel::H1))
        .and_then(|d| d.title.clone());
    let mut slides = Vec::new();
    for draft in drafts {
        let first = slides.is_empty();
        convert(draft, first, &mut slides);
    }
    if slides.is_empty() {
        return Err("markdown document has no content to turn into slides".to_owned());
    }
    Ok(Deck {
        theme,
        title,
        author: None,
        slides,
        slide_numbers: true,
        footer: None,
    })
}

fn take_text(text: &mut String) -> String {
    let out = text.trim().to_owned();
    text.clear();
    out
}

/// Move the inline text of an open list item into the item list.
fn flush_item_text(c: &mut Collector) {
    let text = take_text(&mut c.text);
    if !text.is_empty() {
        if let Some(items) = c.items.last_mut() {
            items.push(text);
        }
    }
}

/// Collect the contents of `<!-- ... -->` comments in `html` as notes.
fn collect_comments(html: &str, notes: &mut Vec<String>) {
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        let after = rest.get(start + 4..).unwrap_or_default();
        let (comment, next) = match after.find("-->") {
            Some(end) => (
                after.get(..end).unwrap_or_default(),
                after.get(end + 3..).unwrap_or_default(),
            ),
            None => (after, ""),
        };
        let comment = comment.trim();
        if !comment.is_empty() {
            notes.push(comment.to_owned());
        }
        rest = next;
    }
}

fn slide(layout: Layout) -> Slide {
    Slide {
        layout,
        shapes: Vec::new(),
        notes: None,
        background: None,
        transition: None,
    }
}

/// Turn a draft into one or more slides.
fn convert(draft: Draft, first: bool, slides: &mut Vec<Slide>) {
    let start = slides.len();
    let title = draft.title.unwrap_or_default();
    let mut blocks = draft.blocks.into_iter().peekable();

    if draft.level == Some(HeadingLevel::H1) || (blocks.peek().is_none() && !title.is_empty()) {
        let subtitle = match blocks.peek() {
            Some(Block::Paragraph(_)) => match blocks.next() {
                Some(Block::Paragraph(text)) => Some(text),
                _ => None,
            },
            _ => None,
        };
        let layout = if first && draft.level == Some(HeadingLevel::H1) {
            Layout::Title(TitleLayout {
                title: title.clone(),
                subtitle,
            })
        } else {
            Layout::Section(SectionLayout {
                title: title.clone(),
                subtitle,
            })
        };
        slides.push(slide(layout));
    }

    // Paragraphs and lists share a slide; quotes, code, and tables get
    // their own.
    let mut text: Vec<Block> = Vec::new();
    for block in blocks {
        match block {
            Block::Paragraph(_) | Block::List(_) => text.push(block),
            Block::Quote(lines) => {
                flush_text(&title, &mut text, slides);
                slides.push(slide(quote(lines)));
            }
            Block::Code(code) => {
                flush_text(&title, &mut text, slides);
                slides.push(slide(Layout::Code(CodeLayout {
                    title: title.clone(),
                    code,
                })));
            }
            Block::Table(headers, rows) => {
                flush_text(&title, &mut text, slides);
                slides.push(slide(Layout::Table(TableLayout {
                    title: title.clone(),
                    headers,
                    rows,
                })));
            }
        }
    }
    flush_text(&title, &mut text, slides);

    if !draft.notes.is_empty() {
        if slides.len() == start {
            // Notes with nothing else still need somewhere to live.
            slides.push(slide(Layout::Blank));
        }
        if let Some(slide) = slides.get_mut(start) {
            slide.notes = Some(draft.notes.join("\n\n"));
        }
    }
}

/// Emit the pending paragraphs and lists as one slide.
fn flush_text(title: &str, text: &mut Vec<Block>, slides: &mut Vec<Slide>) {
    if text.is_empty() {
        return;
    }
    let blocks = std::mem::take(text);
    let lists: Vec<&Vec<String>> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::List(items) => Some(items),
            _ => None,
        })
        .collect();
    let layout = if let ([left, right], 2) = (lists.as_slice(), blocks.len()) {
        Layout::Columns(ColumnsLayout {
            title: title.to_owned(),
            left: (*left).clone(),
            right: (*right).clone(),
        })
    } else if lists.is_empty() {
        let paragraphs = blocks
            .into_iter()
            .filter_map(|b| match b {
                Block::Paragraph(p) => Some(p),
                _ => None,
            })
            .collect();
        Layout::Text(TextLayout {
            title: title.to_owned(),
            paragraphs,
        })
    } else {
        let items = blocks
            .into_iter()
            .flat_map(|b| match b {
                Block::Paragraph(p) => vec![p],
                Block::List(items) => items,
                _ => Vec::new(),
            })
            .collect();
        Layout::Bullets(BulletsLayout {
            title: title.to_owned(),
            items,
        })
    };
    slides.push(slide(layout));
}

/// A quote layout; a final line starting with a dash is the attribution.
fn quote(mut lines: Vec<String>) -> Layout {
    let author = match lines.last() {
        Some(last) if lines.len() > 1 => ["\u{2014}", "\u{2013}", "--", "-"]
            .iter()
            .find_map(|dash| last.strip_prefix(dash))
            .map(|author| author.trim().to_owned()),
        _ => None,
    };
    if author.is_some() {
        lines.pop();
    }
    Layout::Quote(QuoteLayout {
        quote: lines.join("\n"),
        author,
        role: None,
    })
}
