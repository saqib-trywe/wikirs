# GUI editor and layout prototype

Map: [wikirs map](../map.md)
Type: prototype
Status: resolved
Assignee: Saqib
Blocked by: 01, 08

## Question

What should the gpui GUI look like and how should it behave? Build a rough prototype covering: layout (Space/page tree, Tag tree, editor, Backlinks panel), the editing model (source+preview vs live-preview, based on the gpui research), Link autocomplete, and how every Operation in the catalogue is reached (command palette?). The goal is to settle the GUI's shape, not to build it.

Inputs from [gpui research](../research/01-gpui-outside-zed.md): source+preview works with gpui-kit today (see its `examples/markdown`); a live-preview hybrid needs a custom editor element; WYSIWYG is not available. Local images render blank (gpui-component #2527), which affects how Attachments are shown.

Input from [Parity architecture](09-parity-architecture.md): every Operation is reachable through a command palette generated from the registry (schema-driven forms, dry-run toggle showing the Plan), and that sets the minimum parity. This prototype decides which Operations also get bespoke UI (tree, editor, backlinks, …). The UI calls typed Operations in-process, and `watch` arrives as a channel to bridge into the event loop.

Input from [Process model and concurrency](10-process-model-concurrency.md): when an open Page changes on disk, the view reloads without prompting if there are no unsaved edits. Otherwise it keeps the buffer and shows a "changed on disk" banner (Reload / Keep mine / Compare). It follows `page_moved` events, and on an external delete it marks the buffer as deleted. Saves always send `base_version`, and a `Conflict` must be shown to the user. See [process-model.md](../../../docs/spec/process-model.md#open-page-changed-on-disk-gui-and-tui).


## Answer

Resolved 2026-09-26 by prototype. The prototype is [11-gui-layout](../prototypes/11-gui-layout/README.md), with three variants in real gpui-kit 0.6.6 (the repo has no commits yet, so it's kept here and not on a throwaway branch). The result is in [docs/spec/gui.md](../../../docs/spec/gui.md).

1. **Layout**: variant A, "Workbench": a left sidebar with Pages and Tags trees, the Page in the centre, and a right panel with Backlinks, Links, Outline and Tags.
2. **Editing model**: from variant B, a Page opens rendered, and ⌘E switches to source and preview side by side. There's no live-preview hybrid and no WYSIWYG.
3. **Custom UI beyond the palette**:
   - the page tree with a context menu
   - a rename/move dialog showing the Plan
   - Backlinks and Links
   - the Tags tree
   - ⌘P quick open and search
   - Broken Link → create
   - `[[` autocomplete
   - Outline

   Everything else is palette-only.
4. **Changed on disk**: keep the banner. Compare becomes a real diff in the build.
5. **Findings**: gpui-kit's preview needs plugins for wikilinks and frontmatter, and local images are still blank (#2527). `[[` autocomplete works through its `CompletionProvider`. The first build takes about 1.5 min.
