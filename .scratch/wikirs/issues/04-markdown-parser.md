# Markdown parser: link/tag extraction and source-preserving rewrite

Map: [wikirs map](../map.md)
Type: research
Status: resolved
Blocked by: —

## Question

Which markdown crate should the core use? Compare comrak, pulldown-cmark and markdown-rs on: source position (byte offset) info for every node; GFM extensions (tables, task lists, footnotes); wikilink (`[[...]]`) support or extension hooks; frontmatter handling; and, most importantly, whether links can be **rewritten in place without reformatting the rest of the file** (offset-based edits vs AST round-trip). Also note how the options perform on large files and whether each can render to HTML for the GUI, HTTP and TUI.

## Answer

- None of the three round-trips losslessly; byte-identical rewrites need offset splicing on the original text. comrak's `format_commonmark` was tested and reformatted bullets, tables, reference links, footnotes and wikilinks.
- pulldown-cmark 0.13.4 gives byte `Range<usize>` per event (`into_offset_iter`), built-in wikilinks (`ENABLE_WIKILINKS`), GFM tables/tasks/footnotes, YAML/TOML metadata blocks. It's also the fastest (~9 ms/MB). Caveats: link range is the whole link, not just the destination, and there are open offset/panic edge-case issues (#1129, #852).
- comrak 0.55.0 has built-in wikilinks, frontmatter, GFM plus many extras, and a mutable AST. Positions are 1-based line/column (UTF-8 bytes, inclusive end), so they need converting to offsets. It was ~4–5x slower than pulldown but still linear, and it releases breaking minor versions often (MSRV 1.85).
- markdown-rs 1.0.0 has byte offsets and GFM/frontmatter, but no wikilinks and no extension hook (`[[x]]` comes through as Text). It was superlinear in testing (1 MB took 5.4 s) and has had no commits since April 2025.
- All three render HTML. For the TUI, `tui-markdown` (ratatui) is built on pulldown-cmark. All three return frontmatter as a raw string (a YAML crate is still needed), and none of them handles inline `#tag` syntax.
- [findings](../research/04-markdown-parser.md)
