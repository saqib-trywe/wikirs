# Process model and concurrency

How wikirs processes, external editors and the Index coexist. Decided in [Process model and concurrency](../../.scratch/wikirs/issues/10-process-model-concurrency.md). The hard-to-reverse parts are in [ADR 0005](../adr/0005-sqlite-fts5-shared-index.md) (shared SQLite Index) and [ADR 0006](../adr/0006-write-lock-and-roll-forward-journal.md) (write lock and journal).

## Processes

- One binary, no daemon.
- Long-lived processes (GUI, TUI, `serve`, `mcp`) watch the filesystem.
- One-shot CLI calls don't watch.
- The core is synchronous (see [interfaces.md](interfaces.md)).

## Per-Wiki cache dir

`<os cache>/wikirs/<blake3(canonical root path), 16 hex>/`, overridable in the [machine settings](wiki-selection.md#machine-settings). The `WIKIRS_CACHE_DIR` env var replaces `<os cache>/wikirs` for every Wiki, which is how tests and CI keep their cache dirs out of the user's cache:

| File | Purpose |
|---|---|
| `index.db` (+ `-wal`, `-shm`) | the Index |
| `write.lock` | the Wiki-wide write lock |
| `journal.json` | exists only while a Plan is applying, or after a crash |
| `unrecovered.json` | edits journal recovery left alone because their file had changed (read by `index_status` and `check`) |
| `wiki_root.txt` | the root path, for debugging |

Moving the Wiki folder changes the key, so the Index is rebuilt (~1 s). Stale cache dirs aren't garbage-collected. `index_status` reports the path.

## Index (SQLite)

- **Settings:**
  - WAL, `synchronous=NORMAL`, `busy_timeout=5s`
  - `BEGIN IMMEDIATE` for writes
  - one connection per `Wiki` handle
- **Full-text:** page text is stored exactly once, in the FTS5 table (no separate body column), so the Index holds one copy of the Wiki's text and snippets still work. The tokenizer is `unicode61 remove_diacritics 2` with no stemming. `search.stemming = "english"` in `config.toml` switches to `porter`, which triggers a rebuild.
- **What's stored per file:** size, mtime and content hash, plus the Index's `index_schema`, `parser_version` and `generation`.

## Freshness

| Mechanism | When | What |
|---|---|---|
| Reconcile scan | `Wiki::open` (every CLI call), after watcher errors or overflow, after journal recovery | Walk the tree. If (size, mtime) differ from the Index → hash the file → reparse only if the hash differs. Files that have disappeared are removed. An mtime less than 2 s old is stored as *racy*, so the next scan hashes that file again: on filesystems with coarse timestamps (FAT, HFS+), a same-size rewrite in the same tick would otherwise go unseen. |
| Watcher | Long-lived processes | Debounced events → incremental update, skipped when the hash already matches (so several watchers do idempotent, cheap work) |
| Mutation | Every apply | Updates the Index as part of the apply, before returning |

Guarantees:
- A wikirs mutation is visible to every process's next query.
- An external edit is visible within about 200 ms when any long-lived process is running, and otherwise at the next `Wiki::open`.
- A healthy long-lived process never rescans before a query.

## Rebuild

A full rebuild happens only when:
- the Index is missing or fails its integrity check at open
- `index_schema` or `parser_version` differs from the binary's (the newer binary rebuilds)
- `rebuild_index` is called

A rebuild runs inside one `BEGIN IMMEDIATE` transaction: it empties the tables, bumps `generation` and re-indexes. Other processes keep reading the old contents until it commits. (Swapping in a new database file by rename was rejected: with WAL, a renamed-in file can be paired with the old `-wal` file that other processes still have open.)

Corruption is detected when SQLite reports it, not by an integrity check on every open, which would add a full database scan to every CLI call. A schema or parser-version mismatch drops and recreates the tables inside a transaction.

## Watcher

- `notify` + `notify-debouncer-full`, with a 200 ms debounce. `watch`, and the `mcp` process at start, start the watcher. One-shot CLI calls never do.
- Each debounced batch hashes only the files it names, plus every known file under a named folder and every file under a new one. Hidden and ignored paths are dropped.
- A batch with a create or rename also checks that every known file still exists. Some backends report only a move's new path: FSEvents folds a rename of a file it once saw created into a create.
- An overflow or watcher error compares every file.
- The machine settings have `watcher = "native" | "poll"`, default `native`. `poll` uses `notify`'s PollWatcher at a 2 s interval.
- If the native watcher fails to start, the process falls back to poll and reports it in `index_status`.
- Networked filesystems aren't detected automatically.

## Mutations

1. Take the Wiki-wide write lock (the standard library's `File::try_lock`, an OS advisory lock, polled for up to 5 s). If it can't be taken within the timeout → `Conflict`. Queries never take it.
2. If `journal.json` exists, recover it first (see below).
3. Compute the Plan, recording the hash of every file it reads.
4. If `dry_run`, release the lock and return the Plan.
5. Re-read every touched file and compare it against the caller's `base_version` and the planned hash. Any mismatch → `Conflict`, and nothing is written.
6. Write `journal.json`: every edit with its hash before and after.
7. Apply in order: creates/modifies → moves → deletes. Each file write is a temp file in the same folder (`.<name>.wikirs-tmp`), fsynced, then renamed over the target, keeping the original's permissions. No directory fsync.
8. Update the Index, emit `watch` events, delete the journal, and release the lock.

**`Version`** is a content hash (xxh3-128, opaque to callers). mtime is never used to detect conflicts.

**Journal recovery** runs when a mutation takes the lock, and at `Wiki::open` only if a journal exists (so ordinary opens never take the lock). For each edit:
- hash = before → apply it
- hash = after → skip it
- otherwise → leave the file untouched and report the edit as a warning in `index_status` and `check`

## `watch` events

- **Own mutations:** emitted right after the Index commit, including real `page_moved` events (the Plan knows about moves).
- **Everything else** (other wikirs processes, external editors): emitted by the watcher → Index update path, only when a file differs from the version **this process** last saw. It isn't compared with the shared Index: another process's mutation has already updated the Index by the time our watcher sees its files, and its changes must still reach our subscribers. A mutation records its files as seen under the same lock the watcher compares under, which filters out the echo of the process's own writes.
- **External moves** arrive as delete + create and aren't paired.
- **Batching:** `index_updated` is emitted once per batch.

## Open Page changed on disk (GUI and TUI)

| Situation | Behaviour |
|---|---|
| No unsaved edits | Reload without prompting, keeping the scroll position |
| Unsaved edits | Keep the buffer and show a "changed on disk" banner: Reload / Keep mine (overwrite) / Compare |
| Moved by a wikirs Plan (`page_moved`) | Follow it to the new path |
| Deleted, or moved outside wikirs | Mark the buffer as deleted. Saving re-creates the file |

A save always sends `base_version`, so a stale save gets `Conflict` and never silently overwrites the file.
