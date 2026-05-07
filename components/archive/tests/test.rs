//! Integration tests for the `archive` component.
//!
//! These tests exercise the public extraction and creation helpers directly
//! (without going through the WIT ABI) so that they can run on the host
//! target in addition to `wasm32-wasip2`.

// Re-export the internal helpers used for testing by compiling the crate in
// library mode with a test harness.  Because `src/lib.rs` is a `cdylib` the
// symbols are not re-exported automatically, so the tests must live here and
// repeat what they need.

// ── helpers copied from src/lib.rs (visible for testing) ─────────────────────

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{Cursor, Read, Write};

/// Mirrors the WIT `file-entry` record.
#[derive(Debug, PartialEq)]
struct FileEntry {
    name: String,
    data: Vec<u8>,
    permissions: Option<u32>,
}

fn create_zip(files: Vec<FileEntry>) -> Result<Vec<u8>, String> {
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
        let mut data = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut data).map_err(|e| e.to_string())?;
        entries.push(FileEntry {
            name,
            data,
            permissions,
        });
    }
    Ok(entries)
}

fn create_tar_gz(files: Vec<FileEntry>) -> Result<Vec<u8>, String> {
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
    Ok(encoder.finish().map_err(|e| e.to_string())?)
}

fn extract_tar_gz(input: Vec<u8>) -> Result<Vec<FileEntry>, String> {
    let cursor = Cursor::new(input);
    let decoder = GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(decoder);
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

// ── ZIP tests ─────────────────────────────────────────────────────────────────

#[test]
fn zip_roundtrip_single_file() {
    let files = vec![FileEntry {
        name: "hello.txt".to_owned(),
        data: b"Hello, world!".to_vec(),
        permissions: Some(0o644),
    }];
    let archive = create_zip(files).expect("create_zip failed");
    let extracted = extract_zip(archive).expect("extract_zip failed");
    assert_eq!(extracted.len(), 1);
    assert_eq!(extracted[0].name, "hello.txt");
    assert_eq!(extracted[0].data, b"Hello, world!");
}

#[test]
fn zip_roundtrip_multiple_files() {
    let files = vec![
        FileEntry {
            name: "a.txt".to_owned(),
            data: b"aaa".to_vec(),
            permissions: None,
        },
        FileEntry {
            name: "b/c.txt".to_owned(),
            data: b"bbb".to_vec(),
            permissions: Some(0o755),
        },
    ];
    let archive = create_zip(files).expect("create_zip failed");
    let mut extracted = extract_zip(archive).expect("extract_zip failed");
    extracted.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(extracted.len(), 2);
    assert_eq!(extracted[0].name, "a.txt");
    assert_eq!(extracted[0].data, b"aaa");
    assert_eq!(extracted[1].name, "b/c.txt");
    assert_eq!(extracted[1].data, b"bbb");
}

#[test]
fn zip_empty_archive() {
    let archive = create_zip(vec![]).expect("create_zip failed");
    let extracted = extract_zip(archive).expect("extract_zip failed");
    assert!(extracted.is_empty());
}

// ── tar.gz tests ──────────────────────────────────────────────────────────────

#[test]
fn tar_gz_roundtrip_single_file() {
    let files = vec![FileEntry {
        name: "readme.md".to_owned(),
        data: b"# Hello".to_vec(),
        permissions: Some(0o644),
    }];
    let archive = create_tar_gz(files).expect("create_tar_gz failed");
    let extracted = extract_tar_gz(archive).expect("extract_tar_gz failed");
    assert_eq!(extracted.len(), 1);
    assert_eq!(extracted[0].name, "readme.md");
    assert_eq!(extracted[0].data, b"# Hello");
    assert_eq!(extracted[0].permissions, Some(0o644));
}

#[test]
fn tar_gz_roundtrip_multiple_files() {
    let files = vec![
        FileEntry {
            name: "x.bin".to_owned(),
            data: vec![0u8, 1, 2, 3],
            permissions: None,
        },
        FileEntry {
            name: "sub/y.bin".to_owned(),
            data: vec![4u8, 5, 6, 7],
            permissions: Some(0o600),
        },
    ];
    let archive = create_tar_gz(files).expect("create_tar_gz failed");
    let mut extracted = extract_tar_gz(archive).expect("extract_tar_gz failed");
    extracted.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(extracted.len(), 2);
    assert_eq!(extracted[0].name, "sub/y.bin");
    assert_eq!(extracted[0].data, vec![4u8, 5, 6, 7]);
    assert_eq!(extracted[0].permissions, Some(0o600));
    assert_eq!(extracted[1].name, "x.bin");
    assert_eq!(extracted[1].permissions, Some(0o644)); // default
}

#[test]
fn tar_gz_empty_archive() {
    let archive = create_tar_gz(vec![]).expect("create_tar_gz failed");
    let extracted = extract_tar_gz(archive).expect("extract_tar_gz failed");
    assert!(extracted.is_empty());
}
