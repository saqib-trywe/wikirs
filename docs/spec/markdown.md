# Supported markdown

Which markdown wikirs understands, and how each construct appears in the GUI and TUI. Decided in [Supported markdown extensions](../../.scratch/wikirs/issues/17-markdown-extensions.md). The one-parser rule is [ADR 0007](../adr/0007-one-markdown-parse-shared-document-model.md). Link syntax is in the [Page identity decision](../../.scratch/wikirs/issues/05-page-identity-and-link-syntax.md), and Tag syntax in the [Tag model](../../.scratch/wikirs/issues/06-tag-model.md).

## Supported

Parsed by pulldown-cmark in `wikirs-core`:

- **CommonMark**
- **GFM**: tables, task lists, strikethrough, autolinks
- **Footnotes**
- **Alerts / callouts**: `> [!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]`, `[!CAUTION]`
- **Math**: `$…$` inline, `$$…$$` display
- **YAML frontmatter** (`---`); only `tags` and `title` carry meaning (plus `order`)
- **Wikilinks** `[[path]]`, `[[path|alias]]`, `[[path#heading]]`, and **Attachment embeds** `![[path/file.png]]`
- **Inline Tags** `#a/b` (the core's own scanner over text events)

**Not supported:**
- **`![[page]]` transclusion**: treated as a Link to the Page (so it counts as a Backlink) and rendered as a link.
- **Mermaid**: a fenced code block, shown as code.

## Meaning vs presentation

- Whether text is a Link, Tag or heading is decided **only** by the core's parse.
- Renderers draw the core's document model (blocks and inlines carrying Link, Tag and heading ranges). They never re-parse.
- Every clickable Link comes from the core (`links` / `resolve_link`).

## Presentation

| Construct | GUI | TUI |
|---|---|---|
| headings, emphasis, lists, quotes | styled text | styled text |
| tables | table | box-drawn grid |
| task lists | checkboxes (read-only in v1) | `[ ]` / `[x]` |
| alerts | coloured callout with its label | bordered block with its label |
| footnotes | superscript marker, with the list at the end | `[^1]`, with the list at the end |
| math | styled source in v1 (typesetting later) | styled source |
| code blocks (incl. mermaid) | syntax-highlighted | code block |
| Links | clickable; Broken Links styled as broken | numbered `[n]`; Broken Links marked |
| images / `![[attachment]]` | inline image (local image loading must work, see [gui.md](gui.md)) | `[image: path]` placeholder, opens in the system viewer |
| frontmatter | hidden (the Tags panel shows `tags`) | hidden |
| raw HTML | shown as literal, dimmed text; never rendered | same |
| anything unrecognised | plain text | plain text |

Unsupported or malformed syntax never causes an error or a `check` diagnostic.

A static HTML export (still in the map's Not yet specified) will decide separately whether raw HTML passes through.
