# Page identity and Link syntax

Map: [wikirs map](../map.md)
Type: grilling
Status: resolved
Blocked by: 04

## Question

How is a Page identified, and how are Links written? Should identity be the file path, or a stable id in frontmatter? Should Links use `[[wikilinks]]` (by title? by path?) or standard relative markdown links (`[text](../space/page.md)`) that render anywhere? How is a Link resolved when it is ambiguous or broken? When a Page is renamed or moved, which Links are rewritten and how? This must stay merge-friendly and keep the files readable in plain editors and on GitHub.

Inputs from [markdown parser research](../research/04-markdown-parser.md): wikilinks (with `|alias`) are built into pulldown-cmark and comrak, but markdown-rs lacks them. Rewrites must splice the original text at byte offsets, since no parser can round-trip a file through its AST unchanged. Note that comrak turned `[[Old Page]]` into `Old%20Page` when it re-serialized.

## Answer

Resolved 2026-09-26 by grilling. The trade-off is recorded in [ADR 0002](../../../docs/adr/0002-path-identity-with-rewrite.md), and the terms (Page Path, Title, Link, Broken Link) are in `CONTEXT.md`.

1. **Identity**: a Page is identified by its **Page Path**, its path from the Wiki root without `.md` (e.g. `eng/rust/async-notes`). There are no stable ids.
2. **Title**: frontmatter `title`, else the first H1, else the filename. Used for display only.
3. **Syntaxes**: both wikilinks (`[[path]]`, `[[path|alias]]`) and standard markdown links (`[text](path.md)`) are Links, with an optional `#heading` target. External URLs are not Links.
4. **Resolving wikilinks**: always the full Page Path from the Wiki root, so they can never be ambiguous. There is no Obsidian-style shortest-unique-name matching.
5. **Resolving standard links**: relative to the file that contains them. A `/`-rooted path resolves from the Wiki root. Percent-encoding (e.g. `%20`) is decoded, and `.md` is required.
6. **What the app writes**: the syntax it inserts is a per-Wiki setting, defaulting to standard relative links. The app never writes `/`-rooted paths.
7. **Broken Links**: allowed and reported; following one offers to create the Page. A missing `#heading` is only a warning, and renaming a heading does not rewrite Links to it.
8. **Rename/move**:
   - It rewrites every Link pointing at the Page, keeping each Link's syntax, text, alias and `#heading`.
   - It also rewrites the moved Page's own relative standard Links.
   - Link text is never changed.
   - It runs as plan then apply, and the planned edits can be returned as a dry run.
   - Edits are spliced at byte offsets, leaving every other byte of the file unchanged.
9. **Attachments**: the same rules for resolving, Backlinks and rewriting (`![[path/diagram.png]]`, `![](diagram.png)`), except the path keeps its extension.
10. **Case**:
    - Page Paths are case-sensitive.
    - Creating or renaming to a path that differs from an existing one only by case is rejected.
    - A Link with no exact match but exactly one match ignoring case still resolves, with a warning.
