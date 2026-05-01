//! Archive WIT component: create and extract ZIP, tar, and gzip archives.
//!
//! All operations work entirely on byte arrays — no filesystem I/O is
//! performed.  The `extract` function auto-detects the archive format
//! (ZIP, tar, tar.gz, or bare gzip) from the leading magic bytes.
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_docs_in_private_items,
    clippy::same_length_and_capacity,
    reason = "wit-bindgen generates unsafe FFI glue and undocumented items"
)]

wit_bindgen::generate!({
    world: "archive",
    path: "wit",
});

/// The WIT component implementation.
struct Component;

export!(Component);

impl Guest for Component {
    fn extract(input: Vec<u8>) -> Result<Vec<FileEntry>, String> {
        extract_archive(input)
    }

    fn create_zip(files: Vec<FileEntry>) -> Result<Vec<u8>, String> {
        create_zip_archive(files)
    }

    fn create_tar_gz(files: Vec<FileEntry>) -> Result<Vec<u8>, String> {
        create_tar_gz_archive(files)
    }
}

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{Cursor, Read, Write};

/// ZIP local-file signature: `PK\x03\x04`.
const ZIP_MAGIC: &[u8] = &[0x50, 0x4B, 0x03, 0x04];

/// Gzip stream signature.
const GZIP_MAGIC: &[u8] = &[0x1F, 0x8B];

/// POSIX / GNU tar magic located at byte offset 257 of the header block.
const TAR_MAGIC: &[u8] = b"ustar";

// ── extraction ───────────────────────────────────────────────────────────────

/// Detect the archive format from the leading bytes and extract all entries.
fn extract_archive(input: Vec<u8>) -> Result<Vec<FileEntry>, String> {
    if input.get(0..4) == Some(ZIP_MAGIC) {
        extract_zip(input)
    } else if input.get(0..2) == Some(GZIP_MAGIC) {
        extract_gzip(&input)
    } else {
        // Fall back to plain tar (no magic bytes required).
        extract_tar(Cursor::new(input))
    }
}

/// Extract all non-directory entries from a ZIP archive.
fn extract_zip(input: Vec<u8>) -> Result<Vec<FileEntry>, String> {
    let cursor = Cursor::new(input);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;
    let mut entries = Vec::with_capacity(archive.len());

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_owned();
        let permissions = file.unix_mode();
        let mut data = Vec::with_capacity(usize::try_from(file.size()).unwrap_or(0));
        file.read_to_end(&mut data).map_err(|e| e.to_string())?;
        entries.push(FileEntry {
            name,
            data,
            permissions,
        });
    }
    Ok(entries)
}

/// Decompress a gzip stream; if the payload is a tar archive delegate to
/// [`extract_tar`], otherwise return the payload as a single file entry.
fn extract_gzip(input: &[u8]) -> Result<Vec<FileEntry>, String> {
    let cursor = Cursor::new(input);
    let mut decoder = GzDecoder::new(cursor);

    // Capture the stored filename from the gzip header (set by some tools).
    let gz_name: Option<String> = decoder
        .header()
        .and_then(|h| h.filename())
        .and_then(|raw| std::str::from_utf8(raw).ok())
        .map(|s| {
            // Strip a trailing `.gz` extension (case-insensitive).
            if s.len() > 3 && s[s.len() - 3..].eq_ignore_ascii_case(".gz") {
                s[..s.len() - 3].to_owned()
            } else {
                s.to_owned()
            }
        });

    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| e.to_string())?;

    // Detect tar magic in the decompressed payload.
    if decompressed.get(257..262) == Some(TAR_MAGIC) {
        extract_tar(Cursor::new(decompressed))
    } else {
        let name = gz_name.unwrap_or_else(|| "file".to_owned());
        Ok(vec![FileEntry {
            name,
            data: decompressed,
            permissions: None,
        }])
    }
}

/// Extract all regular-file entries from a tar archive read from `reader`.
fn extract_tar<R: Read>(reader: R) -> Result<Vec<FileEntry>, String> {
    let mut archive = tar::Archive::new(reader);
    let mut entries = Vec::new();

    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let name = entry
            .path()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned();
        let permissions = entry.header().mode().ok();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
        entries.push(FileEntry {
            name,
            data,
            permissions,
        });
    }
    Ok(entries)
}

// ── creation ─────────────────────────────────────────────────────────────────

/// Build a ZIP archive and return its raw bytes.
fn create_zip_archive(files: Vec<FileEntry>) -> Result<Vec<u8>, String> {
    use zip::write::SimpleFileOptions;

    let buf = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(buf);

    for file in files {
        let options = match file.permissions {
            Some(mode) => SimpleFileOptions::default().unix_permissions(mode),
            None => SimpleFileOptions::default(),
        };
        writer
            .start_file(&file.name, options)
            .map_err(|e| e.to_string())?;
        writer.write_all(&file.data).map_err(|e| e.to_string())?;
    }

    let result = writer.finish().map_err(|e| e.to_string())?;
    Ok(result.into_inner())
}

/// Build a gzip-compressed tar archive and return its raw bytes.
fn create_tar_gz_archive(files: Vec<FileEntry>) -> Result<Vec<u8>, String> {
    let buf = Vec::new();
    let encoder = GzEncoder::new(buf, Compression::default());
    let mut builder = tar::Builder::new(encoder);

    for file in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(file.data.len() as u64);
        header.set_mode(file.permissions.unwrap_or(0o644));
        header.set_entry_type(tar::EntryType::Regular);
        builder
            .append_data(&mut header, &file.name, file.data.as_slice())
            .map_err(|e| e.to_string())?;
    }

    let encoder = builder.into_inner().map_err(|e| e.to_string())?;
    let compressed = encoder.finish().map_err(|e| e.to_string())?;
    Ok(compressed)
}
