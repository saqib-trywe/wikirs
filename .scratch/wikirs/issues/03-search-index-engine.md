# Search and Index engine: tantivy vs SQLite FTS5 vs in-memory

Map: [wikirs map](../map.md)
Type: research
Status: resolved
Blocked by: —

## Question

What should the Index (Links, Backlinks, Tags, full-text search) be built on? Compare tantivy, SQLite with FTS5 (rusqlite), and a pure in-memory index rebuilt at startup, looking at: time to rebuild from about 10k pages, incremental update cost, safety when several processes share it (readers and writers across processes, locking), stemming and ranking quality, binary size, and suitability for hierarchical-Tag and Backlink queries (not just full-text). Can one engine serve both the structured queries and search?

## Answer

- All three options can rebuild 10k pages in about 1 s or less. File reading and parsing take ~0.4 s, and engine indexing takes ~0.4 s (tantivy 0.26.2 and SQLite 3.53 FTS5 were measured at about the same speed).
- Per-page updates: SQLite takes ~0.3 ms per transaction (WAL, `synchronous=NORMAL`). tantivy takes ~88 ms per `commit()` because each commit writes a new segment and fsyncs, so commits need batching or debouncing.
- Multi-process: SQLite WAL lets many readers run alongside one writer, and other writers wait in turn via busy_timeout. It needs the same host and a local filesystem. tantivy allows one `IndexWriter` across all processes: a second writer fails straight away with `LockBusy`, and readers in other processes see commits by polling `meta.json` every 500 ms. In-memory shares nothing: each process rebuilds, holds its own copy and watches the files.
- Ranking and stemming: tantivy has Lucene-style BM25, a query language, and stemmers for 17–18 languages. FTS5 has `bm25()`, the English-only `porter` tokenizer and `trigram`. Custom FTS5 tokenizers need unsafe FFI, because rusqlite has no safe API for them. In-memory means writing your own.
- Binary size (stripped): bundled SQLite adds ~1.8 MB and tantivy adds ~4.8 MB with default features.
- Structured queries: SQLite handles Backlinks, hierarchical Tags (prefix or range on an indexed path), joins and renames in SQL, in the same transaction as FTS. tantivy covers Tags through `Facet` and Backlinks through term fields, but has no joins, so graph and rename logic goes in application code.
- One engine for both: SQLite+FTS5 can do both natively. tantivy can do both only partially. In-memory can do both, but separately in each process. Hybrids (SQLite for structure plus tantivy for search) mean running two stores and keeping them in sync.
- [findings](../research/03-search-index-engine.md)
