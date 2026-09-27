# PROTOTYPE: wikirs GUI layout (ticket 11), throwaway

Answers [GUI editor and layout prototype](../../issues/11-gui-prototype.md). Real gpui + gpui-kit `=0.6.6`, with an in-memory fake Wiki and nothing written to disk.

```sh
cd .scratch/wikirs/prototypes/11-gui-layout
cargo run                      # starts on variant A
WIKIRS_VARIANT=C cargo run     # or start on B / C
```

First build takes about 2 min, and it needs the full Xcode (for Metal).

## Variants (floating bar at the bottom, or ⌘⌥← / ⌘⌥→)

| | Layout | Editing model |
|---|---|---|
| **A · Workbench** | Left: Pages / Tags trees. Right: Backlinks, Links, Outline, Tags | Source and preview side by side, ⌘E for source only |
| **B · Reader** | Space switcher on top, the current Space's tree, and "Linked from" plus Child Pages under the content | Rendered by default; Edit (⌘E) swaps in the source |
| **C · Focus** | No panels, a centred column, and a status bar (Backlinks count, Tags) | Source by default, ⌘E for preview; everything else goes through ⌘P / ⌘K |

([screenshots](shots/))

## Shared in every variant

- **⌘K command palette**: all 32 Operations from a fake registry. Pick one to get a form generated from its schema. Mutations get a dry-run toggle, and Run shows a fake Plan diff.
- **⌘P quick open**: filters by path or Title.
- **`[[` autocomplete** in the editor: type `[[rust` to get Page Paths, and accepting inserts `path]]`.
- **⌘S** saves. A dirty buffer shows "● unsaved" in the breadcrumb.
- **"simulate external edit"** on the bar: with no unsaved edits the Page reloads without prompting. With unsaved edits it shows the red banner (Reload / Keep mine / Compare).
- Clicking a Broken Link (`eng/missing-page` in "wikirs design" → Links) shows where create-from-Broken-Link would go.

## Known fakes and gaps (these are findings, not bugs to fix here)

- The preview rewrites `[[x]]` into ordinary links and strips frontmatter before rendering. The real app needs gpui-kit `MarkdownPlugin`s for both, and links in the preview don't navigate.
- `![diagram](…)`: local images aren't loaded (gpui-kit #2527).
- The live-preview hybrid isn't shown, because gpui-kit's editor can't change font size or hide markup (see the research).
