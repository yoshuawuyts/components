//! A minimal ZIP archive writer, sufficient for OOXML packages.
//!
//! Entries are DEFLATE-compressed unless compression doesn't shrink them
//! (already-compressed images, for example), in which case they're stored.
//! Timestamps are fixed to the DOS epoch so output is deterministic.

/// DOS date for 1980-01-01, the earliest representable date.
const DOS_DATE: u16 = (1 << 5) | 1;

/// "Version needed to extract": 2.0, the minimum for DEFLATE.
const VERSION: u16 = 20;

/// General-purpose flag bit 11: file names are UTF-8.
const UTF8_FLAG: u16 = 1 << 11;

const METHOD_STORED: u16 = 0;
const METHOD_DEFLATE: u16 = 8;

/// An entry recorded for the central directory.
#[derive(Debug)]
struct Record {
    name: String,
    method: u16,
    crc: u32,
    compressed_len: u32,
    len: u32,
    offset: u32,
}

/// Builds a ZIP archive in memory.
#[derive(Debug, Default)]
pub(crate) struct ZipWriter {
    out: Vec<u8>,
    records: Vec<Record>,
}

impl ZipWriter {
    /// Create an empty archive.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Append a file to the archive.
    pub(crate) fn add(&mut self, name: &str, data: &[u8]) -> Result<(), String> {
        let crc = crc32fast::hash(data);
        let deflated = miniz_oxide::deflate::compress_to_vec(data, 6);
        let (method, body) = if deflated.len() < data.len() {
            (METHOD_DEFLATE, deflated.as_slice())
        } else {
            (METHOD_STORED, data)
        };
        let too_large = || format!("zip entry `{name}` is too large (over 4 GiB)");
        let offset = u32::try_from(self.out.len()).map_err(|_| too_large())?;
        let len = u32::try_from(data.len()).map_err(|_| too_large())?;
        let compressed_len = u32::try_from(body.len()).map_err(|_| too_large())?;
        let name_len = u16::try_from(name.len()).map_err(|_| too_large())?;

        // Local file header.
        self.u32(0x0403_4b50);
        self.u16(VERSION);
        self.u16(UTF8_FLAG);
        self.u16(method);
        self.u16(0); // time
        self.u16(DOS_DATE);
        self.u32(crc);
        self.u32(compressed_len);
        self.u32(len);
        self.u16(name_len);
        self.u16(0); // extra field length
        self.out.extend_from_slice(name.as_bytes());
        self.out.extend_from_slice(body);

        self.records.push(Record {
            name: name.to_owned(),
            method,
            crc,
            compressed_len,
            len,
            offset,
        });
        Ok(())
    }

    /// Write the central directory and return the finished archive.
    pub(crate) fn finish(mut self) -> Result<Vec<u8>, String> {
        let too_large = || "zip archive is too large (over 4 GiB)".to_owned();
        let directory_offset = u32::try_from(self.out.len()).map_err(|_| too_large())?;
        let records = std::mem::take(&mut self.records);
        for record in &records {
            self.u32(0x0201_4b50);
            self.u16(VERSION); // version made by
            self.u16(VERSION); // version needed
            self.u16(UTF8_FLAG);
            self.u16(record.method);
            self.u16(0); // time
            self.u16(DOS_DATE);
            self.u32(record.crc);
            self.u32(record.compressed_len);
            self.u32(record.len);
            self.u16(u16::try_from(record.name.len()).map_err(|_| too_large())?);
            self.u16(0); // extra field length
            self.u16(0); // comment length
            self.u16(0); // disk number
            self.u16(0); // internal attributes
            self.u32(0); // external attributes
            self.u32(record.offset);
            self.out.extend_from_slice(record.name.as_bytes());
        }
        let directory_len =
            u32::try_from(self.out.len()).map_err(|_| too_large())? - directory_offset;
        let count = u16::try_from(records.len()).map_err(|_| too_large())?;

        // End of central directory record.
        self.u32(0x0605_4b50);
        self.u16(0); // this disk
        self.u16(0); // disk with central directory
        self.u16(count);
        self.u16(count);
        self.u32(directory_len);
        self.u32(directory_offset);
        self.u16(0); // comment length
        Ok(self.out)
    }

    fn u16(&mut self, value: u16) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }
}
