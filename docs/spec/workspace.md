# Cargo workspace and feature flags

How the code is split into crates, and what each build contains. Decided in [Crate/workspace layout and feature flags](../../.scratch/wikirs/issues/16-crate-workspace-layout.md). The architecture it packages is in ADRs [0001](../adr/0001-single-binary-no-daemon.md), [0004](../adr/0004-operation-registry-generated-adapters.md) and [0005](../adr/0005-sqlite-fts5-shared-index.md).

## Layout

```
Cargo.toml              # workspace root AND the `wikirs` binary package
src/main.rs             # thin: resolve the Wiki, dispatch subcommands
tests/                  # parity test (coverage + behaviour + catalogue snapshot)
rust-toolchain.toml     # pinned stable toolchain
crates/
  wikirs-core/          # Wiki handle, Operations, Operation trait + operations![] registry,
                        # markdown parse/splicing + document model (ADR 0007), SQLite Index,
                        # watcher, write lock, journal
  wikirs-forms/         # schema → form model (fields, validation, dry-run toggle), no UI code
  wikirs-cli/           # clap subcommands + Render
  wikirs-mcp/           # rmcp tools + resources
  wikirs-http/          # axum RPC routes, security middleware, mounts wikirs-mcp at /mcp
  wikirs-tui/           # ratatui
  wikirs-gui/           # gpui-kit
```

- **`wikirs-core` is one crate.** It's one deep module behind a small module interface: `Wiki::open`, `run`, and the registry. Internal modules keep their own seams for unit tests. They become separate crates only once a second consumer needs them.
- **`wikirs-forms`** is a real seam, because two UIs (GUI and TUI) render its form model.
- All crates are `publish = false` for now.

## Dependency direction

```
wikirs (bin) ──► wikirs-cli ─┐
            ├──► wikirs-http ──► wikirs-mcp ─┤
            ├──► wikirs-mcp ────────────────┤
            ├──► wikirs-tui ──► wikirs-forms ┼──► wikirs-core
            └──► wikirs-gui ──► wikirs-forms ┘
```

Adapters depend only on `wikirs-core` (and `wikirs-forms`), never on each other, except that `wikirs-http` mounts `wikirs-mcp`.

## Features

| Crate | Feature | Effect |
|---|---|---|
| `wikirs` (bin) | `default = ["gui", "tui", "serve", "mcp"]` | the full build |
| | `serve` | `wikirs-http` (tokio, axum); implies `mcp` |
| | `mcp` | `wikirs-mcp` (tokio, rmcp) |
| | `tui` | `wikirs-tui` (ratatui) |
| | `gui` | `wikirs-gui` (gpui, gpui-kit) |
| `wikirs-core` | `clap` | derives `clap::Args` on Inputs through `cfg_attr`. Turned on by `wikirs-cli` |
| `wikirs-core` | `test-hooks` | compiles in fault injection for tests (e.g. `WIKIRS_TEST_CRASH_AFTER_EDITS`, see [testing.md](testing.md)). Never enabled in release builds |

- `--no-default-features` builds a **headless, CLI-only** binary, with no gpui and no tokio.
- A subcommand whose feature is off still appears in `--help`, marked "not built in".
- `serde` and `schemars` are always on in the core, because the registry catalogue needs them.

## Pins and settings

- `[workspace.dependencies]` shares versions. `gpui-kit` is pinned **exactly** (`=0.6.x`), and only `wikirs-gui` uses it.
- `Cargo.lock` is committed. `rust-toolchain.toml` pins a stable version, and a gpui-kit upgrade is one PR that bumps both the pin and the toolchain.
- Edition 2024. `[workspace.lints]`: `unsafe_code = "forbid"`, and clippy `pedantic` as warnings.

## Tests and CI

- **Parity test**: in the root package's `tests/`, run with `cargo test --all-features`. It calls the adapters in-process (see [interfaces.md](interfaces.md#parity-test)).
- **Unit tests** live in each crate.
- **CI** also runs `cargo check --no-default-features`, to keep the headless build working.
