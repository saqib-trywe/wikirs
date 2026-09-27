# 5. SQLite FTS5 as a shared multi-process Index

Status: accepted (2026-09-26)

## Context

With one binary and no daemon ([ADR 0001](0001-single-binary-no-daemon.md)), several wikirs processes (GUI, TUI, `serve`, `mcp`, one-shot CLI calls) run against the same Wiki at once, and each needs the Index: Links, Backlinks, Tags and full-text search. The [engine research](../../.scratch/wikirs/research/03-search-index-engine.md) compared three options:

- **tantivy**: the best ranking and stemming, but only one writer process at a time (the others fail at once with `LockBusy`), ~88 ms per commit, and structured queries (Backlinks, Tag subtrees, rename rewrites) have to be written in application code. Without a daemon, we would have to choose which process writes.
- **An in-memory Index per process**: no shared state, but every process, including every CLI call, rebuilds it (~1 s at 10k Pages) and watches files on its own.
- **A hybrid** (SQLite for the structured data, tantivy for search): two stores to keep in sync.

## Decision

One SQLite database with FTS5 per Wiki, stored in the OS cache dir (not in the Wiki), in WAL mode. Links, Tags, file metadata and the full-text table live in one transactional store. Writers from several processes queue behind a busy timeout, and a committed update is visible to every process's next read with no polling.

## Consequences

- Having no daemon costs nothing here: any process can update the Index, and a CLI call opens the existing Index instead of rebuilding.
- Search is weaker than tantivy's: BM25 with the `unicode61` tokenizer and no stemming by default, with English `porter` as an opt-in. Custom tokenizers would need unsafe FFI. Because the Index is disposable, tantivy can later be added for `search` alone without changing the Operation contract.
- WAL needs a local filesystem. The Index lives in the local cache dir, so a Wiki on a network share still works, but only the watcher is affected (it falls back to polling).
- The Index costs ~1.8 MB of binary size and roughly the Wiki's text size on disk.
