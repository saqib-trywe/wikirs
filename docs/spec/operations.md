# Operation catalogue

The parity contract: every Operation below is exposed by every Interface (GUI, CLI, TUI, HTTP, MCP). How it is exposed is decided by the parity architecture; this document fixes only *what* exists.

Decided in [Operation catalogue](../../.scratch/wikirs/issues/08-operation-catalogue.md). Terms follow `CONTEXT.md`.

How each Interface exposes these Operations is in [interfaces.md](interfaces.md).

## Kinds

- **Query**: reads the Wiki or the Index; never changes a file.
- **Mutation**: changes files in the Wiki. Every mutation takes `dry_run: bool` and returns the **Plan** it would apply (dry run) or did apply. Apply recomputes the Plan; it never accepts one from the caller. A Plan is applied as one unit; there is no cross-Operation batch.
- **Maintenance**: changes only derived state (the Index), never a Wiki file, so it has no Plan or `dry_run` (`rebuild_index` only).
- **Subscription**: a stream of events (`watch` only).

Granularity is one Operation per user intent, with reads kept cheap and single-purpose: a GUI opening a Page calls `get_page`, `backlinks` and `children` separately.

## Shared types

- **PagePath**: Page Path, no extension (`eng/rust/async-notes`).
- **AttachmentPath**: path from the Wiki root, with extension if the file has one.
- **Version**: opaque content hash of a file, returned by reads and used as `base_version` by writes.
- **Filter**: `{ space?, path_prefix?, tag?, exact?: bool }`. `tag` includes descendant Tags unless `exact`.
- **Scope**: `{ space? | path_prefix? }`, defaulting to the whole Wiki.
- **Plan**: `{ edits: [Edit], warnings: [Warning] }`.
  - **Edit**: `create(path, content) | create_binary(path, size, version) | modify(path, base_version, splices: [{range, old, new}]) | move(from, to) | delete(path)`. Splices are byte ranges into the original file; every other byte is unchanged. `create_binary` (a new Attachment) carries no content: the bytes are staged in the cache dir before the journal is written, so a crashed Plan can still finish it. A path outside the Wiki (the machine settings file) is absolute.
- **Warning**: non-fatal note in a result or Plan, e.g. `link_will_break{from, link}`, `case_fallback`, `mixed_case_tag`, `heading_missing`.
- **Diagnostic**: `{ kind, page, range, message }` where `kind` is `broken_link | heading_missing | case_fallback | mixed_case_tag | unrecovered_edit` (an edit from a crashed Plan that journal recovery left alone because the file had changed; see [process-model.md](process-model.md#mutations)).
- **LinkStatus**: `ok | broken | heading_missing | case_fallback`.

## Errors

A closed set. Each entry below lists which it can return. `Io` and `Internal` are always possible, and every mutation can also return `Conflict` (lock timeout, or a file changed between planning and applying); these aren't repeated. The wire format, `details` and the mapping onto each Interface are in [errors.md](errors.md).

| Error | Meaning |
|---|---|
| `NotFound` | Target Page, Attachment, Tag, config key or Wiki does not exist |
| `AlreadyExists` | Target path is taken |
| `CaseConflict` | Path differs from an existing one only by case |
| `InvalidPath` | Malformed path, or a move into its own subtree |
| `InvalidInput` | Any other invalid argument |
| `Conflict` | File changed since `base_version` or since the Plan was computed, or the Wiki's write lock timed out |
| `NoMatch` / `AmbiguousMatch` | `edit_page`: an `old` string matched zero / several times |
| `Io` | Filesystem failure |
| `Internal` | A bug: a panic caught by the registry, or a broken invariant |

Every Input rejects unknown fields with `InvalidInput`, so a misplaced or misspelled argument (e.g. `list_pages{tag}` instead of `list_pages{filter: {tag}}`) never silently does something else. The JSON schemas say so too (`additionalProperties: false`).

Warnings are never errors. On JSON Interfaces every Output is wrapped as `{ result, warnings }` (see [errors.md](errors.md#success-and-warnings)).

## Wiki

| Operation | Kind | Inputs | Output | Errors |
|---|---|---|---|---|
| `init` | mutation | — | Plan (creates `.wikirs/config.toml`) | `AlreadyExists` |
| `get_config` | query | `key?` | value, or all settings with their source (`wiki` / `machine` / default) | `NotFound` |
| `set_config` | mutation | `key, value \| null, scope: wiki \| machine` | Plan (for `machine`, the edit targets the machine-settings file outside the Wiki) | `NotFound`, `InvalidInput` |

Operations take no `wiki` argument: the Wiki is the process's (or GUI window's) context, see [wiki-selection.md](wiki-selection.md). `init` is the only Operation whose root isn't resolved the usual way: it targets `--wiki` / `WIKIRS_WIKI`, else the cwd, with no walk-up.

The settings are a closed set; any other key is `NotFound`. `get_config` returns `[{ key, value, source, scope, description }]` (one entry when `key` is given), where `scope` is the file the key belongs in.

| Key | Scope | Values (default) |
|---|---|---|
| `links.syntax` | wiki | `standard` \| `wikilink` (`standard`) |
| `ignore` | wiki | globs (`[]`): a pattern without `/` matches a name at any depth, one with `/` is anchored at the root, `*` stops at `/` |
| `search.stemming` | wiki | `none` \| `english` (`none`); changing it rebuilds the Index |
| `watcher` | machine | `native` \| `poll` (`native`) |
| `cache_dir` | machine | a path (the OS cache dir); used from the next open |
| `serve.bind`, `serve.port`, `serve.require_token`, `serve.allowed_hosts`, `serve.cors_origins`, `serve.read_only`, `serve.max_body` | machine | see [http-security.md](http-security.md#settings) |

- A `wiki` key can also be set with `scope: machine`, which overrides the Wiki's value on this machine. A `machine` key with `scope: wiki` is `InvalidInput`: it never goes in the committed file.
- An unreadable file or an invalid value is ignored (the next source applies) with a `bad_setting` warning; `set_config` refuses to edit a file that isn't valid TOML.
- Edits keep the file's comments and layout. A table left empty is removed.

## Pages

| Operation | Kind | Inputs | Output | Errors |
|---|---|---|---|---|
| `get_page` | query | `page` | `{ path, title, content (raw markdown), frontmatter (JSON), tags, version }` | `NotFound` |
| `list_pages` | query | `filter?, sort?: path \| title \| modified, limit?, offset?` | `[{ path, title, tags, modified }]`, total | `InvalidInput` |
| `create_page` | mutation | `path` **or** `parent? + title`; `content?`, `tags?` | Plan; the final path | `AlreadyExists`, `CaseConflict`, `InvalidPath` |
| `write_page` | mutation | `page, content, base_version?` | Plan; new version | `NotFound`, `Conflict` |
| `edit_page` | mutation | `page, [{ old, new }], base_version?` | Plan; new version | `NotFound`, `NoMatch`, `AmbiguousMatch`, `Conflict` |
| `set_page_meta` | mutation | `page, { key: value \| null }` | Plan | `NotFound`, `InvalidInput` (the `tags` key) |
| `move_page` | mutation | `from, to` | Plan | `NotFound`, `AlreadyExists`, `CaseConflict`, `InvalidPath` |
| `delete_page` | mutation | `page, recursive?` | Plan (warnings list Links that will break) | `NotFound`, `InvalidInput` |

- `create_page` from `parent + title` slugifies the filename and writes the Title as an H1. Creating at a Placeholder's path turns it into a Page. Creating from a Broken Link is `create_page{path}` with the Link's target.
- `edit_page`: each `old` must match exactly once; all replacements apply or none do.
- `set_page_meta` splices into frontmatter, creating the block if absent; `null` removes a key. Setting `title` never renames the file.
- `move_page` is also rename. It carries the Page's folder (Child Pages, Attachments) and rewrites every Link to anything moved, plus the moved Pages' own relative Links. Moving onto a Placeholder is allowed and merges the folders; a clashing child path fails the whole Plan. Moving into its own subtree is `InvalidPath`. A Placeholder can be moved (a folder move). Renaming a Space is `move_page` on the Space's path.
- `move_page` rewrites Links by splicing only their path, keeping syntax, alias, `#heading` and text; a case-fallback Link is corrected to the exact new path. **Reference-style Links** (`[text][ref]` with `[ref]: path.md` elsewhere) can't be rewritten in place: the Plan carries a `link_not_rewritten` warning, and `check` reports the Link as broken afterwards.
- `delete_page` removes only the `.md`, leaving a Placeholder; `recursive` removes the subtree. Deleting a Placeholder requires `recursive`. A folder left empty is removed.

## Hierarchy and Spaces

| Operation | Kind | Inputs | Output | Errors |
|---|---|---|---|---|
| `children` | query | `parent?` (Wiki root if absent), `depth = 1` | ordered `[{ path, title, kind: page \| placeholder, has_children, children? }]` | `NotFound` |
| `reorder_page` | mutation | `page, before \| after: sibling` | Plan (writes `order:`; renumbers siblings only when no gap is left) | `NotFound`, `InvalidInput` (not a sibling) |
| `list_spaces` | query | — | `[{ name, home_page?, page_count }]` | — |

There is no `create_space`: a Space exists once a Page exists at or under its path. Reparenting is `move_page`.

- A folder holding only Attachments is neither a Placeholder nor a Space: both need a Page beneath them.
- A Placeholder can't carry `order:`, so it always sorts after its ordered siblings; a `reorder_page` whose result depends on one says so with a `placeholder_unordered` warning.
- `set_page_meta` rejects a non-number `order` (`InvalidInput`).

## Links

| Operation | Kind | Inputs | Output | Errors |
|---|---|---|---|---|
| `links` | query | `page` | `[{ raw, range, target, heading?, status: LinkStatus }]` | `NotFound` |
| `backlinks` | query | `target` (Page or Attachment) | `[{ from, range, raw }]` | — |
| `resolve_link` | query | `from_page, raw` | `{ target, heading?, status: LinkStatus }` | `InvalidInput` (not a Link) |
| `outline` | query | `page` | `[{ level, text, anchor }]` | `NotFound` |
| `check` | query | `scope?, kinds?` | `[Diagnostic]` | — |

`backlinks` on a nonexistent target is not an error: Broken Links point at it. "List Broken Links" is `check{kinds: [broken_link]}`.

## Tags

| Operation | Kind | Inputs | Output | Errors |
|---|---|---|---|---|
| `tag_tree` | query | `scope?` | `[{ tag, direct, inclusive, children }]` | — |
| `tag_page` | mutation | `page, tags[]` | Plan (Tags already carried, frontmatter or inline, are skipped) | `NotFound`, `InvalidInput` (bad Tag syntax) |
| `untag_page` | mutation | `page, tags[]` | Plan (removes frontmatter entries, strips inline `#`) | `NotFound` |
| `rename_tag` | mutation | `from, to` | Plan (moves the subtree; renaming onto an existing Tag merges) | `NotFound`, `InvalidInput` |

`untag_page` matches the exact Tag only, not descendants. Pages by Tag is `list_pages{filter: {tag}}`.

Removing a Page's last frontmatter Tag removes the `tags:` key; if nothing else is left in the frontmatter, the whole `---` block goes too.

## Search

| Operation | Kind | Inputs | Output | Errors |
|---|---|---|---|---|
| `search` | query | `text, filter?, limit?, offset?` | `[{ path, title, snippet, score }]`, total | `InvalidInput` |

Query syntax is plain terms and `"quoted phrases"`; the engine's own syntax is not exposed.

## Attachments

| Operation | Kind | Inputs | Output | Errors |
|---|---|---|---|---|
| `add_attachment` | mutation | `page, name, source: local_path \| base64` | Plan; final path (numeric suffix on clash) | `NotFound`, `InvalidInput` |
| `list_attachments` | query | `page? \| scope?` | `[{ path, size, modified }]` | `NotFound` |
| `read_attachment` | query | `path, as: bytes \| local_path` | bytes (base64 over JSON) or an absolute path | `NotFound` |
| `move_attachment` | mutation | `from, to` | Plan (rewrites Links) | `NotFound`, `AlreadyExists`, `CaseConflict`, `InvalidPath` |
| `delete_attachment` | mutation | `path` | Plan (warnings list Links that will break) | `NotFound` |

Attachments have their own move and delete because an Attachment path and a Page Path can be indistinguishable (extensionless files).

- `add_attachment.source` is `{ local_path }` or `{ base64 }`, exactly one. The Page must exist. A name taken in the folder, ignoring case (a Child Page's folder counts), gets `-2`, `-3`… before the extension, with a `renamed` warning; a path matching `ignore` gets an `ignored` warning. A `local_path` is copied, never moved.
- `list_attachments{page}` lists that folder's own files, not its Child Pages' (a Placeholder's folder works too); `list_attachments{scope}` lists everything in it. Both at once is `InvalidInput`.
- `read_attachment.as` defaults to `bytes`, returned as `base64`. Hidden and ignored files aren't Attachments (`NotFound`).

## Index

| Operation | Kind | Inputs | Output | Errors |
|---|---|---|---|---|
| `index_status` | query | — | `{ root, cache_dir, pages, links, tags, last_updated, stale, watcher: native \| poll \| none, skipped: [{ path, reason: symlink \| non_utf8 \| case_clash }], unrecovered_edits: [{ path, edit }] }` | — |
| `rebuild_index` | maintenance | — | `index_status` after rebuild | — |

`rebuild_index` touches only derived state, so it has no Plan or `dry_run`. There is no refresh Operation: queries bring the Index up to date themselves (mechanism: process model).

## Subscription

| Operation | Kind | Inputs | Output |
|---|---|---|---|
| `watch` | subscription | `scope?` | stream of `{ kind, path, from?, version? }` |

- `kind`: `page_created | page_modified | page_deleted | page_moved | attachment_changed | index_updated`.
- Events carry no content; subscribers call `get_page` as needed.
- A process receives events for its own mutations too. A multi-file Plan emits one event per affected path, then `index_updated`.
- How changes are detected (watcher, debounce, other processes) is decided by the process model.

## Not in the catalogue

- Rendering a Page to HTML, import and export: out of scope for this spec (see the map), and left to a later effort. Any of them can be added through the registry.
- Batch/transaction across Operations.
- A separate merge-Tag, create-Space, reparent or create-from-Broken-Link Operation (each is covered above).
