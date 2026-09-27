# 7. One markdown parse, shared document model for rendering

Status: accepted (2026-09-26)

## Context

Markdown is interpreted in three places. The core parses it with pulldown-cmark to build the Index (Links, Tags, headings) and to splice edits. The TUI and GUI render it. gpui-kit's ready-made `TextView` re-parses with markdown-rs, which:

- reads `[[wikilinks]]` as plain text, so every UI-visible Link would need a plugin that re-implements the core's Link rules
- was measured at 106 s on a 6.6 MB file, against 58 ms for pulldown-cmark ([parser research](../../.scratch/wikirs/research/04-markdown-parser.md); GFM tables are quadratic in cell count, upstream issue #218)

If a renderer's idea of a Link differs from the Index's, Backlinks stop matching what's clickable.

Alternatives considered:

- **gpui-kit `TextView` + plugins** for the GUI: ready to use, but it means two parsers, two sets of Link rules, and the table-performance risk.
- **Each UI parses on its own**: the same drift, three times over.

## Decision

pulldown-cmark, in `wikirs-core`, is the **only** markdown parser. The core turns its parse into a **document model**: blocks and inlines that carry the core's own Link, Tag and heading ranges. The TUI renders that model to ratatui, and the GUI renders it with gpui elements, reusing gpui-kit components (code highlighting, table, text layout) but not its parser.

Whether text is a Link, Tag or heading is decided by the core's parse alone, and every clickable Link in any UI comes from the core.

## Consequences

- Link, Tag and heading rules stay identical everywhere, and they're tested once.
- Large tables stay fast.
- The GUI has to build its own markdown renderer over the model. That's more work than dropping in `TextView`, and gpui-kit's markdown plugins (math, mentions) aren't reusable.
- A new markdown extension is added once, in the core's parse and model, and then gets a rendering (or a "shown as source" fallback) in each UI.
