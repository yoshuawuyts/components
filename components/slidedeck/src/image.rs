//! Image format detection and dimension parsing.

/// A supported raster image format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageFormat {
    Png,
    Jpeg,
    Gif,
}

impl ImageFormat {
    /// File extension used inside the package.
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Gif => "gif",
        }
    }

    /// MIME type registered in `[Content_Types].xml`.
    pub(crate) fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
        }
    }
}

/// What we learned from an image's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImageInfo {
    pub(crate) format: ImageFormat,
    /// Pixel dimensions, when they could be read and are non-zero.
    pub(crate) size: Option<(u32, u32)>,
}

/// Detect the format of `data` and, where possible, its pixel dimensions.
pub(crate) fn inspect(data: &[u8]) -> Result<ImageInfo, String> {
    let (format, size) = if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        (ImageFormat::Png, png_size(data))
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        (ImageFormat::Jpeg, jpeg_size(data))
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        (ImageFormat::Gif, gif_size(data))
    } else {
        return Err("image data is not a PNG, JPEG, or GIF file".to_owned());
    };
    let size = size.filter(|&(w, h)| w > 0 && h > 0);
    Ok(ImageInfo { format, size })
}

fn be_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn le_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

/// The IHDR chunk always comes first, right after the 8-byte signature.
fn png_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.get(12..16)? != b"IHDR" {
        return None;
    }
    Some((be_u32(data, 16)?, be_u32(data, 20)?))
}

/// The logical screen descriptor follows the 6-byte signature.
fn gif_size(data: &[u8]) -> Option<(u32, u32)> {
    Some((u32::from(le_u16(data, 6)?), u32::from(le_u16(data, 8)?)))
}

/// Walk JPEG segments until the first start-of-frame marker.
fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    loop {
        if *data.get(at)? != 0xFF {
            return None;
        }
        let marker = *data.get(at + 1)?;
        at += 2;
        match marker {
            // Fill bytes before a marker.
            0xFF => at -= 1,
            // Markers without a payload.
            0x01 | 0xD0..=0xD8 => {}
            // End of image before any frame.
            0xD9 => return None,
            // Start-of-frame markers; C4 (DHT), C8 (JPG), and CC (DAC) aren't.
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                let height = be_u16(data, at + 3)?;
                let width = be_u16(data, at + 5)?;
                return Some((u32::from(width), u32::from(height)));
            }
            _ => at += usize::from(be_u16(data, at)?),
        }
    }
}
