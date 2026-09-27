# Process model and concurrency

Map: [wikirs map](../map.md)
Type: grilling
Status: resolved
Assignee: Saqib
Blocked by: 03

## Question

Given one binary and no daemon (ADR 0001), how do concurrent processes and external editors coexist? Which filesystem watcher (notify) should be used, with what debounce semantics? How is the Index shared or locked between processes? How are write conflicts detected (mtime/hash check before write)? What guarantees do atomic writes give (temp file + rename)? When is the Index rebuilt vs updated incrementally? What does the GUI or TUI do when a Page it has open changes on disk?

Inputs from [Index engine research](../research/03-search-index-engine.md): SQLite WAL lets writers from several processes queue behind a busy timeout (local filesystem only). tantivy allows one writer process at a time (the second fails with `LockBusy`), so without a daemon we would have to decide which process writes. An in-memory Index means every process rebuilds its own copy (about 1 s for 10k pages).

Inputs from [Operation catalogue](08-operation-catalogue.md) ([spec](../../../docs/spec/operations.md)): writes take an optional `base_version` (a content hash) and fail with `Conflict`; apply works out the Plan again and must detect files that changed since the dry run; a Plan is applied as one unit, so recovering from a crash in the middle of a Plan is decided here; queries bring the Index up to date themselves (there's no refresh Operation); `watch` emits path-level events, including those from the process's own mutations and from other processes.

Input from [Parity architecture](09-parity-architecture.md): the core is synchronous; `watch` returns a plain channel receiver that each adapter bridges (SSE, MCP notifications, the gpui and ratatui event loops); `Wiki::open(root)` holds the Index connection.

## Answer

Resolved 2026-09-26 by grilling. Recorded in [ADR 0005](../../../docs/adr/0005-sqlite-fts5-shared-index.md) (shared SQLite Index) and [ADR 0006](../../../docs/adr/0006-write-lock-and-roll-forward-journal.md) (write lock and journal). Everything else is in [docs/spec/process-model.md](../../../docs/spec/process-model.md).

1. **Index engine**: SQLite + FTS5, one DB per Wiki in the OS cache dir, WAL mode. tantivy can still be added later for `search` only.
2. **Write lock**: one advisory OS file lock per Wiki, held from computing the Plan until the Index update finishes. Queries never take it, and a timeout gives `Conflict`.
3. **Conflicts**: `Version` = xxh3-128 content hash. Under the lock, every touched file is checked against `base_version` and the planned hash, and any mismatch aborts the whole Plan. mtime is never used.
4. **Atomic writes**: a hidden temp file in the same folder, fsync, rename. Permissions are kept.
5. **Crash recovery**: a roll-forward journal. Edits apply in the order creates/modifies → moves → deletes. On recovery, an edit is finished if the file has the "before" hash, skipped if it has the "after" hash, and otherwise reported.
6. **Watcher**: `notify` + `notify-debouncer-full` with a 200 ms debounce, in long-lived processes only. Watcher errors trigger a reconcile scan. `local.toml` can switch to polling, and a failed native watcher falls back to polling.
7. **Freshness**: a reconcile scan at `Wiki::open` (size and mtime, then hash); incremental updates from the watcher that skip files whose hash hasn't changed; mutations update the Index as part of applying. wikirs writes are visible to every process's next read, and external edits within about 200 ms or at the next open.
8. **Rebuild**: only when the Index is missing or corrupt, when the schema or parser version changes, or on `rebuild_index`. The new DB is swapped in with a rename, and a generation counter tells other processes to reopen.
9. **`watch` sources**: a process's own applies emit events directly (with real moves). Watcher updates emit an event only when the Index actually changed, which filters out echoes. External moves arrive as delete + create.
10. **Tokenizer**: `unicode61 remove_diacritics 2` with no stemming. English stemming is opt-in in `config.toml`.
11. **Cache dir**: keyed by a blake3 hash of the canonical root path.
12. **Open Page changed on disk**: reload without prompting if there are no unsaved edits, otherwise a banner (Reload / Keep mine / Compare). Follow app moves. On external delete, mark the buffer as deleted. A save always sends `base_version`.
