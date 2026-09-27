# Crate/workspace layout and feature flags

Map: [wikirs map](../map.md)
Type: grilling
Status: resolved
Assignee: Saqib
Blocked by: —

## Question

How is the cargo workspace split? Candidates: a core crate (Wiki handle, Operations, the `operations![]` registry, the Index, the watcher), one crate per adapter (cli, http, mcp, tui, gui), and a thin `wikirs` binary crate. Which cargo features gate which Interfaces (for example, a headless build without gpui, and whether tokio/axum/rmcp are pulled in only by `serve`/`mcp`)? Where do the pinned gpui/gpui-kit versions live? Does the palette's schema-form logic go in a crate shared by the GUI and TUI? Should `codebase-design` be consulted? Context: ADRs [0001](../../../docs/adr/0001-single-binary-no-daemon.md), [0004](../../../docs/adr/0004-operation-registry-generated-adapters.md), [0005](../../../docs/adr/0005-sqlite-fts5-shared-index.md), and [gpui findings](../research/01-gpui-outside-zed.md).

## Answer

Resolved 2026-09-26 by grilling. The result is in [docs/spec/workspace.md](../../../docs/spec/workspace.md). No ADR: moving crate boundaries is cheap before code exists.

1. **Core**: one deep `wikirs-core` crate (Operations, registry, markdown, Index, watcher, lock, journal). Internal seams are modules, not crates.
2. **Adapters**: `wikirs-cli`, `wikirs-mcp`, `wikirs-http` (mounts mcp), `wikirs-tui`, `wikirs-gui`, plus `wikirs-forms` (the schema → form model shared by the GUI and TUI palettes).
3. **Root**: the root is both the workspace and the thin `wikirs` binary package. Everything is `publish = false`.
4. **Features**: default `gui`, `tui`, `serve`, `mcp`, with `serve` implying `mcp`. `--no-default-features` gives a headless CLI with no gpui and no tokio. Missing subcommands show as "not built in".
5. **clap**: an optional `clap` feature on the core derives `Args` on Inputs.
6. **Pins**: gpui-kit pinned exactly in workspace deps and used only by `wikirs-gui`. The lockfile is committed, and `rust-toolchain.toml` is pinned.
7. **Tests**: the parity test lives in the root `tests/` (`--all-features`), and CI also checks `--no-default-features`.
8. **Settings**: edition 2024, workspace lints (`unsafe_code = forbid`, clippy pedantic as warnings).
