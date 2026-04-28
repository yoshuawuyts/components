//! Wordmark WIT component: convert Markdown to a `.docx` document.
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_docs_in_private_items,
    reason = "wit-bindgen generates unsafe FFI glue and undocumented items"
)]

wit_bindgen::generate!({
    world: "wordmark",
    path: "wit",
});

/// The WIT component implementation.
struct Component;

export!(Component);

impl Guest for Component {
    fn to_word(markdown: String) -> Result<Vec<u8>, String> {
        let doc = Converter::new(&markdown).convert()?;
        Ok(doc)
    }
}

use docx_rs::*;
use pulldown_cmark::HeadingLevel;

/// Converts markdown to a Word (.docx) document.
///
/// Walks a stream of pulldown-cmark events and builds a `Docx` document
/// by mapping markdown constructs to Word paragraph styles and runs.
struct Converter {
    markdown: String,
    docx: Docx,
    runs: Vec<Run>,
    bold: bool,
    italic: bool,
    heading_level: Option<HeadingLevel>,
    in_code_block: bool,
    code_text: String,
}

impl Converter {
    /// Create a new converter for the given markdown string.
    fn new(markdown: &str) -> Self {
        Self {
            docx: Docx::new(),
            runs: Vec::new(),
            bold: false,
            italic: false,
            heading_level: None,
            in_code_block: false,
            code_text: String::new(),
            markdown: markdown.to_owned(),
        }
    }

    /// Convert the markdown into a `.docx` byte buffer.
    fn convert(mut self) -> Result<Vec<u8>, String> {
        use pulldown_cmark::{Event, Parser, Tag, TagEnd};
        use std::io::Cursor;

        let markdown = std::mem::take(&mut self.markdown);
        let parser = Parser::new(&markdown);

        for event in parser {
            match event {
                Event::Start(Tag::Heading { level, .. }) => self.start_heading(level),
                Event::End(TagEnd::Heading(_)) => self.end_heading(),
                Event::Start(Tag::Paragraph) => self.start_paragraph(),
                Event::End(TagEnd::Paragraph) => self.end_paragraph(),
                Event::Start(Tag::Strong) => self.start_strong(),
                Event::End(TagEnd::Strong) => self.end_strong(),
                Event::Start(Tag::Emphasis) => self.start_emphasis(),
                Event::End(TagEnd::Emphasis) => self.end_emphasis(),
                Event::Start(Tag::CodeBlock(_)) => self.start_code_block(),
                Event::End(TagEnd::CodeBlock) => self.end_code_block(),
                Event::Start(Tag::Item) => self.start_item(),
                Event::End(TagEnd::Item) => self.end_item(),
                Event::Text(text) => self.push_text(&text),
                Event::Code(text) => self.push_code(&text),
                Event::SoftBreak | Event::HardBreak => self.push_break(),
                _ => {}
            }
        }

        let mut buf = Cursor::new(Vec::new());
        self.docx
            .build()
            .pack(&mut buf)
            .map_err(|e| e.to_string())?;
        Ok(buf.into_inner())
    }

    /// Record the current heading level.
    fn start_heading(&mut self, level: HeadingLevel) {
        self.heading_level = Some(level);
    }

    /// Flush accumulated runs into a heading paragraph.
    fn end_heading(&mut self) {
        let style = match self.heading_level {
            Some(HeadingLevel::H1) => "Heading1",
            Some(HeadingLevel::H2) => "Heading2",
            Some(HeadingLevel::H3) => "Heading3",
            Some(HeadingLevel::H4) => "Heading4",
            Some(HeadingLevel::H5) => "Heading5",
            Some(HeadingLevel::H6) => "Heading6",
            None => "Normal",
        };
        let para = self.flush_runs(Paragraph::new().style(style));
        self.add_paragraph(para);
        self.heading_level = None;
    }

    /// Clear runs for a new paragraph.
    fn start_paragraph(&mut self) {
        self.runs.clear();
    }

    /// Flush accumulated runs into a normal paragraph.
    fn end_paragraph(&mut self) {
        let para = self.flush_runs(Paragraph::new());
        self.add_paragraph(para);
    }

    /// Enable bold formatting for subsequent runs.
    fn start_strong(&mut self) {
        self.bold = true;
    }

    /// Disable bold formatting.
    fn end_strong(&mut self) {
        self.bold = false;
    }

    /// Enable italic formatting for subsequent runs.
    fn start_emphasis(&mut self) {
        self.italic = true;
    }

    /// Disable italic formatting.
    fn end_emphasis(&mut self) {
        self.italic = false;
    }

    /// Enter a fenced code block and reset the code buffer.
    fn start_code_block(&mut self) {
        self.in_code_block = true;
        self.code_text.clear();
    }

    /// Flush the code buffer into a monospace paragraph.
    fn end_code_block(&mut self) {
        self.in_code_block = false;
        let run = Run::new()
            .add_text(&self.code_text)
            .fonts(RunFonts::new().ascii("Courier New"));
        let para = Paragraph::new().add_run(run);
        self.code_text.clear();
        self.add_paragraph(para);
    }

    /// Clear runs for a new list item.
    fn start_item(&mut self) {
        self.runs.clear();
    }

    /// Flush accumulated runs into a list item paragraph.
    fn end_item(&mut self) {
        let para = self.flush_runs(Paragraph::new().style("ListParagraph"));
        self.add_paragraph(para);
    }

    /// Append text as a run, or into the code buffer if inside a code block.
    fn push_text(&mut self, text: &str) {
        if self.in_code_block {
            self.code_text.push_str(text);
        } else {
            let mut run = Run::new().add_text(text);
            if self.bold {
                run = run.bold();
            }
            if self.italic {
                run = run.italic();
            }
            self.runs.push(run);
        }
    }

    /// Append inline code as a monospace run.
    fn push_code(&mut self, text: &str) {
        let run = Run::new()
            .add_text(text)
            .fonts(RunFonts::new().ascii("Courier New"));
        self.runs.push(run);
    }

    /// Append a line break run.
    fn push_break(&mut self) {
        self.runs
            .push(Run::new().add_break(BreakType::TextWrapping));
    }

    /// Drain all accumulated runs into a paragraph.
    fn flush_runs(&mut self, mut para: Paragraph) -> Paragraph {
        for run in self.runs.drain(..) {
            para = para.add_run(run);
        }
        para
    }

    /// Add a paragraph to the document.
    fn add_paragraph(&mut self, para: Paragraph) {
        self.docx = std::mem::take(&mut self.docx).add_paragraph(para);
    }
}
