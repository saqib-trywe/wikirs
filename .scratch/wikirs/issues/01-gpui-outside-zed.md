# gpui outside Zed: maturity, components, text editing, markdown rendering

Map: [wikirs map](../map.md)
Type: research
Status: resolved
Blocked by: —

## Question

How practical is gpui for a standalone app today? Find out:
- Is it published on crates.io or git-only? How often does the API change, and what is the release cadence?
- Which component libraries exist (for example, longbridge's gpui-component), and do they include a text editor, a tree view, tabs, and a markdown renderer?
- How would a multi-line markdown source editor with syntax highlighting be built? Is there an Editor widget that can be reused outside Zed?
- Can markdown be rendered into a view (and with which crate), and could a live-preview hybrid be done?
- Which platforms are supported (macOS, Linux, Windows), and what are the known pain points?
Output: the facts the GUI editor prototype needs in order to choose between a source+preview editor, a live-preview hybrid, and WYSIWYG.

## Answer

- **Distribution:** upstream `gpui` last shipped to crates.io as 0.2.2 (2025-10-22). Zed `main` has since split out `gpui_platform` and friends, which are unpublished, so current upstream is git-only. Zed calls gpui "pre-1.0… often breaking changes".
- **Snapshots:** Longbridge publishes weekly `gpui-pre` snapshots (0.3.6, zed@bcf6582, released 2026-09-21). gpui-kit pins them with `=` because every snapshot can break the API.
- **Components:** gpui-kit / gpui-component 0.6.6 (Apache-2.0, about 14.8k stars, used in Longbridge Pro) has a code editor, tree, tabs plus a dock layout, and a markdown `TextView` (markdown-rs, GFM, plugin hooks).
- **Editor:** Zed's own `Editor` / `markdown` crates are GPL-3.0 and `publish = false`, so they can't be reused. gpui-kit's `EditorState` is the practical editor: Tree-sitter Markdown highlighting, soft wrap, line numbers, folding, find, decorations, and an LSP-style hook.
- **Source + preview already exists** as a gpui-kit example (`examples/markdown`: editor next to a live `TextView`).
- **Hybrid and WYSIWYG:** a live-preview hybrid is limited to colour, weight, italic, underline, and fade styling of the source. There is no font size, no hidden markup, and no inline widgets. WYSIWYG has no library support; gpui-kit's rich-text editor is marked "pending".
- **Platforms:** macOS (needs Metal and full Xcode), Windows, and Linux (Vulkan/wgpu; X11 and Wayland). Known issues: GPU driver failures on Linux, no Wayland decorations, Windows TitleBar bugs, Metal leaks on macOS, and local images rendering blank (#2527).

[findings](../research/01-gpui-outside-zed.md)
