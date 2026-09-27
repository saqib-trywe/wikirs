# wikirs

A local-only, markdown-native wiki for one person across their machines, operable identically through a GUI, CLI/TUI, HTTP API and MCP server.

## Language

### Content

**Wiki**:
The root folder of markdown files that is the sole source of truth; everything else is derived from it.
_Avoid_: Vault, workspace, repository

**Space**:
A top-level, non-hidden folder within a Wiki that groups related Pages. Pages at the Wiki root belong to no Space. A Space's home Page, if any, is the Page with the same path as the Space (`eng.md` for the `eng` Space).
_Avoid_: Project, notebook, section

**Page**:
A single `.md` file within a Wiki (usually inside a Space), with optional frontmatter metadata.
_Avoid_: Note, document, article

**Page Path**:
A Page's identity: its path relative to the Wiki root, without the `.md` extension (e.g. `eng/rust/async-notes`). Renaming or moving a Page changes its Page Path.
_Avoid_: Page id, slug, key

**Title**:
A Page's display name: frontmatter `title`, else its first level-1 heading, else its filename. Used only for display, never for identity.
_Avoid_: Name, heading

**Child Page**:
A Page positioned beneath another Page in the page hierarchy. Its position is its Page Path: `eng/rust/async` is a Child Page of `eng/rust`.
_Avoid_: Subpage, nested page

**Placeholder**:
A position in the page hierarchy that has Child Pages but no Page of its own (a folder with no matching `.md`). Creating the Page turns it into an ordinary parent.
_Avoid_: Empty page, folder node

**Attachment**:
A non-markdown file (image, PDF, etc.) stored alongside Pages and referenced from them.
_Avoid_: Asset, upload, file

### Connections

**Link**:
A reference written in a Page's markdown to another Page or an Attachment, in either wikilink (`[[...]]`) or standard markdown (`[text](path.md)`) form, optionally targeting a heading. References to external URLs are not Links.
_Avoid_: Reference, cross-reference, external link

**Broken Link**:
A Link whose target Page or Attachment does not exist. It is allowed and reported, not rejected. A Link to an existing Page with a missing heading is not broken, only warned about.
_Avoid_: Dead link, red link, dangling link

**Backlink**:
A Link viewed from its target: the set of Pages that link to a given Page.
_Avoid_: Inbound link, mention

**Tag**:
A hierarchical, case-insensitive label (e.g. `lang/rust/async`) attached to a Page, written in its frontmatter or inline in its body (`#lang/rust`); both count equally. A Page tagged with a Tag is implicitly under each of its ancestor Tags. A Tag exists only while some Page carries it or one of its descendants; there is no separate Tag registry.
_Avoid_: Label, category, keyword

**Inline Tag**:
A Tag written in a Page's body text as `#tag`, as opposed to in its frontmatter.
_Avoid_: Hashtag

### Derived state

**Index**:
A disposable, rebuildable cache of Links, Backlinks, Tags and search data derived from the Wiki's files.
_Avoid_: Database, store

### Interaction

**Operation**:
A single user-meaningful action on the Wiki (e.g. create Page, rename Page, list Backlinks), defined once and exposed identically by every Interface.
_Avoid_: Command, endpoint, tool, action

**Plan**:
The full set of file edits an Operation that changes the Wiki will make, plus warnings such as Links that will break. Every such Operation can return its Plan without applying it (a dry run).
_Avoid_: Preview, diff, changeset

**Interface**:
One of the ways to invoke Operations: GUI, CLI, TUI, HTTP, MCP.
_Avoid_: Frontend, client, mode
