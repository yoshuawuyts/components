//! Slidedeck WIT component: generate PowerPoint (`.pptx`) presentations.
//!
//! A deck is described declaratively as a list of slides. Each slide starts
//! from a layout (title, bullets, chart, ...) that expands into themed shapes,
//! and may add free-form shapes on top. The component renders everything to
//! Office Open XML and zips it into a `.pptx` file. Output is deterministic:
//! the same deck always produces the same bytes.
//!
//! Decks can also be generated straight from Markdown.
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_docs_in_private_items,
    reason = "wit-bindgen generates unsafe FFI glue and undocumented items"
)]

wit_bindgen::generate!({
    world: "slidedeck",
    path: "wit",
});

mod charts;
mod image;
mod layouts;
mod markdown;
mod package;
mod shapes;
mod theme;
mod xml;
mod zip;

#[cfg(test)]
mod tests;

pub(crate) use exports::yoshuawuyts::slidedeck::types;

use exports::yoshuawuyts::slidedeck::presentation::Guest;
use types::{Deck, Theme};

struct Component;

impl Guest for Component {
    fn generate(deck: Deck) -> Result<Vec<u8>, String> {
        package::build(&deck)
    }

    fn from_markdown(markdown: String, theme: Theme) -> Result<Vec<u8>, String> {
        let deck = markdown::parse(&markdown, theme)?;
        package::build(&deck)
    }
}

export!(Component);
