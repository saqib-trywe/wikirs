# Map: wikirs — local markdown wiki with GUI/CLI/TUI/HTTP/MCP parity

Label: wayfinder:map

## Destination

An implementation-ready spec for wikirs: domain model, on-disk format, core architecture and Operation catalogue, the parity contract across GUI (gpui), CLI/TUI (ratatui), HTTP and MCP, plus ADRs for the hard-to-reverse choices. Done when nothing is left to decide before build slicing starts.

## Notes

- **Domain**: personal knowledge base. One user, possibly several machines; sync happens outside the app (git/Dropbox), so the on-disk format must be merge-friendly. The `.md` files are the only source of truth; the Index is a disposable cache.
- **Vocabulary**: `CONTEXT.md` at the repo root is the glossary. Use its terms (Wiki, Space, Page, Link, Backlink, Tag, Index, Operation, Interface) and update it as terms sharpen.
- **Skills**: grilling tickets call `grilling` and `domain-modeling`. Architecture tickets (the Operation registry and the process model) should also consult `codebase-design`.
- **Charting decisions** (settled while charting the map):
  - Destination is a spec (planning only). A walking skeleton is the first build slice after handoff, not part of this map.
  - One binary, no daemon: every Interface is a subcommand, and all of them link the core. See [ADR 0001](../../docs/adr/0001-single-binary-no-daemon.md).
  - Parity is enforced by an Operation registry in the core, with every Interface as a thin adapter over it. The details belong to the "Parity architecture" ticket.
  - Confluence features in scope: Spaces (as folders), page hierarchy, Backlinks, full-text search, Attachments, and rename/move with link rewriting.
  - Pages are also exposed as MCP resources, as a read-only view over the read Operations, so it doesn't break parity. The details belong to the "Parity architecture" ticket.
  - Parity is a hard requirement of the *contract*. Rolling out the Interfaces may happen in stages after handoff.
- **Tracker**: local markdown. Tickets live in `issues/`, and research findings in `research/`. The repo has no commits yet, so research is written here rather than on `research/*` branches.

## Decisions so far

<!-- one line per closed ticket: [title](link): gist -->

- [Rust MCP SDK choice and transports](issues/02-rust-mcp-sdk.md): official `rmcp` 3.4.x, stdio + Streamable HTTP (axum-mountable); tools can be registered at runtime from the registry (`ToolRoute::new_dyn`, schemars schemas); resources are hand-written (Pages will be resources; how is decided in the parity ticket)
- [gpui outside Zed: maturity, components, text editing, markdown rendering](issues/01-gpui-outside-zed.md): viable through Longbridge's gpui-kit, but the gpui version must be pinned exactly (weekly breaking snapshots); its editor and markdown view make source+preview ready to use, a live-preview hybrid needs a custom editor element, and there is no WYSIWYG support; local images currently render blank
- [Search and Index engine: tantivy vs SQLite FTS5 vs in-memory](issues/03-search-index-engine.md): every option rebuilds 10k pages in about 1 s; SQLite+FTS5 handles structured queries and search in one store, and its WAL mode lets writers from several processes queue; tantivy searches better but allows only one writer process at a time and needs batched commits
- [Markdown parser: link/tag extraction and source-preserving rewrite](issues/04-markdown-parser.md): pulldown-cmark 0.13 is the only candidate with byte ranges, built-in wikilinks and fast parsing together; no parser can re-serialize a file without reformatting it, so link rewrites must splice the original text at the link's offsets; no parser handles inline `#tag`, and frontmatter needs a separate YAML crate
- [Page identity and Link syntax](issues/05-page-identity-and-link-syntax.md): a Page is identified by its Page Path (no ids, [ADR 0002](../../docs/adr/0002-path-identity-with-rewrite.md)); both wikilinks (full path from the Wiki root) and relative standard links are read, and which one the app writes is a per-Wiki setting; renames rewrite Links in place as plan then apply; Broken Links are allowed
- [Tag model: syntax, canonical source, hierarchy semantics](issues/06-tag-model.md): Tags live in frontmatter and inline `#a/b` as a union, and the app writes to frontmatter; matching ignores case and the app writes lowercase; there's no Tag registry; queries include descendants by default; rename Tag moves the subtree and doubles as merge; untag strips the inline `#`; edits are spliced plan then apply
- [On-disk layout: Spaces, page hierarchy, Attachments, config](issues/07-on-disk-layout.md): the folder tree is the hierarchy (`parent.md` + `parent/child.md`, a folder with no `.md` is a Placeholder, [ADR 0003](../../docs/adr/0003-hierarchy-by-sibling-folder.md)); top-level folders are Spaces; `.wikirs/config.toml` is committed (machine settings moved out of the Wiki, see below); the Index goes in the OS cache dir; optional `order:` in frontmatter; Attachments go in the Page's folder; moves carry the subtree and deletes leave a Placeholder
- [Operation catalogue](issues/08-operation-catalogue.md): 32 Operations in [docs/spec/operations.md](../../docs/spec/operations.md), grouped as queries, mutations (each with `dry_run` → Plan) and a `watch` subscription; `write_page` + `edit_page` handle content; the set of error kinds is closed; rendering and batching are not Operations
- [Parity architecture: Operation registry, adapters, parity test](issues/09-parity-architecture.md): a trait per Operation plus one `operations![]` macro, serde/schemars as the only schema, and a sync core ([ADR 0004](../../docs/adr/0004-operation-registry-generated-adapters.md)); the CLI, HTTP RPC (`POST /ops/{name}`) and MCP tools are generated from it, and the GUI and TUI get a generated command palette; Pages and Attachments are MCP resources projected from the read Operations; a three-layer parity test; see [docs/spec/interfaces.md](../../docs/spec/interfaces.md)
- [Process model and concurrency](issues/10-process-model-concurrency.md): a shared SQLite+FTS5 Index in the OS cache dir ([ADR 0005](../../docs/adr/0005-sqlite-fts5-shared-index.md)); a Wiki-wide write lock, hash-checked atomic writes and a roll-forward journal ([ADR 0006](../../docs/adr/0006-write-lock-and-roll-forward-journal.md)); a reconcile scan at open plus debounced watchers keep the Index fresh; open buffers reload without prompting or show a conflict banner; see [docs/spec/process-model.md](../../docs/spec/process-model.md)
- [GUI editor and layout prototype](issues/11-gui-prototype.md): a "Workbench" layout (Pages/Tags trees, the Page, a Backlinks/Links/Outline panel); Pages open rendered, and ⌘E switches to source+preview; custom UI for the tree, rename with Plan preview, Backlinks, Tags, quick open, Broken Link → create, and `[[` autocomplete, with the rest in the palette; gpui-kit needs wikilink, frontmatter and local-image work; see [docs/spec/gui.md](../../docs/spec/gui.md)
- [TUI layout and editing prototype](issues/12-tui-prototype.md): the GUI's Workbench shape in columns, plus numbered Links to follow; `$EDITOR` hand-off is the main way to edit, with an inline textarea for quick fixes; both a `:` command line (CLI names) and the ctrl-k palette; a pulldown-cmark → ratatui renderer, with images as placeholders; see [docs/spec/tui.md](../../docs/spec/tui.md)
- [Error mapping across Interfaces](issues/13-error-mapping.md): `{error: {kind, message, details}}` (kind and details are the contract); a new `internal` kind for caught panics; `{result, warnings}` wraps every Output; CLI exit codes 0–7, HTTP 400/404/409/422/500/503, MCP `isError` tool results; save conflicts go to the changed-on-disk flow; see [docs/spec/errors.md](../../docs/spec/errors.md)
- [Selecting a Wiki](issues/14-selecting-a-wiki.md): one Wiki per process (one per window in the GUI) and no `wiki` argument on Operations; resolved from `--wiki` → env → walk-up to `.wikirs/` → `default_wiki` → error; named Wikis in a user-level config; `mcp` never guesses from the cwd; the GUI has a welcome window and recent list; see [docs/spec/wiki-selection.md](../../docs/spec/wiki-selection.md)
- [HTTP and MCP-over-HTTP security](issues/15-http-security.md): loopback on port 4747 by default; remote needs `--allow-remote` plus a bearer token, which is stored in the state dir outside the Wiki; Host/Origin/Content-Type checks in front of `/ops` and `/mcp`; separate transport errors (401/403/413); `--read-only` for `serve` and `mcp` as a capability; no built-in TLS; see [docs/spec/http-security.md](../../docs/spec/http-security.md)
- [Crate/workspace layout and feature flags](issues/16-crate-workspace-layout.md): one deep `wikirs-core` crate plus thin adapter crates (cli, mcp, http, tui, gui) and `wikirs-forms`, which is shared by the two palettes; the root is the binary; default features `gui`/`tui`/`serve`/`mcp`, and `--no-default-features` gives a headless CLI; gpui-kit and the toolchain are pinned exactly; see [docs/spec/workspace.md](../../docs/spec/workspace.md)
- [Supported markdown extensions](issues/17-markdown-extensions.md): CommonMark + GFM, footnotes, alerts, math, frontmatter and wikilinks; pulldown-cmark in the core is the only parser, and the GUI and TUI render a shared document model instead of gpui-kit's `TextView` ([ADR 0007](../../docs/adr/0007-one-markdown-parse-shared-document-model.md)); raw HTML is never rendered; see [docs/spec/markdown.md](../../docs/spec/markdown.md)
- [Where per-machine Wiki settings live](issues/18-per-machine-settings-location.md): machine settings move out of the Wiki to `~/.config/wikirs/wikis/<wiki key>.toml` (`~/.config` on every OS), so Dropbox and iCloud can't carry them; the key is the root-path hash, with "adopt" after a move; the config scopes are `wiki` / `machine`; see [wiki-selection.md](../../docs/spec/wiki-selection.md#machine-settings)
- [Testing strategy](issues/19-testing-strategy.md): fixture Wikis plus a builder; `insta` goldens for splicing; proptest for Plan properties and "incremental Index = rebuild"; real-process tests for the write lock and crash recovery; TUI frame snapshots and GUI logic tested outside gpui; a CI matrix split by OS; see [docs/spec/testing.md](../../docs/spec/testing.md)

## Not yet specified

Nothing left: the way to the destination is clear (2026-09-27). The next step is build slicing, starting with the walking skeleton.


## Out of scope

- Sync, multi-user editing, and hosting: the app is local-only and single-user, and sync is external.
- Git integration (page history, auto-commit): git stays the user's business. History could be added later as Operations.
- Comments: they don't fit a markdown-only source of truth.
- Templates: deferred to a later effort.
- Import/export and rendering to HTML (a Confluence importer, a static HTML export, a `render_page` Operation): ruled out of this spec on 2026-09-27, to be a later effort. Nothing in the design blocks it: an exporter can walk the core's document model ([ADR 0007](../../docs/adr/0007-one-markdown-parse-shared-document-model.md)), an importer is a batch of `create_page`/`add_attachment` calls, and any new Operation goes through the registry.
- Tag metadata (descriptions, colours, aliases): a registry file would change on many edits and cause merge conflicts, and Tags exist only through the Pages that carry them. See [Tag model](issues/06-tag-model.md).
