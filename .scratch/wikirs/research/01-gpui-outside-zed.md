# gpui outside Zed: maturity, components, text editing, markdown rendering

Ticket: [01-gpui-outside-zed](../issues/01-gpui-outside-zed.md)
Researched: 2026-09-26. Sources: crates.io API, GitHub repos (zed-industries/zed, longbridge/gpui-kit at HEAD), gpui-kit.com docs.

## 1. Distribution, API churn, release cadence

- **Upstream `gpui` on crates.io is stale.** The latest upstream release is `gpui 0.2.2`, published 2025-10-22. It was the first real publish since 0.1.0 in 2022, after test releases in early October 2025 ([crates.io/crates/gpui](https://crates.io/crates/gpui)). In Zed `main`, `crates/gpui/Cargo.toml` is still `version = "0.2.2"` with `publish = true`, so nothing newer has shipped in about 11 months ([Cargo.toml](https://github.com/zed-industries/zed/blob/main/crates/gpui/Cargo.toml)).
- **Upstream has been re-split since then.** The gpui README on `main` now says to depend on `gpui` plus a separate `gpui_platform` crate (`gpui_platform::application()`). `gpui_platform` does **not** exist on crates.io ([README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md)). Zed's tree now has `gpui_apple`, `gpui_linux`, `gpui_macos`, `gpui_windows`, `gpui_wgpu`, `gpui_web`, `gpui_platform`, and others. In practice, current upstream gpui is **git-only**.
- **Zed's own wording:** "still pre-1.0. There will often be breaking changes between versions. You'll also need to use the latest version of stable Rust." ([README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md))
- **Churn:** 59 commits touched `crates/gpui` in the 30 days to 2026-09-26 (GitHub commits API, `path=crates/gpui`).
- **Third-party snapshots (`gpui-pre-*`):** the Longbridge maintainer (crates.io user `huacnlee`) publishes snapshots of Zed's crates as `gpui-pre`, `gpui-pre-platform`, `gpui-pre-macros`, and so on. The current one is `0.3.6`, described as a "snapshot of zed@bcf6582". Versions 0.3.0–0.3.6 were all released between 2026-09-03 and 2026-09-21, so there is roughly one new snapshot a week ([crates.io/crates/gpui-pre](https://crates.io/crates/gpui-pre)). The gpui-kit README says these are published "with Zed's license and notices intact".
- **Pinning policy:** gpui-kit's workspace `Cargo.toml` pins every snapshot crate with `=0.3.6`. The reason given is that "any snapshot may change GPUI's API… a caret requirement let the weekly release move applications onto a newer snapshot that gpui-component did not compile with (#3156)". CI fails if a pin is not exact ([Cargo.toml](https://github.com/longbridge/gpui-kit/blob/main/Cargo.toml)). This also shows that gpui-kit releases **weekly**.
- **Mixing versions is a known trap.** Open issue [#2532](https://github.com/longbridge/gpui-kit/issues/2532) reports that "Multiple versions of gpui crate cause type incompatibility when using git dependencies".

## 2. Component libraries

**longbridge/gpui-kit** (formerly `gpui-component`) is the dominant library: about 14.8k stars and pushed on 2026-09-26 ([repo](https://github.com/longbridge/gpui-kit)). A crates.io search for "gpui" shows no other component library with real download numbers.

- **Crates:** `gpui-kit` 0.6.6 is the umbrella crate that re-exports GPUI. It layers over `gpui-base`, which holds unstyled behaviour and state, and `gpui-component`, which is the styled layer. All three are at 0.6.6, released 2026-09-21. Licence is Apache-2.0.
- **Release history:** `gpui-component` 0.3.0 (Oct 2025) → 0.4.x (Nov 2025) → 0.5.0 (Dec 2025) → 0.5.1 (Feb 2026) → 0.6.0–0.6.6 (Sep 2026). 0.5.1 depended on crates.io `gpui ^0.2.2`. 0.6.x moved to `gpui-pre =0.3.6` ([crates.io deps](https://crates.io/crates/gpui-component/0.6.6/dependencies)).
- **Breaking changes are ongoing.** The unreleased 0.7.0 in [release-notes.md](https://github.com/longbridge/gpui-kit/blob/main/release-notes.md) already removes `Root::render_dialog_layer` and related APIs.
- **Claimed maturity:** "75+ components" and "Used to build Longbridge Pro from day one" (a shipped commercial desktop app). It also claims AccessKit accessibility, headless UI tests, and WASM support ([README](https://github.com/longbridge/gpui-kit#readme)).
- **Components wikirs needs** (modules under [`crates/component/src`](https://github.com/longbridge/gpui-kit/tree/main/crates/component/src)):
  - **Text editor:** `input/editor.rs` / `EditorState`. Code editor with Tree-sitter, LSP hooks, line numbers, folding, search, and multi-cursor.
  - **Tree view:** `tree.rs`, which the source describes as "A styled tree view".
  - **Tabs:** `tab/`, plus `dock/` for draggable tabs, splits, and serializable panels.
  - **Markdown renderer:** `text/`, which provides `TextView` / `TextViewState::markdown`. HTML rendering is also available.
  - **Other relevant pieces:** sidebar, command palette (`command/`), `virtual_list`, table, resizable, notification, dialog, menu, and `title_bar`.

## 3. Multi-line markdown source editor with highlighting

- **Zed's `Editor` cannot be reused.** Zed's `editor`, `markdown`, `markdown_preview`, and `ui` crates are licensed **GPL-3.0-or-later**. The Zed workspace also sets `publish = false`, so they are not on crates.io and are tightly coupled to Zed's workspace crates ([editor/Cargo.toml](https://github.com/zed-industries/zed/blob/main/crates/editor/Cargo.toml), [workspace Cargo.toml](https://github.com/zed-industries/zed/blob/main/Cargo.toml)). Only `gpui` itself is Apache-2.0.
- **gpui-kit's `EditorState` is the practical option** ([docs](https://gpui-kit.com/component/editor)). It is built with `EditorState::new(window, cx).language(Language::Markdown).line_number(true).searchable(true)`.
  - **Highlighting:** Tree-sitter, behind the `tree-sitter-markdown` and `tree-sitter-markdown-inline` features.
  - **Soft wrap:** supported; `set_soft_wrap` defaults to true (`crates/base/src/input/base/state.rs`).
  - **Other features:** folding, tab size, read-only mode, multi-cursor, and built-in find. `LanguageConfig` offers "a supported subset of Monaco-style language configuration".
  - **Scale:** the README claims "Stable performance at 200K lines".
  - **Backing store:** a `ropey` rope.
- **Hooks for custom styling:**
  - An LSP-style `semantic_tokens_provider`. The markdown example uses it to highlight TODO/FIXME markers.
  - Monaco-like decorations: `TextDecoration { range, style: HighlightStyle }` and `RangeDecoration`, which draws a fill or frame.
- **Wiki links:** `InlineToken` lets an app render a byte range as an "atomic inline editing unit" with a label, which suits `[[wiki links]]` shown as chips. However, tokens are **not supported in code-editor mode** (`supported = !M::CODE_EDITOR && …` in `inline_tokens.rs`). They only work in plain multi-line input mode, which has no syntax highlighting.
- **Known editor bugs:**
  - [#2877](https://github.com/longbridge/gpui-kit/issues/2877): CRLF is kept when pasting from the Windows clipboard.
  - [#3081](https://github.com/longbridge/gpui-kit/issues/3081): the completion trigger character is wrong.
  - [#3183](https://github.com/longbridge/gpui-kit/issues/3183): the focused Input frame has no accessibility node.

## 4. Markdown rendering and live preview

- **Renderer:** `TextView` / `TextViewState::markdown(src, cx)` in gpui-base/gpui-component.
  - **Parser:** the [`markdown`](https://crates.io/crates/markdown) crate (markdown-rs, mdast) with `ParseOptions::gfm()` (`crates/base/src/text/markdown_ext.rs`).
  - **Extensibility:** `MarkdownPlugin`, `MarkdownBlockParserFn`, and `MarkdownBlockRenderFn` let an app add custom block and inline syntax. The example uses this for math, and it could handle wiki links too. `FrontmatterPlugin` handles front matter.
  - **Other features:** tables, Tree-sitter highlighting inside code blocks, selectable text, range highlights (find-in-preview), and a streaming mode.
- **Source + preview works out of the box.** [`examples/markdown`](https://github.com/longbridge/gpui-kit/tree/main/examples/markdown) is exactly that: an `EditorState` in Markdown mode next to a `TextViewState::markdown` preview, updated on `InputEvent`, with find-in-preview. Run it with `cargo run -p example-markdown`.
- **Live-preview hybrid (Obsidian-style) is not built in.**
  - There is no rich-text or WYSIWYG editor component. The JS catalog lists an "Editor – Rich text editor state" story as `availability: "pending"` (`examples/js_story/stories/layouts.js`).
  - Styling source text in place is limited to gpui's `HighlightStyle`, which has these fields: `color`, `font_weight`, `font_style`, `background_color`, `underline`, `strikethrough`, and `fade_out`. It has **no font size**, no hiding or collapsing of markup, and no inline widgets ([style.rs](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/style.rs)).
  - So a light hybrid is achievable: bold and italic spans, coloured headings, dimmed (`fade_out`) markup characters. A real hybrid (big headings, hidden syntax, rendered images or tables inline) would need a custom editor element, or a fork of `EditorState`.
- **WYSIWYG** would mean writing a rich-text editor on raw gpui elements (text layout, `EntityInputHandler` for IME, selection, and undo) with no library support. That is the largest option by far.
- **Images:** open issue [#2527](https://github.com/longbridge/gpui-kit/issues/2527) (Linux, gpui 0.2.2 / component 0.5.1) reports local images that take up layout space but render blank. This matters for wiki pages with embedded images.

## 5. Platforms and pain points

- **Supported platforms:** macOS (Metal), Windows (Win32 + DirectWrite text), and Linux/FreeBSD (Wayland and/or X11 features) ([gpui README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md)). gpui-kit also claims web support via `wasm32-unknown-unknown`, through `gpui_web`.
- **macOS build:** the full Xcode is required, not just the Command Line Tools, because Metal shaders are compiled at build time. gpui-kit enables `runtime_shaders` on `gpui-pre-platform`. The `font-kit` feature is needed, or glyphs will not render.
- **Linux:** needs a Vulkan-capable GPU (the renderer is now `gpui_wgpu`). Zed's Linux docs list driver failures: `NoSupportedDeviceFound`, amdvlk crashes, and PRIME/discrete-GPU selection problems ([docs/src/linux.md](https://github.com/zed-industries/zed/blob/main/docs/src/linux.md)). gpui-kit [#3035](https://github.com/longbridge/gpui-kit/issues/3035) reports no window decorations on Wayland from `TitleBar`.
- **Windows:** gpui-kit has several open TitleBar issues ([#2018](https://github.com/longbridge/gpui-kit/issues/2018), [#2496](https://github.com/longbridge/gpui-kit/issues/2496)). Open issue counts matching the search terms were windows 24, linux 8, wayland 6, IME 2 (GitHub search, 2026-09-26; keyword matches, not triaged).
- **macOS memory leaks:** closed windows keep Metal resources alive ([#3052](https://github.com/longbridge/gpui-kit/issues/3052), [#3107](https://github.com/longbridge/gpui-kit/issues/3107)).
- **General:**
  - The pre-1.0 API changes weekly, and the version must be pinned exactly.
  - Upstream's crates.io release has stalled, so the ecosystem depends on one company's snapshot crates.
  - Documentation is mostly gpui-kit's; upstream gpui docs are thin.
  - Builds are heavy (gpui plus Tree-sitter grammars).

## Implications for choosing an editor mode (facts, not a decision)

| Option | What exists today | Gap to build |
|---|---|---|
| Source + preview | `EditorState` (Markdown Tree-sitter) + `TextView::markdown` (GFM, plugins); a working example exists | Wiki-link plugin, image loading (see #2527), syncing scroll between panes |
| Live-preview hybrid | Colour, weight, italic, underline, and fade decorations on source; semantic-token hook | No font-size changes, markup hiding, or inline widgets in the code editor; needs a custom element or fork |
| WYSIWYG | Nothing reusable (Zed's editor is GPL and unpublished; the gpui-kit rich-text editor is "pending") | A full rich-text editor on raw gpui |

## Sources
- https://crates.io/crates/gpui, https://crates.io/crates/gpui-pre, https://crates.io/crates/gpui-component, https://crates.io/crates/gpui-kit, https://crates.io/crates/gpui-base
- https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md
- https://github.com/zed-industries/zed/blob/main/crates/gpui/Cargo.toml
- https://github.com/zed-industries/zed/blob/main/Cargo.toml (workspace `publish = false`)
- https://github.com/zed-industries/zed/blob/main/crates/editor/Cargo.toml (GPL-3.0-or-later)
- https://github.com/zed-industries/zed/blob/main/crates/gpui/src/style.rs (HighlightStyle)
- https://github.com/zed-industries/zed/blob/main/docs/src/linux.md
- https://github.com/longbridge/gpui-kit (README, Cargo.toml, release-notes.md, crates/base/src/input, crates/base/src/text, examples/markdown)
- https://gpui-kit.com/component/editor
- gpui-kit issues: #2018, #2496, #2527, #2532, #2877, #3035, #3052, #3081, #3107, #3183
