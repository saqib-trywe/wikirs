# TUI (ratatui)

The shape of the TUI Interface. Decided in [TUI layout and editing prototype](../../.scratch/wikirs/issues/12-tui-prototype.md), after trying three variants in a [throwaway prototype](../../.scratch/wikirs/prototypes/12-tui-layout/README.md). How the TUI consumes the registry is in [interfaces.md](interfaces.md), and its reactions to changes on disk are in [process-model.md](process-model.md#open-page-changed-on-disk-gui-and-tui). The GUI counterpart is [gui.md](gui.md).

## Layout: Columns (prototype variant A)

- The GUI's Workbench layout in the terminal:
  - left: Pages tree (Placeholders dimmed)
  - centre: the rendered Page
  - right: Backlinks, Links (Broken Links marked), Outline and Tags
- The top bar shows the breadcrumb and an unsaved marker, and a one-line banner row appears under it when needed.
- The bottom row shows key hints, or the `:` command line.

## Navigation

- j/k move, and Tab cycles focus between the tree and the Links panel. Enter opens or follows.
- **Numbered Links** (from variant C): Links in the rendered Page are shown as `[1] [2] …`. Typing the number follows that Link.
- Following a Broken Link offers to create the Page (`create_page{path}`).
- `o` quick open (`list_pages` / `search`), `b` Backlinks popup, `t` the Page's tasks: Enter ticks one (saved at once unless there are unsaved edits).

## Editing

- **`E` hands off to `$EDITOR`** (the primary path):
  - The terminal is suspended and the Page is edited as a temp copy.
  - When the editor exits, the text is loaded back into the buffer.
  - Saving uses `write_page` with the `base_version` taken when the Page was opened, so the conflict rules still apply.
- **`e` edits inside the TUI** in a textarea, for quick fixes. ctrl-s saves, and Esc returns to the view.

## Operations

- **`:` command line**: the Operation name with arguments, CLI-style (`:move_page eng/a eng/b --dry-run`). The names are the same as `wikirs <op>`, and Tab completes them.
- **ctrl-k palette**: every Operation, with a form generated from its Input schema and a dry-run toggle. Results and Plans appear in a popup.
- These get their own keys, matching the GUI's list ([gui.md](gui.md#custom-ui-beyond-the-palette)): the tree, following Links, Backlinks, quick open and search, create from a Broken Link, and rename/move with its Plan preview. Everything else is reached through `:` or ctrl-k.

## Keys added in the build

The build (`wikirs tui [page]`, crate `wikirs-tui`) adds these to the keys above:
- `/` full-text search (`search`), next to `o`. `R` opens `move_page` for the open Page as a dry run, and `a` in any dry-run result applies it.
- Leaving a Page with unsaved edits is refused: ctrl-s saves, `D` discards. `q` asks first when there are unsaved edits, and `Q` quits anyway.
- `:` also takes `q`, `w` and `wq`. Operation names work with `_` or `-` (`:move_page` or `:move-page`, the CLI's spelling). `:watch` and the palette's `watch` show the events this TUI has seen.
- A broken `![[file]]` embed reports the missing file and never offers to create a Page.

## Changed on disk

The banner row reads "CHANGED ON DISK": Reload / Keep mine / Compare (r / m / d from the view; Esc first if editing). Compare shows mine and disk side by side.

## Rendering work the build needs

- A ratatui renderer for the core's document model ([ADR 0007](../adr/0007-one-markdown-parse-shared-document-model.md)); presentation per construct is in [markdown.md](markdown.md#presentation).
- **Images** (`ratatui-image`): a paragraph that is only an embedded Attachment keeps its `[n] [image: path]` line and shows the picture in the rows under it. It uses kitty graphics (kitty, Ghostty) or iTerm2 images (iTerm2, WezTerm and others) where the environment says the terminal has them, and half-block characters elsewhere. The terminal is never queried over stdin, because one that didn't answer would hang startup. A picture is drawn only when all of it is on screen. Following its number still opens the file in the system viewer.
