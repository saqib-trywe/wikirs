# Markdown parser: comrak vs pulldown-cmark vs markdown-rs — findings

Ticket: [04-markdown-parser](../issues/04-markdown-parser.md). Researched 2026-09-26 against crates.io, docs.rs, the published crate sources (`~/.cargo/registry`, exact versions below), GitHub issue trackers, and a small throwaway probe program (`/tmp/mdprobe`, not in the repo) that ran all three crates on the same sample page.

## Candidates (crates.io, 2026-09-26)

| Crate | Latest | Last publish | Downloads (total / 90d) | MSRV | License | Repo |
|---|---|---|---|---|---|---|
| `pulldown-cmark` | 0.13.4 | 2026-05-20 | 160M / 48.7M | 1.71.1 | MIT | <https://github.com/pulldown-cmark/pulldown-cmark> |
| `comrak` | 0.55.0 | 2026-09-06 | 8.1M / 2.4M | 1.85 | BSD-2-Clause | <https://github.com/kivikakk/comrak> |
| `markdown` (markdown-rs) | 1.0.0 | 2025-04-23 | 10.8M / 3.4M | 1.56 | MIT | <https://github.com/wooorm/markdown-rs> |

Source: `https://crates.io/api/v1/crates/<name>` and `/versions`. comrak releases roughly monthly (0.48 in Nov 2025 → 0.55 in Sep 2026, breaking minor bumps). pulldown-cmark 0.13.x has shipped patch releases through 2026. markdown-rs's last commit on `main` is 2025-04-23 (the 1.0.0 release) — <https://github.com/wooorm/markdown-rs/commits/main>.

## Summary table

| | pulldown-cmark 0.13.4 | comrak 0.55.0 | markdown-rs 1.0.0 |
|---|---|---|---|
| Model | Pull event stream (`Event`s), no tree | Arena AST (`Node<'a>`), mutable | mdast AST (`mdast::Node`) or direct HTML |
| Source position | **Byte `Range<usize>`** per event via `into_offset_iter()` | `Sourcepos { start, end: LineColumn }` — 1-based line + column; column in UTF-8 bytes by default; end is **inclusive** | `Position { start, end: Point{line, column, offset} }` — has a 0-based `offset`; end is exclusive |
| Tables / task lists / footnotes | Yes (`ENABLE_TABLES`, `ENABLE_TASKLISTS`, `ENABLE_FOOTNOTES` GFM-style) | Yes (`table`, `tasklist`, `footnotes`, + `inline_footnotes`) | Yes (`ParseOptions::gfm()`) |
| Wikilinks `[[...]]` | **Built in** since 0.13.0: `ENABLE_WIKILINKS` → `LinkType::WikiLink { has_pothole }` | **Built in**: `wikilinks_title_after_pipe` / `wikilinks_title_before_pipe` → `NodeValue::WikiLink(NodeWikiLink { url })` | **No.** `[[x]]` comes through as plain `Text`. No plugin/extension hook ("not a goal … to support lots of different extensions") |
| Frontmatter | `ENABLE_YAML_STYLE_METADATA_BLOCKS` (`---`) and `ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS` (`+++`); yields `Tag::MetadataBlock` + raw text | `front_matter_delimiter = Some("---")` → `NodeValue::FrontMatter(String)` raw | `constructs.frontmatter` → `Yaml` / `Toml` nodes, raw string |
| Serialize back to markdown | Only via third-party `pulldown-cmark-to-cmark` (22.0.1, 2026-08-10) — reformats | `format_commonmark` — **reformats** (see test below) | Separate `mdast_util_to_markdown` crate in the repo — reformats |
| HTML | `pulldown_cmark::html::push_html` | `markdown_to_html` / `format_html` (+ `render.sourcepos` data attrs, syntax-highlight plugins) | `to_html` / `to_html_with_options` |
| Perf (probe, release, M-series Mac) | fastest | ~4–5× slower than pulldown, still linear | **superlinear** — unusable on MB-sized files |

Sources: Options — <https://docs.rs/pulldown-cmark/latest/pulldown_cmark/struct.Options.html>; LinkType — <https://docs.rs/pulldown-cmark/latest/pulldown_cmark/enum.LinkType.html>; `into_offset_iter` — <https://docs.rs/pulldown-cmark/latest/pulldown_cmark/struct.Parser.html>; wikilinks added in 0.13.0 — <https://github.com/pulldown-cmark/pulldown-cmark/releases>; comrak Extension — <https://docs.rs/comrak/latest/comrak/options/struct.Extension.html>; comrak `Sourcepos`/`LineColumn`/`NodeWikiLink` — `comrak-0.55.0/src/nodes.rs` (lines ~352, 841–905); markdown-rs `Point`/`Position` — `markdown-1.0.0/src/unist.rs`; markdown-rs extension list and non-goal — `markdown-1.0.0/readme.md` "Extensions" section (<https://github.com/wooorm/markdown-rs#extensions>).

## The key question: in-place link rewrite, rest of file byte-identical

None of the three offers a lossless AST → markdown round trip. The only way to keep the rest of the file byte-identical is **offset-based splicing**: parse, collect the source span of each Link, then `String::replace_range` on the original text (applied back-to-front). Each crate's suitability for that:

### Probe result (sample page with frontmatter, `*`/`+` bullets, task list, table, footnote, reference link, indented code, inline code, trailing-space hard break, non-ASCII)

**pulldown-cmark** — offsets exact and directly usable:
```
metadata start YamlStyle range=0..44
link WikiLink { has_pothole: true }  dest="Old Page"     range=95..113  src="[[Old Page|alias]]"
link WikiLink { has_pothole: false } dest="Old Page"     range=140..152 src="[[Old Page]]"
link Inline    dest="Old%20Page.md"   range=157..182 src="[text](Old%20Page.md \"t\")"
link Reference dest="./Old%20Page.md" range=187..195 src="[ref][r]"
link WikiLink (in table cell)         range=220..232
link WikiLink dest="Old Page#sec" (in footnote) range=271..287
```
Splicing "Old Page" → "New Page" inside those ranges changed exactly the 4 intended lines; every other byte was untouched. `[[...]]` inside indented code and inline code was correctly **not** reported as a link.

Caveats:
- The range covers the whole link (`[text](dest "title")`), not the destination alone; you must locate the destination inside the span yourself. Open issue #441 (since 2020) asks for title/dest sub-ranges — <https://github.com/pulldown-cmark/pulldown-cmark/issues/441>.
- For reference-style links the target lives in the definition (`[r]: ./Old%20Page.md`); its span is available as `LinkDef.span: Range<usize>` via `reference_definitions()` on the parser/`OffsetIter` (`pulldown-cmark-0.13.4/src/parse.rs` ~1811, 2169).
- Open offset bugs: #1129 `into_offset_iter()` panics on a crafted 9-byte fuzz input (opened 2026-08-07, 0.13.4) — <https://github.com/pulldown-cmark/pulldown-cmark/issues/1129>; #852 indented code range excludes indent; #1132 heading-attributes + lone CR. 0.13.3 (2026-03-22) fixed wikilink offset calculation. Worth wrapping parsing in `catch_unwind` or fuzzing.

**comrak** — positions correct, but line/column, not byte offsets:
```
frontmatter sourcepos=1:1-4:3
wikilink url="Old Page" sourcepos=9:12-9:29     (inclusive end; 18 bytes = "[[Old Page|alias]]")
link url="Old%20Page.md" sourcepos=12:22-12:46
```
Converting to byte offsets needs a line-start table (columns are UTF-8 bytes by default, 1-based, end inclusive), which is straightforward. `NodeWikiLink` stores only `url` (the label is child text); open issue #536 notes link vs wikilink children inconsistency — <https://github.com/kivikakk/comrak/issues/536>. Docs note the description-lists extension is "Not (yet) compatible with render.sourcepos" (`src/parser/options.rs` ~196).

**comrak round-trip via `format_commonmark` is NOT source-preserving.** On the same sample it changed: `*`→`-` bullets and inserted `<!-- end list -->` between the `*` and `+` lists; `[[Old Page|alias]]` → `[[Old%20Page|alias]]`; `[[Old Page]]` → `[[Old%20Page|Old Page]]`; table delimiter row `|---|:-:|` → `| --- | :-: |` and escaped `\|` in cells; reference link `[ref][r]` inlined and definition dropped; footnote definition moved to the end and re-indented; trailing-space hard break → `\`. So AST-edit-then-serialize would rewrite far more than the link.

**markdown-rs** — has byte offsets (for Rust `&str` input, `offset` indexed correctly into the source in the probe: `157..182` identical to pulldown), and exposes `Definition` nodes with positions for reference links. But wikilinks are absent: all `[[...]]` came out as `Text` (e.g. `"See [[Old Page]] and "`), so wikilink extraction would require a second hand-written scanner over Text nodes (and you'd need to handle code exclusion yourself — Text nodes already exclude code, which helps).

## Performance (probe, `--release`)

Synthetic page repeated (headings, emphasis, wikilink, inline link, task list, 2×2 table, footnote per section):

| Input | pulldown parse (offset iter) | comrak `parse_document` | markdown-rs `to_mdast` |
|---|---|---|---|
| 100 KB | 0.9 ms | 3.5 ms | 83 ms |
| 1 MB | 9.1 ms | 35 ms | **5.4 s** |
| 11 MB | 98 ms | 467 ms | (skipped) |

HTML rendering of a 6.6 MB file: pulldown 58 ms, comrak 249 ms, markdown-rs 106 s. markdown-rs's 10× input → 65× time is consistent with open upstream issues: #218 "GFM tables: parsing is quadratic in cell count" (opened 2026-08-21) and #113 "larger MDX files are unmanageably slow" (2024) — <https://github.com/wooorm/markdown-rs/issues?q=is%3Aissue+performance>. Disabling footnotes did not help (5.2 s at 1 MB). Numbers are one machine, one synthetic corpus — indicative only.

## Rendering for GUI / HTTP / TUI

- **HTML (GUI webview, HTTP):** all three render HTML natively. comrak additionally offers `render.sourcepos` (`data-sourcepos` attributes) and plugin hooks for syntax highlighting; comrak also has `link_url_rewriter` / `image_url_rewriter` callbacks useful for turning wikilink targets into app URLs at render time.
- **TUI:** none renders to a terminal directly. `tui-markdown` 0.3.10 (2026-09-25, ratatui) is built on `pulldown-cmark ^0.13` — <https://github.com/joshka/tui-markdown>. `termimad` 0.35.5 uses its own `minimad` parser, not any of the three.

## Things none of them do

- **Frontmatter parsing:** all three return the frontmatter block as a raw string; a separate YAML (or TOML) crate is needed to read tags/metadata, and rewriting frontmatter byte-identically has the same splicing concern.
- **Inline `#tag` syntax:** not a CommonMark/GFM construct in any of them; if Tags can appear in body text, a custom scanner over text events/nodes is needed. (CONTEXT.md does not currently say whether Tags live only in frontmatter.)

## Trade-offs to weigh (no recommendation made)

- pulldown-cmark: byte ranges + built-in wikilinks + fastest + TUI ecosystem; but no tree (you build what you need from events), link ranges are whole-link not dest-only, and a few open offset/panic edge-case bugs.
- comrak: richest extension set, real mutable AST, actively released; but positions are line/col (needs conversion), ~4–5× slower, frequent breaking minor versions, higher MSRV (1.85), and its serializer must not be used for rewrites.
- markdown-rs: most spec-faithful (tested against cmark reference behavior), clean mdast with byte offsets; but no wikilinks and no extension hook, superlinear on large files, and no commits since April 2025.
- Mixing is possible (e.g. one parser for indexing/rewrite offsets, another for HTML) at the cost of two dialects that may disagree on edge cases.
