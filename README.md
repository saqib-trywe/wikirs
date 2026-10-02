# wikirs

A local-only, markdown-native wiki for one person across their machines. A Wiki is a folder of `.md` files, and those files are the only source of truth. Everything else (the search index, the link graph) is derived from them and rebuilt when needed. Sync the folder however you like (git, Syncthing, a cloud drive).

One binary, no daemon. Every Operation (create, move, link, tag, search, …) can be run identically from the CLI, the terminal UI, an HTTP API and an MCP server for AI agents. All of these are generated from one registry, so they can't drift apart.

**Status:** all 32 Operations, the CLI, MCP, HTTP and the TUI are built. The desktop GUI is next.

## What it does

- **Pages** are markdown files. A Page's identity is its path (`eng/rust/async-notes`), and folders give the hierarchy: `eng/rust/async.md` is a Child Page of `eng/rust.md`.
- **Links**: `[[eng/rust]]`, `[[eng/rust|Rust]]`, `[[eng/rust#Pinning]]` and ordinary `[text](../rust.md)` links. Backlinks, broken-link checks, and **moving or renaming a Page rewrites every Link to it**.
- **Tags**: `#lang/rust` inline or `tags:` in frontmatter, hierarchical, with rename and merge.
- **Attachments**: images and other files next to Pages, embedded with `![[diagram.png]]`.
- **Search**: SQLite FTS5 full-text search over the Wiki.
- **Safe writes**: every mutation is a Plan you can preview with `--dry-run`. Writes check the version you read, so a stale edit is refused rather than silently overwriting someone else's. A write lock and a roll-forward journal mean a crash never leaves a half-applied change.
- **Markdown**: CommonMark and GFM (tables, task lists, strikethrough), footnotes, alerts (`> [!NOTE]`), math and YAML frontmatter.

## Build

Needs Rust 1.98.1 (pinned in `rust-toolchain.toml`; rustup picks it up).

```sh
cargo build --release          # target/release/wikirs
cargo install --path .         # or put `wikirs` on your PATH
```

`cargo build --no-default-features` builds a headless, CLI-only binary without MCP, HTTP, the TUI, or tokio.

## Quick start

```sh
mkdir notes && cd notes
wikirs init                                         # creates .wikirs/config.toml
wikirs create-page --path eng/rust --content "# Rust

See [[eng/rust/async]]. #lang/rust"
wikirs create-page --path eng/rust/async --content "# Async notes"
wikirs backlinks eng/rust/async
wikirs move-page eng/rust/async eng/async --dry-run  # shows the Plan as a diff
wikirs move-page eng/rust/async eng/async            # moves it and rewrites the Link
wikirs search rust
wikirs tui
```

`wikirs --help` lists every Operation, and `wikirs <operation> --help` describes each one. Add `--json` for machine-readable output, or `--input '{…}'` to pass a whole Input as JSON. Exit codes are listed in [errors.md](docs/spec/errors.md).

## Interfaces

| | Run | Notes |
|---|---|---|
| **CLI** | `wikirs <operation> …` | Subcommands are kebab-case (`move-page`). |
| **TUI** | `wikirs tui [page]` | Pages tree, rendered Page with numbered Links (type the number to follow one), Backlinks/Outline/Tags. `e` edits, `E` opens `$EDITOR`, `:` runs an Operation, ctrl-k is the palette. See [tui.md](docs/spec/tui.md). |
| **MCP** | `wikirs mcp --wiki <path or name>` | stdio, for AI agents. Every Operation is a tool, Pages are resources, and changes arrive as notifications. Add `--read-only` to expose queries only. |
| **HTTP** | `wikirs serve` | `POST /ops/<operation>` with a JSON Input, `GET /ops` for the catalogue, `GET /ops/watch` for change events (SSE), and MCP over HTTP at `/mcp`. Listens on localhost:4747 by default. See [http-security.md](docs/spec/http-security.md) for tokens and remote access. |

An MCP client config looks like this:

```json
{ "mcpServers": { "wiki": { "command": "wikirs", "args": ["mcp", "--wiki", "/path/to/notes"] } } }
```

## Which Wiki?

The CLI, TUI and `serve` use the first of these:
1. `--wiki <path or name>`
2. `WIKIRS_WIKI`
3. the nearest folder above the current directory that contains `.wikirs/`
4. `default_wiki` in your user config

`mcp` skips the walk-up, because an agent's working directory is unpredictable. Named Wikis let the same config work on every machine. Details are in [wiki-selection.md](docs/spec/wiki-selection.md).

## Documentation

- [CONTEXT.md](CONTEXT.md): the vocabulary (Wiki, Space, Page, Placeholder, Link, Tag, Plan, …)
- [docs/spec/](docs/spec/): how each part behaves. [operations.md](docs/spec/operations.md) has the full Operation reference, generated from the registry.
- [docs/adr/](docs/adr/): the architecture decisions and why they were made

## Development

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

The workspace is split into crates:
- `wikirs-core`: the Wiki, Operations, Index, markdown parser and document model
- `wikirs-forms`: schema-driven palette forms
- adapters: `wikirs-cli`, `wikirs-mcp`, `wikirs-http`, `wikirs-tui`

Adapters depend only on the core, never on each other. A parity test checks that every Interface exposes every Operation with identical results. `UPDATE_DOCS=1 cargo test --test docs` regenerates the Operation reference. Testing is described in [testing.md](docs/spec/testing.md), and the crate layout in [workspace.md](docs/spec/workspace.md).
