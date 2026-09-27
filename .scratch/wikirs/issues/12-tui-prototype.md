# TUI layout and editing prototype

Map: [wikirs map](../map.md)
Type: prototype
Status: resolved
Assignee: Saqib
Blocked by: 08

## Question

What should the ratatui TUI look like and how should it behave? Should it edit Pages inside the TUI (a textarea widget) or hand off to $EDITOR? What is the layout (tree, Page view, Backlinks, Tags), how are Links navigated and followed, and how are the catalogue's Operations reached (keymap, command line)? Build a rough prototype to react to.

Input from [Parity architecture](09-parity-architecture.md): every Operation is reachable through a command palette generated from the registry (schema-driven forms, dry-run toggle showing the Plan), and that sets the minimum parity. This prototype decides which Operations also get bespoke UI (tree, editor, backlinks, …). The UI calls typed Operations in-process, and `watch` arrives as a channel to bridge into the event loop.

Input from [Process model and concurrency](10-process-model-concurrency.md): when an open Page changes on disk, the view reloads without prompting if there are no unsaved edits. Otherwise it keeps the buffer and shows a "changed on disk" banner (Reload / Keep mine / Compare). It follows `page_moved` events, and on an external delete it marks the buffer as deleted. Saves always send `base_version`, and a `Conflict` must be shown to the user. See [process-model.md](../../../docs/spec/process-model.md#open-page-changed-on-disk-gui-and-tui).

Input from [GUI editor and layout prototype](11-gui-prototype.md): the GUI settled on a "Workbench" layout (Pages/Tags trees, the Page, a Backlinks/Links/Outline panel), rendered-first with ⌘E to switch to source+preview, and a set of Operations with custom UI (see [gui.md](../../../docs/spec/gui.md#custom-ui-beyond-the-palette)). The TUI needn't copy it, but consistent keybindings and names help. The prototype's scaffolding pattern (fake Wiki, fake OPS registry, variant switcher) is reusable.


## Answer

Resolved 2026-09-26 by prototype. The prototype is [12-tui-layout](../prototypes/12-tui-layout/README.md), with three variants in ratatui 0.30 and text snapshots in `shots/`. It's kept here, not on a throwaway branch, because the repo has no commits yet. The result is in [docs/spec/tui.md](../../../docs/spec/tui.md).

1. **Layout**: variant A, "Columns", the GUI Workbench shape (tree | rendered Page | Backlinks/Links/Outline/Tags), plus variant C's numbered Links (type the number to follow).
2. **Editing**: `$EDITOR` hand-off is the main way to edit, and the inline textarea is for quick fixes. Saves go through `write_page` with `base_version`, as in the GUI.
3. **Operations**: both a `:` command line (the same names as the CLI, Tab completion) and the ctrl-k generated palette. The keys that get custom UI match the GUI's list.
4. **Rendering**: a markdown → ratatui renderer that walks pulldown-cmark events. Images show as placeholders and open in the system viewer. `ratatui-image` is the candidate if inline images come later.
