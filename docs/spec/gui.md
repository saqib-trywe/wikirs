# GUI (gpui + gpui-kit)

The shape of the GUI Interface. Decided in [GUI editor and layout prototype](../../.scratch/wikirs/issues/11-gui-prototype.md), after trying three variants in a [throwaway prototype](../../.scratch/wikirs/prototypes/11-gui-layout/README.md). How the GUI consumes the registry is in [interfaces.md](interfaces.md), and its reactions to changes on disk are in [process-model.md](process-model.md#open-page-changed-on-disk-gui-and-tui).

## Layout: Workbench (prototype variant A as the base)

- **Left sidebar** has two tabs:
  - **Pages**: the page hierarchy, with Placeholders shown in muted italics.
  - **Tags**: the Tag tree with direct and inclusive counts.
- **Centre**: breadcrumb (Titles along the Page Path, plus an unsaved marker), the change-on-disk banner, then the Page.
- **Right panel**: Backlinks, outgoing Links (Broken Links marked), Outline, and Tags.
- The panels are resizable.

## Editing model

- A Page opens **rendered** (reading first, as in prototype variant B).
- **⌘E** switches to **source and preview side by side**, and ⌘E again returns to the rendered view.
- There's no live-preview hybrid and no WYSIWYG. gpui-kit's editor can't vary font size or hide markup (see the [gpui research](../../.scratch/wikirs/research/01-gpui-outside-zed.md)).
- ⌘S saves via `write_page` with `base_version`.

## Custom UI (beyond the palette)

The generated ⌘K palette reaches every Operation. These also get their own UI:

| UI | Operations |
|---|---|
| Page tree + context menu | `children`, `create_page`, `move_page`, `delete_page`, `reorder_page` |
| Rename / move dialog showing the dry-run Plan before applying | `move_page` (and `rename_tag` from the Tags tree) |
| Backlinks and Links panel | `backlinks`, `links` |
| Tags tree | `tag_tree`, `list_pages{tag}` |
| ⌘P quick open + search | `list_pages`, `search` |
| Clicking a Broken Link offers to create the Page | `resolve_link`, `create_page{path}` |
| `[[` autocomplete in the editor | `list_pages{path_prefix}` / `search` (through gpui-kit's `CompletionProvider`) |
| Outline panel | `outline` |

Everything else (config, attachments management, `check`, Index maintenance, `watch` log…) is palette-only for now.

## Changed on disk

This is the banner from the prototype (Reload / Keep mine / Compare). In the real build, Compare is a proper side-by-side diff.

## As built

`wikirs gui [page]` (crate `wikirs-gui`, gpui-kit `=0.6.6`) drives the same open-Page `Session` as the TUI (`wikirs-ui`):
- ⌘S and ⌘E are ctrl-S and ctrl-E on Linux and Windows.
- Clicking a Placeholder in the tree, or a Broken Link, shows a banner with "Create it".
- Following a `[[page#heading]]` Link scrolls to the heading.
- Watch events are taken in every 250 ms on the UI thread.

Still to come (the next slice): the ⌘K palette, ⌘P quick open and search, the move dialog with its Plan, the tree's context menu, `[[` autocomplete, a real diff for Compare, Tag → Pages, and inline images.

## Rendering work the build needs

The prototype found gaps in gpui-kit 0.6.6's `TextView`, and [ADR 0007](../adr/0007-one-markdown-parse-shared-document-model.md) replaces it for Page rendering:

- **A gpui renderer for the core's document model** (see [markdown.md](markdown.md)). It reuses gpui-kit components (code highlighting, table, text) but not its markdown parser, so wikilinks, frontmatter and Link navigation all come from the core.
- Local image loading for Attachments (gpui-kit #2527: local images render blank).
- gpui / gpui-kit pinned to exact versions (weekly breaking snapshots).
