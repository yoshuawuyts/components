//! OCR WIT component: extract UTF-8 text (or Markdown) from images and PDFs.
//!
//! This component defines the OCR contract (see `wit/world.wit`) and a
//! scaffold implementation that sniffs the input format (PNG / JPEG / PDF)
//! from its magic bytes. The actual recognition backend (e.g. Tesseract or
//! a pure-Rust model) is intentionally not bundled here: it is too heavy
//! and too platform-sensitive to ship without a deliberate decision about
//! distribution. A backend can be added in a follow-up without changing
//! the WIT interface.
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_docs_in_private_items,
    reason = "wit-bindgen generates unsafe FFI glue and undocumented items"
)]

wit_bindgen::generate!({
    world: "ocr",
    path: "wit",
});

/// The WIT component implementation.
struct Component;

export!(Component);

impl Guest for Component {
    fn extract(input: Vec<u8>, options: ExtractOptions) -> Result<OcrOutput, String> {
        let kind = InputKind::sniff(&input)?;
        // No backend is bundled in this build; surface that explicitly so
        // callers do not silently receive empty output. The format and the
        // requested options are echoed back to make debugging easier.
        Err(format!(
            "ocr backend not bundled: detected {kind} input ({size} bytes), \
             format={format:?}, include_words={include_words}",
            kind = kind.as_str(),
            size = input.len(),
            format = options.format,
            include_words = options.include_words,
        ))
    }
}

/// The input formats this component is able to recognise from magic bytes.
#[derive(Debug, Clone, Copy)]
enum InputKind {
    Png,
    Jpeg,
    Pdf,
}

impl InputKind {
    /// PNG signature: 89 50 4E 47 0D 0A 1A 0A
    const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    /// JPEG SOI marker: FF D8 FF
    const JPEG_MAGIC: [u8; 3] = [0xFF, 0xD8, 0xFF];
    /// PDF header: "%PDF-"
    const PDF_MAGIC: [u8; 5] = *b"%PDF-";

    /// Detect the input format from leading bytes, or return a descriptive
    /// error if the input is empty or of an unsupported format.
    fn sniff(input: &[u8]) -> Result<Self, String> {
        if input.is_empty() {
            return Err("empty input".to_owned());
        }
        if input.starts_with(&Self::PNG_MAGIC) {
            return Ok(Self::Png);
        }
        if input.starts_with(&Self::JPEG_MAGIC) {
            return Ok(Self::Jpeg);
        }
        if input.starts_with(&Self::PDF_MAGIC) {
            return Ok(Self::Pdf);
        }
        Err("unsupported input format: expected PNG, JPEG, or PDF".to_owned())
    }

    /// Human-readable name of the input kind, used in error messages.
    fn as_str(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Pdf => "PDF",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InputKind;

    #[test]
    fn sniff_png() {
        let bytes = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00];
        assert_eq!(InputKind::sniff(&bytes).unwrap().as_str(), "PNG");
    }

    #[test]
    fn sniff_jpeg() {
        let bytes = [0xFF, 0xD8, 0xFF, 0xE0, 0x00];
        assert_eq!(InputKind::sniff(&bytes).unwrap().as_str(), "JPEG");
    }

    #[test]
    fn sniff_pdf() {
        let bytes = b"%PDF-1.4\n";
        assert_eq!(InputKind::sniff(bytes).unwrap().as_str(), "PDF");
    }

    #[test]
    fn sniff_empty_input_errors() {
        let err = InputKind::sniff(&[]).unwrap_err();
        assert!(err.contains("empty"), "unexpected error: {}", err);
    }

    #[test]
    fn sniff_unsupported_format_errors() {
        // GIF magic, not supported.
        let bytes = b"GIF89a\x00\x00";
        let err = InputKind::sniff(bytes).unwrap_err();
        assert!(err.contains("unsupported"), "unexpected error: {}", err);
    }

    #[test]
    fn sniff_truncated_png_magic_errors() {
        // First two bytes of PNG magic are not enough to match.
        let bytes = [0x89, 0x50];
        assert!(InputKind::sniff(&bytes).is_err());
    }
}
