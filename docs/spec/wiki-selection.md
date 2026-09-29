# Selecting a Wiki

How each Interface finds the root it passes to `Wiki::open(root)`. Decided in [Selecting a Wiki](../../.scratch/wikirs/issues/14-selecting-a-wiki.md). The layout of a Wiki itself (`.wikirs/`, config, cache) is in the [On-disk layout](../../.scratch/wikirs/issues/07-on-disk-layout.md) and [process-model.md](process-model.md#per-wiki-cache-dir).

## Wikis per process

| Interface | Wikis |
|---|---|
| CLI, TUI, `mcp`, `serve` | exactly one per process (serving two Wikis = two `serve`s on two ports) |
| GUI | one per **window**; one process may hold several windows, each with its own `Wiki` handle and watcher |

Operations never take a `wiki` argument. The Wiki is the context of the process or window, so the catalogue and the parity contract are unaffected.

## Resolution order (CLI, TUI, `serve`)

First match wins:

1. `--wiki <path | name>`
2. `WIKIRS_WIKI` (same syntax)
3. walk up from the cwd to the nearest folder containing `.wikirs/` (nested Wikis resolve to the innermost)
4. `default_wiki` in the user config
5. otherwise an error: `not_found` with `what: wiki` and the hint `run 'wikirs init' here, pass --wiki, or set default_wiki`

**`mcp`** skips step 3 and ignores MCP `roots`: an MCP client's cwd is unpredictable, and guessing could silently point an agent at the wrong Wiki. It uses only the flag, env var or `default_wiki`, and otherwise fails.

**`init`** skips this order: it targets `--wiki` / `WIKIRS_WIKI` if given, and otherwise the cwd (no walk-up, since it creates the `.wikirs/` that walk-up looks for).

The chosen root is shown by `--verbose` and in `index_status`.

## `--wiki` values

- A value containing `/`, or starting with `~` or `.`, is a path.
- Anything else is first looked up as a **named Wiki** in the user config, then tried as a relative path.

Named Wikis let one MCP client config (`wikirs mcp --wiki notes`) work on every machine even when the paths differ.

## Folders without `.wikirs/`

They can be opened, but only explicitly: `--wiki`, the env var, `default_wiki`, or the GUI's Open Folder. Nothing is created in them until a `wiki`-scope setting is saved or `init` runs. (Machine settings live outside the Wiki, so saving one creates nothing inside it.) Walk-up never guesses a Wiki from a folder of `.md` files.

## User-level files (per machine, outside any Wiki)

| File | Written by | Holds |
|---|---|---|
| `~/.config/wikirs/config.toml` | the user | `default_wiki = "…"`, `[wikis] notes = "~/notes"` |
| `~/.config/wikirs/wikis/<wiki key>.toml` | the user or `set_config(scope: machine)` | **machine settings** for one Wiki (see below) |
| `<state dir>/wikirs/recent.json` | the app | recently opened Wikis, and the GUI's last-session windows |

- **Config** is always under `~/.config/wikirs/` (or `$XDG_CONFIG_HOME/wikirs/`) on every OS, including macOS and Windows, so it's in the one place users expect to find dotfiles.
- `WIKIRS_CONFIG_DIR` replaces `~/.config/wikirs` (like `WIKIRS_CACHE_DIR` for the cache), so tests and CI never touch the user's settings.
- **State** (`<state dir>`) and **cache** keep the OS conventions (e.g. `~/.local/state`, and `~/Library/Application Support` / `~/Library/Caches` on macOS).

## Machine settings

Settings that belong to one machine's use of one Wiki: `[serve]` ([http-security.md](http-security.md#settings)), `watcher`, and the Index location override ([process-model.md](process-model.md)). They live **outside the Wiki**, so no sync tool (git, Dropbox, iCloud) carries them to other machines. `.wikirs/` holds only the committed, shared `config.toml`.

- **`<wiki key>`** is the same hash of the canonical root path that keys the cache dir and the serve token.
- **Recorded root**: each file records its Wiki's `root = "…"`.
- **Moving a Wiki** changes its key. On open, if no settings file matches but a file's recorded `root` no longer exists, the app offers to **adopt** it: a GUI prompt, or the CLI hint `wikirs config adopt`. The cache rebuilds and the token regenerates without this.
- **Config scopes** are named after these two files: `wiki` (`.wikirs/config.toml`, committed) and `machine` (this file).

## GUI

- On launch, the windows from the last session reopen.
- With no previous session: a welcome window with recent Wikis, **Open Folder…** and **New Wiki…** (a folder picker and then `init`).
- `wikirs gui --wiki …`, or `wikirs gui` run inside a Wiki, opens that Wiki directly.
- "Open Wiki in New Window" is a menu item.

## TUI

No welcome screen: the TUI resolves like the CLI, or exits with the hint.
