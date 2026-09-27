# Supported markdown extensions

Map: [wikirs map](../map.md)
Type: grilling
Status: resolved
Assignee: Saqib
Blocked by: —

## Question

Which markdown extensions does wikirs officially support, meaning they're parsed for the Index, rendered in the GUI and treated the same by every Interface? Candidates: GFM tables, task lists, strikethrough, footnotes, callouts/admonitions (`> [!note]`), math (`$…$`), mermaid, and embedded Pages/transclusion (`![[page]]`). For each, what does the GUI render, and does the TUI show only the source? Must the two parsers agree (pulldown-cmark for the Index, markdown-rs inside gpui-kit's renderer), or does the Index only need Links, Tags and headings? Does a static HTML export (still unspecified on the map) change the answer?

Inputs: [markdown parser research](../research/04-markdown-parser.md) (pulldown-cmark 0.13 options), [gpui research](../research/01-gpui-outside-zed.md) (gpui-kit's renderer is GFM with `MarkdownPlugin` extension points, and the prototype confirmed that wikilinks and frontmatter need plugins), and [gui.md](../../../docs/spec/gui.md).

Input from [TUI layout and editing prototype](12-tui-prototype.md): the TUI will render by walking pulldown-cmark events. So every supported extension needs a terminal rendering too (or an explicit "shown as source" fallback), and pulldown-cmark becomes the renderer for two of the three places markdown is shown.

## Answer

Resolved 2026-09-26 by grilling. The result is in [docs/spec/markdown.md](../../../docs/spec/markdown.md), and the parser decision is [ADR 0007](../../../docs/adr/0007-one-markdown-parse-shared-document-model.md). `gui.md`, `tui.md` and `workspace.md` were amended to match.

1. **Supported**: CommonMark + GFM (tables, task lists, strikethrough, autolinks), footnotes, alerts, math, YAML frontmatter, wikilinks and Attachment embeds. `![[page]]` is a Link, not a transclusion. Mermaid is a plain code block.
2. **Meaning**: pulldown-cmark in the core is the only parser. A piece of text is a Link, Tag or heading only if the core's parse says so, and every clickable Link comes from the core.
3. **Rendering**: a shared document model in the core, which the TUI and GUI each render. This replaces gpui-kit's `TextView`, which re-parses with markdown-rs (twice the Link rules, and quadratic tables: 106 s on 6.6 MB).
4. **Presentation**: a table per construct for each UI. Math is styled source in v1, and task lists are read-only.
5. **Raw HTML** is shown as literal text, never rendered. Unknown syntax is plain text with no diagnostics.
