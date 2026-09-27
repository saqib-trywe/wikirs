# Search and Index engine: tantivy vs SQLite FTS5 vs in-memory — findings

Ticket: [03-search-index-engine](../issues/03-search-index-engine.md). Researched 2026-09-26 against crates.io, docs.rs, sqlite.org, shallow clones of the tantivy and rusqlite `main` branches, and a small local benchmark (details and caveats at the end).

## Candidates (crates.io, 2026-09-26)

| Option | Crate / version | Notes |
|---|---|---|
| tantivy | `tantivy` 0.26.2 (2026-09-08); `main` is 0.27.0-dev | MIT, ~4M recent downloads, maintained by the Quickwit org. <https://crates.io/crates/tantivy> |
| SQLite FTS5 | `rusqlite` 0.40.2 (2026-08-08), feature `bundled` | Bundled SQLite at build time was 3.53.2; the `main` bundle is now 3.53.4. <https://crates.io/crates/rusqlite> |
| In-memory | Hand-written maps, plus a Markdown parser (for example `pulldown-cmark` 0.13.x) | No engine dependency. You write ranking and stemming yourself or pull in a stemmer crate. |

- rusqlite's `bundled` build compiles SQLite with `-DSQLITE_ENABLE_FTS5`, `FTS3` and `JSON1`, so FTS5 needs no extra setup. See [libsqlite3-sys/build.rs](https://github.com/rusqlite/rusqlite/blob/master/libsqlite3-sys/build.rs).

## Multi-process safety (the key constraint: no daemon)

### tantivy
- **One writer per index, across processes.** `IndexWriter` holds `.tantivy-writer.lock`. The doc comment says: "Only one process should be able to write tantivy's index at a time". The lock is an OS file lock (`fs4` `try_lock_exclusive`) and is **non-blocking**. A second `writer()` call fails straight away with `LockBusy`. See `src/directory/directory_lock.rs` and `src/directory/mmap_directory/mod.rs` (`acquire_lock`).
  - I checked this locally. A second `index.writer()` returned: *"Failed to acquire index lock … there is already an `IndexWriter` working on this `Directory`, in this process or in a different process."*
- **Readers in other processes are supported.** `.tantivy-meta.lock` (a blocking lock) "protect[s] the segment files being opened by `IndexReader::reload()` from being garbage collected" and lets "another process … safely consume our index in-writing". Same source file.
- **How other processes see new commits:** `ReloadPolicy::OnCommitWithDelay` reloads "within milliseconds after a new commit … by watching changes in the `meta.json` file" ([docs.rs](https://docs.rs/tantivy/latest/tantivy/enum.ReloadPolicy.html)). In `MmapDirectory` this is a **polling thread with a 500 ms interval** that CRC-checks meta.json (`src/directory/mmap_directory/file_watcher.rs`, `POLLING_INTERVAL`). Cross-process visibility therefore lags by up to about 0.5 s.
- **Consequence for wikirs:** tantivy has no built-in queueing between processes. When a GUI and an MCP server both want to write, one of them gets `LockBusy`. The app has to pick a strategy: retry/back off, write briefly and then drop the writer, or elect one "indexer" process. Holding an `IndexWriter` open for a long time blocks every other process from writing.

### SQLite (WAL mode)
- "Readers do not block writers and a writer does not block readers." However, "there can only be one writer at a time" ([sqlite.org/wal.html](https://sqlite.org/wal.html)).
- Writers queue up rather than fail. A process waiting on the lock gets `SQLITE_BUSY` unless it sets a busy timeout (`busy_timeout` / rusqlite `Connection::busy_timeout`). Short write transactions from several processes then serialise automatically. This is the standard SQLite model. Use `BEGIN IMMEDIATE` for read-then-write transactions to avoid upgrade deadlocks ([sqlite.org/lang_transaction.html](https://sqlite.org/lang_transaction.html)).
- Constraints: "All processes using a database must be on the same host computer; WAL does not work over a network filesystem". WAL uses a shared-memory `-shm` file. Long-running readers can stall checkpoints and make the WAL grow ([wal.html](https://sqlite.org/wal.html)). A wiki folder on a network share or in some synced folders would be a problem. One way round it is to keep the Index DB outside the wiki folder, for example in a per-user cache dir.
- A committed write is visible to the next read transaction in any process. No polling is needed.

### In-memory
- Each process builds and owns its own copy, so no locks are shared. Every process pays the full rebuild cost at startup and holds its own copy in RAM. Each process must also watch the filesystem (or be notified) to learn about edits made by the other processes. Consistency between processes is then only as good as file watching.

## Rebuild and incremental cost (local benchmark)

Setup: a synthetic corpus of 10,000 pages (~3.3 KB each, 33 MB total, ~20k distinct terms), Apple M4 Max, release build. The indexing-only runs index pre-generated strings. Reading and parsing the files is timed separately.

| Step | tantivy 0.26.2 | SQLite 3.53.2 FTS5 (`porter unicode61`, external-content, WAL, `synchronous=NORMAL`) | In-memory HashMap |
|---|---|---|---|
| Read 10k `.md` files and parse with pulldown-cmark | — | — | **~0.42 s** (read + parse + build postings and backlinks together) |
| Build index for 10k docs (one commit / one transaction) | **~0.44 s** (100 MB writer budget, English stemmer, plus a facet field and a link field) | **~0.37 s** (FTS rows plus link and tag tables with B-tree indexes) | included above |
| Update one page (delete + add + commit) | **~88 ms** per commit | **~0.27 ms** per transaction | microseconds (update the maps) |
| Top-10 BM25 query | ~0.07 ms | ~4 ms | n/a (no ranking built) |
| On-disk size | 24 MB (stores body) | 47 MB (includes a full copy of each body in the content table) | none |

What the numbers suggest:
- At 10k pages, a full rebuild for any option comes to **roughly 1 s or less**: file I/O and parsing (~0.4 s) plus engine indexing (~0.4 s). None of the three is ruled out by rebuild time.
- Frequent small updates cost far more in tantivy: each `commit()` writes a new segment and fsyncs, and the docs say "A call to commit blocks" ([IndexWriter docs](https://docs.rs/tantivy/latest/tantivy/indexer/struct.IndexWriter.html)). Callers usually batch or debounce commits. In WAL mode with `synchronous=NORMAL`, SQLite does not fsync on each commit, so part of its advantage comes from that setting. With `synchronous=FULL` it would pay an fsync per commit too.
- tantivy has no in-place update. The pattern is `delete_term(id)` + `add_document`, and the whole document is re-indexed.

## Stemming and ranking

- **tantivy:** BM25 ("the same as Lucene"), phrase queries, a natural query language, and configurable tokenizers. The README says "stemming available for 17 Latin languages". The `Language` enum in `main` lists 18 (Arabic … Turkish, including Greek, Russian and Tamil). The stemmer is the default `stemmer` feature, now backed by `frostem`. There are third-party CJK tokenizers. Sources: [README](https://github.com/quickwit-oss/tantivy/blob/main/README.md), `src/tokenizer/stemmer.rs`, `Cargo.toml`.
- **FTS5:** has a `bm25()` auxiliary function with per-column weights ("numerically smaller" is better). Built-in tokenizers are `unicode61` (default), `ascii`, `porter` and `trigram`. `porter` is "designed for use with English language terms only". `trigram` gives substring/LIKE/GLOB matching. There are also prefix indexes, a `detail=` option, and `rebuild`/`optimize`/`automerge` commands ([sqlite.org/fts5.html](https://sqlite.org/fts5.html)).
  - Non-English stemming needs a custom FTS5 tokenizer. **rusqlite has no safe API for registering FTS5 tokenizers.** It only exposes the raw `fts5_api` pointer via ffi (`src/row.rs`), so custom tokenizers mean `unsafe` FFI code or a separate crate.
- **In-memory:** you write it all yourself: tokenisation, stemming (for example a Snowball stemmer crate), and BM25 scoring. That is manageable, but it is code you own and test.

## Binary size (release, `strip = true`, macOS arm64, measured)

| Binary | Size |
|---|---|
| Hello-world baseline | 0.34 MB |
| + pulldown-cmark + HashMap index | 0.66 MB |
| + rusqlite (bundled SQLite) | 2.1 MB (≈ +1.8 MB) |
| + tantivy (default features) | 5.2 MB (≈ +4.8 MB) |

Default features were used, with no LTO or `opt-level="z"`. Trimming features could shrink tantivy, for example by dropping `zstd`, `lz4` or `stopwords`.

## Structured queries: hierarchical Tags and Backlinks

- **SQLite:** these are ordinary relational queries. Backlinks are `SELECT … FROM link WHERE dst = ?` on an indexed column. Hierarchical Tags work as `path = 'a/b' OR path LIKE 'a/b/%'` (or a range `path >= 'a/b/' AND path < 'a/b0'`) on an indexed `path` column. A closure table or recursive CTE is also possible. Joins, counts, renames/rewrites of link targets, orphan detection and "pages with tag X that link to Y" all fit in one SQL statement. In the benchmark, a tag-subtree count plus a backlink lookup took ~0.4 ms. FTS5 lives in the **same database and transaction**, so full-text and structured data stay consistent with each other.
- **tantivy:** hierarchical Tags map naturally onto `Facet` fields (`/lang/rust/async`). A doc tagged deep in the tree implicitly belongs to its ancestors, and "being a child of a given facet" is a range over term ordinals ([Facet docs](https://docs.rs/tantivy/latest/tantivy/schema/struct.Facet.html)). `FacetCollector` gives per-subtree counts. Backlinks can be a `STRING` field holding each target and queried with `TermQuery`. There are no joins or relational updates, though. Renaming a Page means re-indexing every Page that links to it, and graph-style queries (orphans, link counts by source) mean collecting and post-processing in Rust.
- **In-memory:** `HashMap<Target, Vec<Source>>` and a `BTreeMap` over tag paths (subtree = range scan) are trivial and fastest. They are rebuilt per process.

## Can one engine serve both?

- **SQLite + FTS5: yes, directly.** It serves structured tables and full-text in one file, with multi-process writer queueing built in. The trade-offs are weaker ranking/stemming (English-only porter out of the box, BM25 without tantivy's query language), no safe Rust API for custom tokenizers, a same-host/local-filesystem limit for WAL, and a larger DB if it keeps its own copy of the content (a contentless or external-content table referencing another table can reduce this).
- **tantivy: partially.** Facets and term fields can cover Tag and Backlink lookups. Relational queries (joins, renames, orphan detection) have to be written in application code. Its single-writer, fail-fast lock needs a coordination strategy when there is no daemon.
- **In-memory: yes, per process.** It has no shared state or locks, but each process rebuilds (~0.5–1 s at 10k pages), holds its own copy, needs its own file watching, and you build your own search ranking.
- **Hybrids** are also viable: SQLite for Links/Backlinks/Tags with tantivy for search, or in-memory structure with SQLite/tantivy for search. They add a second store that has to be kept in sync (the Index is disposable, so a mismatch can be fixed by a rebuild).

## Benchmark caveats

- Synthetic, repetitive vocabulary, so it is not representative of real prose for ranking or on-disk size. Timings come from a single run on a fast machine. The code was in `/tmp/idxbench` (throwaway).
- SQLite used `synchronous=NORMAL` and tantivy used default fsync behaviour, so the per-update gap is partly due to durability settings. For a disposable Index, relaxed durability is arguably acceptable, but that is a design decision.
- Cross-process lock contention was not load-tested. The lock behaviour is from source and docs, plus the in-process `LockBusy` check.
