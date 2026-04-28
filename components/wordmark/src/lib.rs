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

    fn to_markdown(docx: Vec<u8>) -> Result<String, String> {
        Inverter::new(&docx).convert()
    }
}

use docx_rs::{
    read_docx, BreakType, DocumentChild, Docx, Paragraph, ParagraphChild, Run, RunChild, RunFonts,
};
use pulldown_cmark::HeadingLevel;

/// Paragraph style applied to fenced code blocks. Used by both the writer
/// (so the style is preserved in the docx) and the reader (so we can detect
/// the construct on the way back).
const CODE_BLOCK_STYLE: &str = "Code";

/// Run style applied to inline code spans, for the same reason.
const CODE_RUN_STYLE: &str = "CodeChar";

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
            .style(CODE_RUN_STYLE)
            .fonts(RunFonts::new().ascii("Courier New"));
        let para = Paragraph::new().style(CODE_BLOCK_STYLE).add_run(run);
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
            .style(CODE_RUN_STYLE)
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

/// Converts a Word (.docx) document to markdown.
///
/// Walks the parsed document tree and maps Word paragraphs, runs, and
/// inline formatting back to markdown constructs. Code spans and fenced
/// blocks are detected via the `CodeChar` run style and `Code` paragraph
/// style emitted by [`Converter`].
struct Inverter<'a> {
    docx: &'a [u8],
    out: String,
}

impl<'a> Inverter<'a> {
    /// Create a new inverter for the given docx bytes.
    fn new(docx: &'a [u8]) -> Self {
        Self {
            docx,
            out: String::new(),
        }
    }

    /// Convert the docx into a markdown string.
    fn convert(mut self) -> Result<String, String> {
        let docx = read_docx(self.docx).map_err(|e| e.to_string())?;
        for child in &docx.document.children {
            if let DocumentChild::Paragraph(p) = child {
                self.write_paragraph(p);
            }
        }
        // Trim trailing blank lines so the output ends with a single newline.
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        Ok(self.out)
    }

    /// Render a single paragraph to markdown.
    fn write_paragraph(&mut self, para: &Paragraph) {
        let style = para
            .property
            .style
            .as_ref()
            .map(|s| s.val.as_str())
            .unwrap_or_default();

        if style == CODE_BLOCK_STYLE {
            let mut text = String::new();
            for child in &para.children {
                if let ParagraphChild::Run(run) = child {
                    collect_run_text(run, &mut text);
                }
            }
            self.out.push_str("```\n");
            self.out.push_str(&text);
            if !text.ends_with('\n') {
                self.out.push('\n');
            }
            self.out.push_str("```\n\n");
            return;
        }

        match style {
            "Heading1" => self.out.push_str("# "),
            "Heading2" => self.out.push_str("## "),
            "Heading3" => self.out.push_str("### "),
            "Heading4" => self.out.push_str("#### "),
            "Heading5" => self.out.push_str("##### "),
            "Heading6" => self.out.push_str("###### "),
            "ListParagraph" => self.out.push_str("- "),
            _ => {}
        }

        for child in &para.children {
            if let ParagraphChild::Run(run) = child {
                self.write_run(run);
            }
        }

        self.out.push_str("\n\n");
    }

    /// Render a single run, applying inline markdown formatting.
    fn write_run(&mut self, run: &Run) {
        let mut text = String::new();
        for child in &run.children {
            match child {
                RunChild::Text(t) => text.push_str(&t.text),
                RunChild::Tab(_) => text.push('\t'),
                RunChild::Break(_) => text.push_str("  \n"),
                _ => {}
            }
        }
        if text.is_empty() {
            return;
        }

        let style = run
            .run_property
            .style
            .as_ref()
            .map(|s| s.val.as_str())
            .unwrap_or_default();

        if style == CODE_RUN_STYLE {
            self.out.push('`');
            self.out.push_str(&text);
            self.out.push('`');
            return;
        }

        let bold = run.run_property.bold.is_some();
        let italic = run.run_property.italic.is_some();
        let marker = match (bold, italic) {
            (true, true) => "***",
            (true, false) => "**",
            (false, true) => "*",
            (false, false) => "",
        };
        self.out.push_str(marker);
        self.out.push_str(&text);
        self.out.push_str(marker);
    }
}

/// Append all literal text contained in a run, ignoring formatting.
fn collect_run_text(run: &Run, out: &mut String) {
    for child in &run.children {
        match child {
            RunChild::Text(t) => out.push_str(&t.text),
            RunChild::Tab(_) => out.push('\t'),
            RunChild::Break(_) => out.push('\n'),
            _ => {}
        }
    }
}
