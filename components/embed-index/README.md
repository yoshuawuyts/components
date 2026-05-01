# embed-index

A Wasm component that builds and queries a **vector similarity index** over
embeddings supplied by the host.

## What is this?

Modern AI applications often turn text, images, or other content into
**embeddings**: dense lists of floating-point numbers (e.g. 384 or 1536 of
them) that represent the meaning of the input in a way machines can compare.
Two pieces of content that mean similar things end up with embedding vectors
that point in similar directions. This is the building block behind semantic
search, retrieval-augmented generation (RAG), recommendation systems, and
deduplication.

To find the items most similar to a given query, you compare the query's
embedding against every embedding in your collection and keep the closest
ones. A **vector index** is a data structure that makes those nearest-neighbor
lookups efficient and convenient.

`embed-index` is exactly that index, packaged as a Wasm component:

- You hand it a list of embedding vectors (`build`) — it gives you back an
  opaque blob of bytes you can store anywhere (a file, a database row, a KV
  store).
- Later, you hand it back the bytes plus a query vector and a `k`
  (`query`) — it gives you back the `k` closest items along with their
  similarity scores.

The component itself **does not produce embeddings**. Generating embeddings is
the host's responsibility (e.g. by calling an embedding model). This keeps the
component small, dependency-free, and free of choices about which model to
use.

### Algorithm

This implementation is a **flat (brute-force) cosine-similarity index**: every
query is compared against every stored vector. That's `O(N · dim)` per query,
which is fast for thousands of vectors, fine for tens of thousands, and
slower for millions. The results are **exact** — there is no approximation,
unlike ANN structures such as HNSW.

Vectors are L2-normalized when the index is built, so cosine similarity
reduces to a plain dot product at query time. Scores are in `[-1.0, 1.0]`,
where `1.0` means "identical direction" and higher is more similar.

## Interface (WIT)

```wit
package yoshuawuyts:embed-index;

world embed-index {
    record vector { values: list<f32> }
    record hit    { index: u32, score: f32 }

    /// Build an index over the supplied vectors. Returns opaque bytes that
    /// the host should store and pass back to `query` later.
    export build: func(vectors: list<vector>) -> result<list<u8>, string>;

    /// Return the `k` vectors most similar to `query`, in descending order
    /// of similarity. `hit.index` refers back to the position of the vector
    /// in the original `build` input.
    export query: func(index: list<u8>, query: list<f32>, k: u32) -> result<list<hit>, string>;
}
```

A few rules the component enforces:

- All vectors passed to `build` must be non-empty and share the same length.
- Vectors with zero norm or non-finite values (`NaN`, `±inf`) are rejected.
- The query vector passed to `query` must have the same length as the
  vectors the index was built from.
- `k = 0`, or querying an empty index, returns an empty list.
- If `k` exceeds the number of indexed vectors, all of them are returned.

## How to use it

Typical flow from the host's point of view:

1. **Embed your corpus.** For each item you want to be searchable, run it
   through your embedding model of choice and collect a `list<f32>`.
2. **Build the index** by calling `build(vectors)`. Persist the returned
   bytes wherever you like — they're self-contained.
3. **At query time**, embed the user's query the same way, then call
   `query(index_bytes, query_vector, k)`.
4. **Map hits back** to your data: each `hit.index` is the position of a
   vector in the list you originally passed to `build`, so keep a parallel
   array (or database column) of the original items.

### Sketch in pseudocode

```text
// One-time, when your corpus changes:
docs           = ["The cat sat on the mat", "Rust is a systems language", ...]
vectors        = [embed(d) for d in docs]            // host's embedding model
index_bytes    = embed_index.build(vectors)
store(index_bytes)

// Per query:
q              = embed("a programming language for low-level work")
hits           = embed_index.query(load(), q, k=3)
results        = [(docs[h.index], h.score) for h in hits]
```

### Building this component

```sh
cargo build -p embed-index --target wasm32-wasip2 --release
```

The resulting `embed_index.wasm` lives under `target/wasm32-wasip2/release/`
and can be loaded by any Wasm Component Model host (Wasmtime, jco, etc.).

## When to use something else

- **Very large corpora (millions of vectors).** Switch to an approximate
  nearest-neighbor (ANN) structure such as HNSW for sub-linear queries.
- **Frequent incremental updates.** This component rebuilds the whole index
  from scratch each time; if you add vectors constantly, an index that
  supports incremental insertion will be a better fit.
- **Distance metrics other than cosine.** Only cosine similarity is
  supported here.
