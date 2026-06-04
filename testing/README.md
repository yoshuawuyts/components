# Wasm component test harness

A reusable strategy for testing the Wasm components in this repo
(`textsearch`, `wordmark`, `tablemark`), modelled on
[Lann Martin's `wasi:test`](https://github.com/lann/wasi-test) prototype.

## The idea

A test suite is *itself a Wasm component* that exports a standard
`wasi:test/tests` interface. A generic **runner** component imports that
interface and exports `wasi:cli/run`. For each component we **compose** three
components into one runnable component and execute it with `wasmtime`:

```
  component-under-test (CUT)  ──┐
  test suite (wasi:test/tests) ─┼──►  wac compose  ──►  wasmtime run
  generic runner (wasi:cli/run)─┘
```

This is language- and runtime-agnostic, and the runner is reused unchanged for
every component. The only per-component code is a small suite crate.

## Layout

```
testing/
  Cargo.toml            separate workspace (the root workspace excludes testing/)
  rust-toolchain.toml   nightly + wasm32-wasip2 (required for wit-bindgen 0.46 async)
  wit/wasi-test.wit     the harness ABI, vendored from Lann (see "OCI" below)
  wasi-test/            helper crate: the `suite!` macro + `TestContext`
  runner-cli/           the generic runner (import tests, export wasi:cli/run)
  suites/
    smoke/              a CUT-less pass+fail suite that exercises the async path
    textsearch/         suite for the textsearch component
    wordmark/           suite for the wordmark component
    tablemark/          suite for the tablemark component
  compositions/
    textsearch.wac      cut + suite + runner  ->  runnable component
    wordmark.wac
    tablemark.wac
```

### Why a separate workspace

The suites/runner/helper use **wit-bindgen 0.46 + the async component-model
stream ABI** and only build for `wasm32-wasip2`. The root workspace's CI runs
`cargo check --all` / `cargo test --all` on the **native** host, which would
fail on these wasm-only async crates. Isolating them keeps the two concerns
separate, and the components stay on wit-bindgen 0.36, unchanged. Suites consume
the components only as `.wasm` artifacts via WAC — never as cargo deps.

## Running the tests

From the repo root (requires `just`, `wac`, `wasmtime` >= 37, and a
`wasm32-wasip2` Rust target):

```sh
just test-component textsearch   # build + compose + run one component's suite
just test-components             # all three
```

Each recipe rebuilds the component under test and the harness first, composes
them with the generic runner via `wac compose`, then runs the result with
`wasmtime run -Wcomponent-model-async`. A failing test prints its streamed logs
and the process exits non-zero.

## How a suite is written

Each suite has two halves:

1. `wit/world.wit` — the component's bare world functions, duplicated as
   **imports** (the "temporary duplication" — see below). At compose time WAC
   wires the component's exports into these imports.
2. `src/lib.rs` — plain `fn(&TestContext) -> impl IntoTestResult` test
   functions, registered with `wasi_test::suite!(...)`, which generates the
   `wasi:test/tests` export.

```rust
mod bindings {
    wit_bindgen::generate!({ world: "imports", path: "wit", pub_export_macro: false });
}
use wasi_test::TestContext;

fn test_basic_search(ctx: &TestContext) -> Result<(), String> {
    ctx.log("searching for 'world'");
    let matches = bindings::search("world", "hello world", &opts())?;
    if matches.len() == 1 { Ok(()) } else { Err(format!("got {}", matches.len())) }
}

wasi_test::suite!(test_basic_search /* , ... */);
```

### The temporary duplication

The components export **bare world-level functions with inline types**, not
named interfaces, so there is no shared interface package for a suite to depend
on. Each suite therefore duplicates the component's signatures as imports in its
`wit/world.wit`. At compose time, component-model type identity flows from the
component's export straight into the suite's structurally-identical import.

This works because every type involved is a plain **value type** (record / list
/ result / string). It does **not** extend to `resource` / handle / borrow
types — a component exporting those would need a real shared named interface.
Each suite's `wit/world.wit` must mirror its `components/<name>/wit/world.wit`
exactly; a drift makes `wac compose` fail loudly (this is also the CI WIT-drift
check).

## On a published OCI `wasi:test` package

As of writing, `wasi:test` is an **unpublished prototype** with no OCI release,
so we vendor `wit/wasi-test.wit` verbatim and treat it as the harness ABI
(`stream<string>` is a native component-model type, so the file is fully
self-contained with no external WIT deps). The runner and *every* suite must
share this exact copy so the `wasi:test/tests` interface identity matches at
compose time.

Optional follow-up: this repo already publishes WIT packages to GHCR (see
`publish.yml` for `docs`/`acp`). We could publish our copy of `wasi:test` the
same way and consume it via OCI instead of vendoring.

## Credits and provenance

Three artifacts here come from Lann Martin's `wasi:test` prototype at
<https://github.com/lann/wasi-test>, and each leads with an explicit credit to
Lann in its file header:

- `wit/wasi-test.wit` (vendored verbatim)
- `wasi-test/` (ported helper crate)
- `runner-cli/` (ported runner)

The upstream repo currently ships no license, so rather than relicense someone
else's work we credit it directly at the top of each vendored file. The repo's
own Apache-2.0 does not extend to these files; the two ported crates are marked
`publish = false` and make no Apache-2.0 claim. If the project later wants to
publish or distribute this code under a license, that needs an explicit license
from Lann first (e.g. Apache-2.0 or MIT).

## Status

All suites are verified end-to-end on wasmtime 37.0.2 (`just test-components`,
exit 0):

| component  | tests | result |
|------------|-------|--------|
| smoke      | 2     | pass+fail path (async streaming logs, non-zero exit) |
| textsearch | 10    | all pass |
| wordmark   | 4     | all pass |
| tablemark  | 4     | all pass |

The async path (wit-bindgen 0.46 streams + `-Wcomponent-model-async`) — the
highest-risk piece — works: streaming logs are emitted, a passing test is quiet,
a failing test prints its log and the process exits non-zero.

Each composed component validates with `wasm-tools validate -f cm-async` and,
post-composition, imports only standard WASI interfaces (wasmtime satisfies them
at run time) — the component-under-test's bare functions are fully wired into the
suite, confirming the WAC composition mechanism.

