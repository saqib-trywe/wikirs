# PROTOTYPE: wikirs TUI layout (ticket 12), throwaway

Answers [TUI layout and editing prototype](../../issues/12-tui-prototype.md). ratatui 0.30 + ratatui-textarea 0.9, with an in-memory fake Wiki. The only file it writes is a temp file for the `$EDITOR` hand-off.

```sh
cd .scratch/wikirs/prototypes/12-tui-layout
cargo run                          # variant A; F2 cycles A → B → C
cargo run -- --variant C
cargo run -- --snapshot B form     # print one frame as text (see shots/)
```

## Variants

| | Layout | Navigating Links |
|---|---|---|
| **A · Columns** | Pages tree, rendered Page, and Backlinks/Links/Outline/Tags (the same shape as the GUI Workbench) | Tab moves focus tree → Links panel, j/k picks a Link, Enter follows it |
| **B · Miller** | ranger-style: parent column, siblings column, Page preview with Backlinks and Children under it | h/l go up and down the hierarchy, j/k move between siblings, Tab cycles Links in the Page, Enter follows |
| **C · Pager** | one full-width Page, like `less` | Links are numbered `[1]`, and typing the number follows it |

## Shared

- **Editing**: `e` edits inline (textarea; ctrl-s saves, Esc returns to the view), and `E` hands off to `$EDITOR`. Both are included so you can compare them.
- **Operations**:
  - `:` opens a command line: `:move_page eng/rust/async-notes eng/rust/async --dry-run`, with Tab completing Operation names.
  - ctrl-k opens the palette: all 32 Operations, a generated form, ctrl-d toggles dry run, and results appear as a Plan popup.
- **Navigation**: `o` quick open, `b` (or `B` in C) opens the Backlinks popup, and a Broken Link (`eng/missing-page` in "wikirs design") offers `c` to create it.
- **Changed on disk**: `X` simulates an external edit. With no unsaved edits the Page reloads without prompting. With unsaved edits you get the banner (Esc, then r / m / d for Reload / Keep mine / Compare).

## Findings

- The rendered view is hand-rolled (headings, bullets, Links, images as `[image: …]`). A real TUI needs a markdown→ratatui renderer: it can walk pulldown-cmark events, since the Index already parses with that.
- Images: `ratatui-image` (kitty/sixel/iTerm protocols) exists and could show Attachments in capable terminals. The prototype shows a placeholder.
