//! Native memory mappings and bounded, read-only byte buffers for WASI.
//!
//! This is a compatibility adapter for the read-only subset used by gitoxide,
//! not a general implementation of the memmap2 API on WASI.

#[cfg(not(target_os = "wasi"))]
pub use native::*;

#[cfg(target_os = "wasi")]
mod wasi {
    use std::fs::File;
    use std::io::{self, Read, Seek, SeekFrom};
    use std::ops::Deref;

    const MAX_BYTES: u64 = 256 * 1024 * 1024;

    /// A read-only snapshot of a file, backed by an owned byte buffer.
    #[derive(Debug)]
    pub struct Mmap(Vec<u8>);

    impl Mmap {
        /// Read a complete file into a bounded, read-only snapshot.
        pub fn map(file: &File) -> io::Result<Self> {
            MmapOptions::new().map_copy_read_only(file)
        }
    }

    impl Deref for Mmap {
        type Target = [u8];

        fn deref(&self) -> &[u8] {
            &self.0
        }
    }

    impl AsRef<[u8]> for Mmap {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }

    /// Options for a bounded, read-only file snapshot.
    #[derive(Debug, Default)]
    pub struct MmapOptions {
        offset: u64,
        len: Option<usize>,
    }

    impl MmapOptions {
        /// Construct default options, reading the entire file.
        #[must_use]
        pub fn new() -> Self {
            Self::default()
        }

        /// Start reading at this byte offset.
        pub fn offset(&mut self, offset: u64) -> &mut Self {
            self.offset = offset;
            self
        }

        /// Read exactly this many bytes.
        pub fn len(&mut self, len: usize) -> &mut Self {
            self.len = Some(len);
            self
        }

        /// Read a file using WASI filesystem operations, never memory mapping.
        ///
        /// The name matches memmap2's API; the operation itself is safe.
        pub fn map_copy_read_only(&self, mut file: &File) -> io::Result<Mmap> {
            let original_position = file.stream_position()?;
            let result = self.read(file);
            let restored = file.seek(SeekFrom::Start(original_position));
            match (result, restored) {
                (Err(error), _) | (Ok(_), Err(error)) => Err(error),
                (Ok(map), Ok(_)) => Ok(map),
            }
        }

        fn read(&self, mut file: &File) -> io::Result<Mmap> {
            let available = file
                .metadata()?
                .len()
                .checked_sub(self.offset)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "offset beyond file"))?;
            let len = self.len.map_or(available, |len| len as u64);
            if len > available || len > MAX_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "file snapshot exceeds file size or the 256 MiB WASI limit",
                ));
            }
            let len =
                usize::try_from(len).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(len).map_err(io::Error::other)?;
            bytes.resize(len, 0);
            file.seek(SeekFrom::Start(self.offset))?;
            file.read_exact(&mut bytes)?;
            Ok(Mmap(bytes))
        }
    }
}

#[cfg(target_os = "wasi")]
pub use wasi::{Mmap, MmapOptions};
