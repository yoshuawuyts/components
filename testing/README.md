# Wasm component testing

This directory contains the test harness for the components in this repository.
The strategy is **tests-as-components**: each test suite is itself a Wasm
component that is composed with the real component-under-test and a generic
runner, then executed with [`wasmtime`]. This exercises the actual built
`*.wasm` artifact across the WIT boundary — including the `wit-bindgen` ABI
glue — rather than testing native Rust that merely resembles the component.

The approach is directly inspired by Lann Martin's [`wasi:test`] prototype.
See [Differences from `wasi:test`](#differences-from-wasitest) below.

[`wasmtime`]: https://wasmtime.dev
[`wasi:test`]: https://github.com/lann/wasi-test

## Layout

```
testing/
├── Cargo.toml             # standalone workspace (excluded from the root one)
├── wit/test.wit           # canonical `wasi:test` harness package
├── runner/                # generic, suite-agnostic test runner (a CLI component)
└── textsearch-tests/      # test suite for the `textsearch` component
```

The crates here target `wasm32-wasip2` and use `wit-bindgen`, so they are kept
in their own workspace (the root `Cargo.toml` `exclude`s `testing`) and are not
part of the host `cargo test --all` build.

## How it works

1. **The harness contract** (`wit/test.wit`, package `wasi:test`) defines a
   `tests` interface with a single `run-all: func() -> list<test-outcome>`,
   plus a `suite` world (exports `tests`) and a `runner` world (imports
   `tests`).

2. **A suite** (e.g. `textsearch-tests`) is a component that *imports* the
   surface of the component-under-test and *exports* `wasi:test/tests`. Its
   tests call the imported functions and report pass/fail.

3. **The runner** is a `wasm32-wasip2` command component that imports
   `wasi:test/tests`, calls `run-all`, prints each result, and exits non-zero
   if any test failed.

4. **Composition** wires them together with [`wac`]:

   ```
   wac plug --plug <component>.wasm <suite>.wasm     -o suite.wasm
   wac plug --plug suite.wasm       <test-runner>.wasm -o runner.wasm
   wasmtime run runner.wasm
   ```

[`wac`]: https://github.com/bytecodealliance/wac

## Running the tests

Install the prerequisites and run the recipe from the repository root:

```sh
rustup target add wasm32-wasip2
# `wac` and `wasmtime` must be on your PATH
just test
```

`just test` builds the component-under-test and the harness crates, performs
the `wac` compositions, and runs each suite under `wasmtime`. CI runs the same
recipe (see `.github/workflows/ci.yaml`).

## Adding a suite for another component

1. Create `testing/<component>-tests/` with a `Cargo.toml` (a `cdylib` crate
   depending on `wit-bindgen`) and add it to `testing/Cargo.toml`'s `members`.
2. Add `wit/world.wit` declaring a world that **imports every exported
   function** of the component-under-test (mirroring its `world.wit`) and
   `include`s `wasi:test/suite`. Vendor the harness package into
   `wit/deps/wasi-test/test.wit` (copy of `testing/wit/test.wit`).
3. Implement `exports::wasi::test::tests::Guest::run_all` in `src/lib.rs`.
4. Add a `just _run-suite <component> <suite_crate>` line to the `test` recipe
   in the root `justfile`.

> **Important:** a suite must import *all* of the component's exported
> functions, even those it does not test. `wac plug` rejects a composition
> whose socket imports only a subset of a freestanding world's exports with
> `type not valid to be used as import`.

## Differences from `wasi:test`

The upstream [`wasi:test`] prototype models each test as a `resource`
(`test-case`) with a `run` method and supports lazy log streaming via
`stream<string>`. To keep suites and runners buildable on stable Rust for
`wasm32-wasip2` without the component-model-async feature, this harness uses a
simpler, resource- and stream-free contract: a single `run-all` function that
returns a list of outcomes. Richer features (per-test resources, host-supplied
options, log capture, a `libtest`-style runner) are possible future work.
