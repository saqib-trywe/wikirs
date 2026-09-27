# Operation catalogue

Map: [wikirs map](../map.md)
Type: grilling
Status: resolved
Assignee: Saqib
Blocked by: 05, 06, 07

## Question

What is the complete list of Operations, with their inputs, outputs and failure modes? Candidate groups: Page CRUD and move/rename, hierarchy (children, reparent, reorder), Links and Backlinks (including broken Links), Tags (list tree, tag/untag, rename, merge), search, Attachments (add, list, remove), Spaces, and Index maintenance (rebuild, status). Which Operations are queries and which are mutations? Is there a notion of change events or subscriptions (the GUI and TUI need to react to edits made elsewhere)? This catalogue is the parity contract.

Inputs from [Page identity and Link syntax](05-page-identity-and-link-syntax.md): Operations needed include rename/move with a plan-then-apply dry run, listing Broken Links and heading warnings, Backlinks for both Pages and Attachments, and creating a Page from a Broken Link.

Inputs from [Tag model](06-tag-model.md): the Tag Operations are a Tag tree (with direct and inclusive counts per node), querying Pages by Tag (descendants by default, with an `exact` flag), tag and untag Page (untag strips inline `#` as a planned edit), and rename Tag (moves the subtree, doubles as merge, plan then apply with a dry run). There's no separate merge Operation and no Tag metadata.

Inputs from [On-disk layout](07-on-disk-layout.md):
- **Hierarchy**: children lists Child Pages and Placeholders; reorder writes `order:`.
- **Move**: moves the subtree as one plan.
- **Delete**: leaves a Placeholder by default, with a `recursive` flag and the Broken Links listed in the plan.
- **Create Page from a Title**: slugifies the filename and writes the Title as an H1.
- **Add Attachment**: puts it in the Page's folder, with a numeric suffix on clashes.
- **Index status**: reports skipped symlinks and non-UTF-8 files.
- **Spaces**: a Space's home Page is `<space>.md`, and renaming a Space is a folder move.

## Answer

Resolved 2026-09-26 by grilling. The full catalogue (32 Operations with inputs, outputs and errors) is in [docs/spec/operations.md](../../../docs/spec/operations.md). **Plan** is added to `CONTEXT.md`. There's no ADR, because the catalogue is additive and cheap to change.

1. **Kinds**: query, mutation, subscription (`watch`). `rebuild_index` is maintenance with no Plan.
2. **Granularity**: one Operation per thing a user means to do, with reads kept cheap and single-purpose (`get_page`, `backlinks`, `children` are separate).
3. **Plan then apply**: every mutation takes `dry_run` and returns its Plan (file edits as byte-range splices, plus warnings). Apply works out the plan again and never accepts one from the caller. There's no batching across Operations.
4. **Writing content**: `write_page` (full replace, optional `base_version`) and `edit_page` (exact-string replacements, each must match once). `set_page_meta` edits frontmatter keys other than `tags`.
5. **Pages**: `create_page` takes a path or parent + title (slugified). Creating from a Broken Link is a plain create. `move_page` also covers rename, reparent and Space rename; moving onto a Placeholder merges, and moving into the Page's own subtree is InvalidPath. `delete_page` leaves a Placeholder unless `recursive`.
6. **Hierarchy and Spaces**: `children(parent?, depth)`, `reorder_page(before|after sibling)`, `list_spaces`. There's no create Space.
7. **Links**: `links`, `backlinks` (Pages and Attachments), `resolve_link`, `outline`, and `check` for all diagnostics (Broken Links, missing headings, case fallbacks, mixed-case Tags).
8. **Tags**: `tag_tree`, `tag_page`, `untag_page`, `rename_tag`. Pages by Tag is a `list_pages` filter.
9. **Search**: `search(text, filter)` with plain terms and phrases, with the engine's syntax hidden. `list_pages` shares the same filter.
10. **Attachments**: add (from a local path or base64), list, read, move, delete. These are separate from the Page Operations because the paths can be ambiguous.
11. **Wiki and Index**: `init`, `get_config`, `set_config(scope: wiki|local)`, `index_status`, `rebuild_index`. There's no refresh Operation.
12. **Events**: `watch(scope?)` streams path-level events with no content, and includes the process's own mutations.
13. **Errors**: a closed set: NotFound, AlreadyExists, CaseConflict, InvalidPath, InvalidInput, Conflict, NoMatch/AmbiguousMatch, Io. Warnings are never errors.
14. **Not Operations**: rendering to HTML (moved to Not yet specified, under import/export), batching.
