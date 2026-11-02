# pdfdiff

A Wasm Component that diffs the text of two PDF documents.

Both documents are parsed and their text is extracted page by page. The text
is compared line by line, and the changes are returned as a unified diff — the
`.diff` / `.patch` format produced by `diff -u`. Each hunk header names the page
the change is on:

```diff
--- a/report.pdf
+++ b/report.pdf
@@ -12,7 +12,7 @@ page 2
 Revenue grew in every region this quarter,
 led by strong demand in Europe.
 
-Total revenue: $1.2M
+Total revenue: $1.4M
 
 Operating costs stayed flat compared to
 the previous quarter.
```

When the text of both documents is identical, the diff is an empty string.

```wit
interface types {
    record options {
        context-lines: u32,      // like `diff -U <n>`
        old-label: string,       // shown in the `---` header
        new-label: string,       // shown in the `+++` header
        ignore-whitespace: bool, // collapse spacing and skip blank lines
    }
}

interface diff {
    use types.{options};

    compare: func(old: list<u8>, new: list<u8>, options: options) -> result<string, string>;
    extract-text: func(pdf: list<u8>) -> result<list<string>, string>;
}
```

`compare` returns an error prefixed with `old:` or `new:` when a document can't
be parsed. `extract-text` returns the text of each page, which is useful for
checking what `compare` sees.

The component needs no filesystem, network, or clock access: PDFs are passed in
as bytes.

## Command line

[`pdfdiff-cli`](../pdfdiff-cli) wraps this component in a `diff`-style command
that reads PDFs from disk:

```bash
just build-pdfdiff-cmd
component run --dir . target/wasm32-wasip2/release/pdfdiff-cmd.wasm old.pdf new.pdf
```

## Limitations

- Only text is compared. Changes to images, fonts, colors, or layout that don't
  change the extracted text are not reported.
- Text order follows the order it is drawn in the PDF, which for multi-column
  or heavily positioned layouts may differ from reading order.
- Scanned PDFs without a text layer have no text to compare.
- Encrypted PDFs are only supported when they open with an empty password.
- Text extraction is done by [`pdf-extract`]; some malformed or unusual PDFs
  can cause it to trap rather than return an error.

[`pdf-extract`]: https://crates.io/crates/pdf-extract
