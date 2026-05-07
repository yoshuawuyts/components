//! Embed-index WIT component: build and query a flat ANN-style vector index.
//!
//! The component implements a *flat* (brute-force) cosine-similarity index:
//! `build` accepts a list of equally-sized dense vectors and returns an
//! opaque byte blob; `query` accepts that blob plus a query vector and
//! returns the top-`k` most similar entries. The host is responsible for
//! producing the embeddings and for persisting the index bytes between
//! calls.
//!
//! The wire format is intentionally simple and self-describing so that an
//! index produced by one build of the component can be queried by any other
//! build of the same major version.
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_docs_in_private_items,
    reason = "wit-bindgen generates unsafe FFI glue and undocumented items"
)]

wit_bindgen::generate!({
    world: "embed-index",
    path: "wit",
});

/// The WIT component implementation.
struct Component;

export!(Component);

impl Guest for Component {
    fn build(vectors: Vec<Vector>) -> Result<Vec<u8>, String> {
        Index::build(&vectors).map(|idx| idx.to_bytes())
    }

    fn query(index: Vec<u8>, query: Vec<f32>, k: u32) -> Result<Vec<Hit>, String> {
        let idx = Index::from_bytes(&index)?;
        idx.query(&query, k)
    }
}

use core::cmp::Ordering;
use std::collections::BinaryHeap;
use std::convert::TryFrom;

/// Magic bytes identifying the serialized index format.
const MAGIC: [u8; 4] = *b"EIDX";

/// Current on-disk format version. Increment on incompatible changes.
const VERSION: u8 = 1;

/// Size in bytes of the fixed-length header preceding the vector payload.
///
/// Layout: `magic[4] | version[1] | reserved[3] | num_vectors[4 LE] | dim[4 LE]`.
const HEADER_LEN: usize = 4 + 1 + 3 + 4 + 4;

/// In-memory representation of a built index.
///
/// Vectors are stored L2-normalized so that cosine similarity reduces to a
/// plain dot product at query time.
struct Index {
    /// Dimensionality of every vector.
    dim: usize,
    /// Number of indexed vectors.
    num_vectors: usize,
    /// Flattened, row-major, L2-normalized vector storage of length
    /// `num_vectors * dim`.
    data: Vec<f32>,
}

impl Index {
    /// Build an index from a slice of input vectors.
    fn build(vectors: &[Vector]) -> Result<Self, String> {
        let dim = match vectors.first() {
            Some(v) => v.values.len(),
            None => return Err("cannot build an index from zero vectors".to_owned()),
        };
        if dim == 0 {
            return Err("vectors must have a non-zero dimensionality".to_owned());
        }
        if u32::try_from(dim).is_err() || u32::try_from(vectors.len()).is_err() {
            return Err("index too large to serialize".to_owned());
        }

        let mut data = Vec::with_capacity(vectors.len() * dim);
        for (i, v) in vectors.iter().enumerate() {
            if v.values.len() != dim {
                return Err(format!(
                    "vector at index {i} has length {} but expected {dim}",
                    v.values.len()
                ));
            }
            let normalized = normalize(&v.values)
                .map_err(|e| format!("vector at index {i} could not be normalized: {e}"))?;
            data.extend_from_slice(&normalized);
        }

        Ok(Self {
            dim,
            num_vectors: vectors.len(),
            data,
        })
    }

    /// Serialize the index to its on-disk byte representation.
    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.data.len() * 4);
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&[0u8; 3]);
        // Casts are safe: `build` rejected inputs that don't fit in `u32`.
        #[allow(clippy::cast_possible_truncation, reason = "checked in build()")]
        out.extend_from_slice(&(self.num_vectors as u32).to_le_bytes());
        #[allow(clippy::cast_possible_truncation, reason = "checked in build()")]
        out.extend_from_slice(&(self.dim as u32).to_le_bytes());
        for v in &self.data {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Parse an index from its on-disk byte representation.
    fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < HEADER_LEN {
            return Err("index bytes are shorter than the header".to_owned());
        }
        let magic = bytes.get(..4).ok_or("missing magic")?;
        if magic != MAGIC {
            return Err("index bytes do not start with the expected magic".to_owned());
        }
        let version = *bytes.get(4).ok_or("missing version")?;
        if version != VERSION {
            return Err(format!(
                "unsupported index version {version}; expected {VERSION}"
            ));
        }
        let num_vectors = read_u32(bytes, 8)? as usize;
        let dim = read_u32(bytes, 12)? as usize;
        if dim == 0 {
            return Err("index has zero-dimensional vectors".to_owned());
        }

        let payload_floats = num_vectors
            .checked_mul(dim)
            .ok_or("index dimensions overflow")?;
        let payload_bytes = payload_floats
            .checked_mul(4)
            .ok_or("index payload size overflows usize")?;
        let expected_len = HEADER_LEN
            .checked_add(payload_bytes)
            .ok_or("index size overflows usize")?;
        if bytes.len() != expected_len {
            return Err(format!(
                "index byte length {} does not match expected {expected_len}",
                bytes.len()
            ));
        }

        let mut data = Vec::with_capacity(payload_floats);
        for i in 0..payload_floats {
            let offset = HEADER_LEN + i * 4;
            let chunk = bytes
                .get(offset..offset + 4)
                .ok_or("truncated vector payload")?;
            let mut buf = [0u8; 4];
            buf.copy_from_slice(chunk);
            data.push(f32::from_le_bytes(buf));
        }

        Ok(Self {
            dim,
            num_vectors,
            data,
        })
    }

    /// Return the top-`k` nearest neighbours of `query` by cosine similarity.
    fn query(&self, query: &[f32], k: u32) -> Result<Vec<Hit>, String> {
        if query.len() != self.dim {
            return Err(format!(
                "query has length {} but index dimensionality is {}",
                query.len(),
                self.dim
            ));
        }
        if k == 0 || self.num_vectors == 0 {
            return Ok(Vec::new());
        }
        let normalized = normalize(query).map_err(|e| format!("query vector: {e}"))?;
        let k = (k as usize).min(self.num_vectors);

        // Min-heap keyed on similarity so we can keep the top-k seen so far.
        let mut heap: BinaryHeap<HeapEntry> = BinaryHeap::with_capacity(k + 1);
        for i in 0..self.num_vectors {
            let start = i * self.dim;
            let end = start + self.dim;
            let row = self
                .data
                .get(start..end)
                .ok_or("internal error: row out of bounds")?;
            let score = dot(row, &normalized);
            // Indices fit in u32 because `build` rejected larger inputs.
            #[allow(clippy::cast_possible_truncation, reason = "checked in build()")]
            let entry = HeapEntry {
                score,
                index: i as u32,
            };
            heap.push(entry);
            if heap.len() > k {
                heap.pop();
            }
        }

        let mut hits: Vec<Hit> = heap
            .into_iter()
            .map(|e| Hit {
                index: e.index,
                score: e.score,
            })
            .collect();
        // Sort descending by score, breaking ties by ascending index for
        // determinism.
        hits.sort_by(|a, b| match score_cmp(b.score, a.score) {
            Ordering::Equal => a.index.cmp(&b.index),
            ord => ord,
        });
        Ok(hits)
    }
}

/// L2-normalize a vector. Returns an error if any value is non-finite or if
/// the input has zero norm.
fn normalize(values: &[f32]) -> Result<Vec<f32>, String> {
    let mut sum_sq: f64 = 0.0;
    for v in values {
        if !v.is_finite() {
            return Err("contains a non-finite value".to_owned());
        }
        sum_sq += f64::from(*v) * f64::from(*v);
    }
    let norm = sum_sq.sqrt();
    if norm == 0.0 {
        return Err("has zero norm".to_owned());
    }
    #[allow(clippy::cast_possible_truncation, reason = "intentional f64->f32")]
    let inv = (1.0 / norm) as f32;
    Ok(values.iter().map(|v| v * inv).collect())
}

/// Inner product of two equal-length slices.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc: f32 = 0.0;
    for (x, y) in a.iter().zip(b.iter()) {
        acc += x * y;
    }
    acc
}

/// Read a little-endian `u32` at `offset` from `bytes`.
fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let chunk = bytes
        .get(offset..offset + 4)
        .ok_or("truncated u32 in header")?;
    let mut buf = [0u8; 4];
    buf.copy_from_slice(chunk);
    Ok(u32::from_le_bytes(buf))
}

/// Total ordering on similarity scores. NaN values sort as the lowest score
/// so they fall out of the top-k first.
fn score_cmp(a: f32, b: f32) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
    }
}

/// Heap entry used to maintain the top-`k` results during a query.
///
/// Implements a *min-heap* on `score`: the smallest similarity bubbles to the
/// top so it can be evicted when a better candidate arrives. Ties are broken
/// so that the entry with the *largest* index sits at the top — i.e. when two
/// entries share a score, the one with the higher index is evicted first and
/// the lower index survives, matching the deterministic tie-break applied to
/// the final result list.
struct HeapEntry {
    score: f32,
    index: u32,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse score order so `BinaryHeap` (a max-heap) acts as a min-heap.
        // On equal scores, the entry with the *larger* index ranks higher so
        // that it is the one popped during eviction.
        match score_cmp(other.score, self.score) {
            Ordering::Equal => self.index.cmp(&other.index),
            ord => ord,
        }
    }
}
