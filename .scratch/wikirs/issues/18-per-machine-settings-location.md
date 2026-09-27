# Where per-machine Wiki settings live

Map: [wikirs map](../map.md)
Type: grilling
Status: resolved
Assignee: Saqib
Blocked by: —

## Question

[On-disk layout](07-on-disk-layout.md) puts per-machine overrides in `.wikirs/local.toml` and calls it "not synced". That only holds for git (through `.wikirs/.gitignore`): Dropbox and iCloud sync the whole folder, so `[serve]` settings ([http-security.md](../../../docs/spec/http-security.md#settings)), the Index path override and `watcher` mode would travel between machines. Options:
1. Keep `local.toml` in the Wiki and correct the wording to "not committed to git".
2. Move it out of the Wiki to `<config dir>/wikirs/wikis/<wiki key>.toml`, next to the user config ([wiki-selection.md](../../../docs/spec/wiki-selection.md)), so it's truly per machine whatever the sync tool.

If it moves, what is the `<wiki key>`, given that the cache key hashes the canonical root path, which moving the folder changes? And does `set_config(scope: local)` still make sense as the name?

## Answer

Resolved 2026-09-27 by grilling. The result is in [wiki-selection.md → Machine settings](../../../docs/spec/wiki-selection.md#machine-settings). This amends [On-disk layout](07-on-disk-layout.md).

1. **Location**: machine settings move out of the Wiki to `~/.config/wikirs/wikis/<wiki key>.toml`. Per the user, config lives in `~/.config` (or `$XDG_CONFIG_HOME`) on **every OS**, including macOS. The user config moves there too; state and cache keep the OS conventions. `.wikirs/` now holds only the committed `config.toml`, and `.wikirs/.gitignore` is gone.
2. **Key**: the same canonical-root-path hash as the cache and token. Each file records `root`. When the Wiki folder moves, the app offers to adopt the orphaned file (a GUI prompt, or `wikirs config adopt`). Like `--rotate-token`, that's a setup action, not a catalogue Operation.
3. **Naming**: the config scopes are `wiki` and `machine` (replacing `local`) in `set_config` / `get_config`.
