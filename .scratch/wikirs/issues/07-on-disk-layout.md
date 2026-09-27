# On-disk layout: Spaces, page hierarchy, Attachments, config

Map: [wikirs map](../map.md)
Type: grilling
Status: resolved
Assignee: Saqib
Blocked by: 05

## Question

What does a Wiki look like on disk? Is a Space a top-level folder, and does it have its own metadata file? Is page hierarchy expressed through folders (`parent.md` + `parent/child.md`, or `parent/index.md`) or through a `parent:` frontmatter field? How are sibling Pages ordered? Where do Attachments live: next to their Page, or in a per-Space folder? Where do the Wiki config and the Index cache go (for example `.wikirs/`), and which of those are gitignored? Everything has to stay merge-friendly, with no files that every edit rewrites.

Inputs from [Page identity and Link syntax](05-page-identity-and-link-syntax.md): identity is the Page Path (no ids); renaming a Page rewrites every Link to it. Decide here whether moving a Page also moves its Child Pages and Attachments (and so rewrites Links to them too). Paths that differ only by case are rejected. The Wiki config must hold the setting for which Link syntax the app inserts.

## Answer

Resolved 2026-09-26 by grilling. The hierarchy trade-off is recorded in [ADR 0003](../../../docs/adr/0003-hierarchy-by-sibling-folder.md). The terms (Space, Page, Child Page, Placeholder) are updated in `CONTEXT.md`.

1. **Spaces**: every top-level folder that isn't hidden is a Space, with no marker needed. Pages at the Wiki root are allowed and belong to no Space. A Space's home Page is `<space>.md` at the Wiki root. There's no separate Space metadata file. Renaming a Space is a folder move, which rewrites Links.
2. **Hierarchy**:
   - A Page's Child Pages live in the folder next to it with the same name (`parent.md` + `parent/child.md`), so the folder tree is the hierarchy.
   - A folder with no matching `.md` is a **Placeholder**: it's shown by its folder name, and creating `parent.md` turns it into a real Page.
   - There's no `index.md` convention and no `parent:` frontmatter field.
3. **`.wikirs/` at the Wiki root**:
   - `config.toml` is committed. It holds the Link syntax the app writes, `ignore` patterns and other Wiki settings.
   - `local.toml` holds per-machine overrides and is not synced.
   - `.wikirs/.gitignore` ignores `local.toml`. The app never touches the user's root `.gitignore`.
   - The Index lives in the **OS cache dir**, keyed per Wiki, not inside the Wiki (it's safe under Dropbox). Its location can be overridden in `local.toml`.
   - `.wikirs/` isn't needed to open a folder as a Wiki: it's created when a setting is first saved, or by `wikirs init`. When present, it marks the Wiki root for walk-up discovery.
4. **What's ignored**:
   - Hidden files and folders (`.git`, `.obsidian`, `.wikirs`, …) and the config `ignore` list. `.gitignore` is **not** honoured.
   - Symlinks are not followed and are reported as skipped.
   - Files with non-UTF-8 names or content are skipped, with a warning in the Index status.
5. **Sibling order**:
   - Each Page can carry an optional `order:` number in its frontmatter. Pages with an `order` come first, sorted by it, and the rest follow in natural sort by Title.
   - Placeholders sort by name.
   - Reorder writes only the moved Page, using gaps of 10, and renumbers its siblings only when no gap is left.
6. **Attachments**:
   - Any file that isn't `.md` or ignored, anywhere in the Wiki, is an Attachment.
   - The add Attachment Operation puts new ones in the Page's own folder (`eng/rust.md` → `eng/rust/diagram.png`) and adds a numeric suffix if the name clashes.
7. **Move and delete**:
   - Moving a Page moves its folder, with every Child Page and Attachment, as one plan that rewrites Links to all of them.
   - Deleting a Page removes only its `.md`, so its folder stays behind as a Placeholder. A `recursive` flag deletes the whole subtree.
   - Every delete plan lists the Links that will become Broken Links.
8. **Creating a Page from a Title**:
   - The filename is slugified: Unicode letters are kept and lowercased, and everything else becomes `-` (so "Async Notes, Part 2" becomes `async-notes-part-2.md`).
   - The Title is written as a `# H1`, not in frontmatter.
   - Changing the Title never renames the file. Any valid filename is accepted when reading.

**Amended 2026-09-27** by [Where per-machine Wiki settings live](18-per-machine-settings-location.md): `local.toml` and `.wikirs/.gitignore` are gone. Machine settings live at `~/.config/wikirs/wikis/<wiki key>.toml`, outside the Wiki, so no sync tool carries them, and `.wikirs/` holds only `config.toml`.
