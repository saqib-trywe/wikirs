# Selecting a Wiki

Map: [wikirs map](../map.md)
Type: grilling
Status: resolved
Assignee: Saqib
Blocked by: —

## Question

How does each Interface find the root for `Wiki::open(root)`? The candidates are a `--wiki` flag, a `WIKIRS_WIKI` env var, walking up from the cwd to a `.wikirs/` folder, a default in the user config, and a GUI recent list. In what order do they apply? Can one process serve several Wikis (the GUI with several windows; `serve`/`mcp` with a Wiki per route or per tool argument)? Or is it strictly one Wiki per process? Is there a user-level config (outside any Wiki) for defaults and recent Wikis, and where does it live? What happens when a folder without `.wikirs/` is opened? Context: [On-disk layout](07-on-disk-layout.md) (walk-up, `.wikirs/` created lazily) and [process-model.md](../../../docs/spec/process-model.md) (a per-Wiki cache dir).

## Answer

Resolved 2026-09-26 by grilling. The result is in [docs/spec/wiki-selection.md](../../../docs/spec/wiki-selection.md). No ADR: all of it is cheap to change.

1. **Per process**: exactly one Wiki per process for the CLI, TUI, `mcp` and `serve`; one per window for the GUI. Operations never take a `wiki` argument.
2. **Resolution order**: `--wiki` → `WIKIRS_WIKI` → walk up to `.wikirs/` → `default_wiki` → error `not_found{what: wiki}` with a hint. `init` alone runs without a resolved Wiki (flag/env, else the cwd).
3. **Without `.wikirs/`**: a folder can be opened only explicitly. Walk-up never guesses.
4. **User files**: `<config dir>/wikirs/config.toml` (edited by hand: `default_wiki`, named `[wikis]`) and `<state dir>/wikirs/recent.json` (written by the app).
5. **Named Wikis**: `--wiki notes` looks the name up first. Values with `/`, `~` or `.` are paths.
6. **MCP**: `wikirs mcp` ignores the cwd and MCP `roots`, and uses only the flag, env or default.
7. **GUI**: reopens the last session, or shows a welcome window (recent Wikis / Open Folder / New Wiki), plus "Open Wiki in New Window". The TUI resolves like the CLI.
