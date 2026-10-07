# slidedeck

A Wasm Component that generates PowerPoint (`.pptx`) presentations.

A deck is a list of slides. Each slide starts from a layout — `title`,
`section`, `text`, `bullets`, `columns`, `comparison`, `quote`, `big-number`,
`stats`, `chart`, `table`, `image`, `code`, `process`, or `blank` — which
expands into themed shapes, and may add free-form shapes on top: text boxes,
geometric shapes, lines, lists, pictures (PNG, JPEG, GIF), tables, native
editable charts (bar, column, line, area, pie, doughnut), code blocks, and stat
boxes. Slides can carry speaker notes, transitions, and custom backgrounds, and
decks can add a footer and slide numbers. Seven built-in themes are available.

Output is deterministic: the same input always produces the same bytes. The
component needs no filesystem, network, or clock access.

```wit
interface presentation {
    generate: func(deck: deck) -> result<list<u8>, string>;
    from-markdown: func(markdown: string, theme: theme) -> result<list<u8>, string>;
}
```

`generate` returns an error naming the first invalid input, prefixed with its
location, such as `slide 3: shape 2 (text): frame extends past the slide: ...`.

## Markdown

`from-markdown` splits slides on thematic breaks (`---`) and headings:

- The first `#` heading becomes a title slide; a paragraph right after it
  becomes the subtitle. Later `#` headings become section slides.
- `##` headings title content slides.
- Lists become bullet slides; two lists side by side become two columns.
  Paragraphs become text slides.
- Block quotes become quote slides. A final line starting with `—` or `--` is
  the attribution.
- Fenced code becomes a code slide and tables become table slides.
- HTML comments (`<!-- ... -->`) become speaker notes.

```bash
wasmtime run --invoke 'from-markdown("# Hello\n\n- one\n- two", light-clean)' slidedeck.wasm
```

## Credits

The layouts, themes, and OOXML structure are modeled on the PowerPoint module
of [hyperlight-dev/hyperagent](https://github.com/hyperlight-dev/hyperagent).
