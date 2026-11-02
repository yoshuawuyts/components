# pdfdiff-cli

A `diff`-style command line front end for the [`pdfdiff`](../pdfdiff)
component. It reads two PDF files and prints a unified diff of their text.

```text
Usage: pdfdiff [OPTIONS] <OLD.pdf> <NEW.pdf>

Options:
  -U, --unified <n>        Show <n> lines of context (default: 3)
  -w, --ignore-all-space   Ignore whitespace and blank-line changes
      --label <label>      Use <label> instead of the file name in the
                           header; pass twice to label both files
  -h, --help               Print this help
```

The exit status is `0` when the text is identical, `1` when it differs, and `2`
on error, matching `diff`.

## Building

This crate is a `wasi:cli/command` that *imports* the `pdfdiff` diff
interface. Compose it with the `pdfdiff` component to get a single runnable
component:

```bash
just build-pdfdiff-cmd
# -> target/wasm32-wasip2/release/pdfdiff-cmd.wasm
```

The composition lives in [`pdfdiff-cli.wac`](./pdfdiff-cli.wac).

## Running

The component has no filesystem access by default, so preopen the directory
containing the PDFs with `--dir`:

```bash
component run --dir . target/wasm32-wasip2/release/pdfdiff-cmd.wasm old.pdf new.pdf
component run --dir . target/wasm32-wasip2/release/pdfdiff-cmd.wasm -U1 old.pdf new.pdf > changes.diff

# `wasmtime` works too
wasmtime run --dir . target/wasm32-wasip2/release/pdfdiff-cmd.wasm old.pdf new.pdf
```

Paths are resolved relative to the preopened directories, so files outside
them (including `..`) can't be read.
