//! The Index: a disposable SQLite + FTS5 cache of the Wiki's files (ADR 0005,
//! process-model.md). One database per Wiki in its cache dir, shared by every
//! wikirs process through WAL.
//!
//! This slice indexes files, Titles and full text. Links and Tags arrive with
//! the markdown parser.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use schemars::JsonSchema;
use serde::Serialize;

use crate::{Error, Result, plan::version_of};

/// Bump when the tables change; a mismatch rebuilds the Index at open.
const INDEX_SCHEMA: i64 = 1;
/// Bump when what's extracted from a file changes (Title rules, parser version).
const PARSER_VERSION: i64 = 1;

pub struct Index {
    conn: Connection,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Index")
    }
}

/// A file found by a scan, before it's read.
#[derive(Clone, Copy)]
struct Scanned {
    kind: &'static str,
    size: i64,
    mtime: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Skipped {
    pub path: String,
    /// `symlink`, `non_utf8` or `case_clash`.
    pub reason: String,
}

fn db_err(e: &rusqlite::Error) -> Error {
    Error::internal(format!("index: {e}"))
}

impl Index {
    /// Opens (creating if needed) the Index at `path`, rebuilding it if its
    /// schema or parser version differs from this binary's.
    pub fn open(path: &Path) -> Result<Self> {
        let dir = path
            .parent()
            .ok_or_else(|| Error::internal("the Index path has no parent dir"))?;
        fs::create_dir_all(dir).map_err(|e| Error::io(Some("cache dir"), &e))?;
        let conn = Connection::open(path).map_err(|e| db_err(&e))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| db_err(&e))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| db_err(&e))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| db_err(&e))?;
        let mut index = Self { conn };
        index.ensure_schema()?;
        Ok(index)
    }

    fn ensure_schema(&mut self) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| db_err(&e))?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL)",
        )
        .map_err(|e| db_err(&e))?;
        let get = |key: &str| -> Result<Option<i64>> {
            tx.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
                .optional()
                .map_err(|e| db_err(&e))
        };
        let current = (get("index_schema")?, get("parser_version")?);
        if current != (Some(INDEX_SCHEMA), Some(PARSER_VERSION)) {
            tx.execute_batch(
                "DROP TABLE IF EXISTS files;
                 DROP TABLE IF EXISTS pages_fts;
                 DROP TABLE IF EXISTS skipped;
                 CREATE TABLE files (
                     path  TEXT PRIMARY KEY,
                     kind  TEXT NOT NULL,      -- 'page' | 'attachment'
                     size  INTEGER NOT NULL,
                     mtime INTEGER NOT NULL,   -- nanoseconds since the epoch
                     hash  TEXT NOT NULL,
                     title TEXT
                 );
                 -- Page text is stored once, here: no separate body column elsewhere.
                 CREATE VIRTUAL TABLE pages_fts USING fts5(
                     path UNINDEXED, title, body,
                     tokenize = 'unicode61 remove_diacritics 2'
                 );
                 CREATE TABLE skipped (path TEXT PRIMARY KEY, reason TEXT NOT NULL);",
            )
            .map_err(|e| db_err(&e))?;
            for (key, value) in [
                ("index_schema", INDEX_SCHEMA),
                ("parser_version", PARSER_VERSION),
                ("generation", 0),
            ] {
                tx.execute(
                    "INSERT INTO meta (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )
                .map_err(|e| db_err(&e))?;
            }
        }
        tx.commit().map_err(|e| db_err(&e))
    }

    /// Brings the Index up to date with the files under `root`: a stat scan,
    /// hashing only files whose size or mtime changed, reparsing only files
    /// whose hash changed. Writes nothing when nothing changed.
    pub fn reconcile(&mut self, root: &Path) -> Result<()> {
        let (scanned, skipped) = scan(root);
        let known: HashMap<String, (i64, i64, String, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT path, size, mtime, hash, kind FROM files")
                .map_err(|e| db_err(&e))?;
            stmt.query_map([], |r| {
                Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            })
            .map_err(|e| db_err(&e))?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| db_err(&e))?
        };
        let old_skipped: HashSet<(String, String)> = self
            .skipped()?
            .into_iter()
            .map(|s| (s.path, s.reason))
            .collect();

        let changed: Vec<&String> = scanned
            .iter()
            .filter(|(path, f)| {
                known
                    .get(*path)
                    .is_none_or(|(size, mtime, _, _)| (*size, *mtime) != (f.size, f.mtime))
            })
            .map(|(path, _)| path)
            .collect();
        let removed: Vec<&String> = known.keys().filter(|p| !scanned.contains_key(*p)).collect();
        // Unchanged non-UTF-8 Pages stay skipped without being re-read.
        let still_unreadable = known.iter().filter(|(path, (.., kind))| {
            kind == "unreadable" && scanned.contains_key(*path) && !changed.contains(path)
        });
        let skipped_now: HashSet<(String, String)> = skipped
            .iter()
            .map(|s| (s.path.clone(), s.reason.clone()))
            .chain(still_unreadable.map(|(p, _)| (p.clone(), "non_utf8".to_string())))
            .collect();
        if changed.is_empty() && removed.is_empty() && skipped_now == old_skipped {
            return Ok(());
        }

        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| db_err(&e))?;
        for path in changed {
            let file = &scanned[path];
            let Ok(bytes) = fs::read(root.join(path)) else {
                continue; // vanished mid-scan; the next scan settles it
            };
            let hash = version_of(&bytes);
            if known.get(path).is_some_and(|(_, _, h, _)| *h == hash) {
                tx.execute(
                    "UPDATE files SET size = ?2, mtime = ?3 WHERE path = ?1",
                    params![path, file.size, file.mtime],
                )
                .map_err(|e| db_err(&e))?;
                continue;
            }
            if file.kind == "page" && std::str::from_utf8(&bytes).is_err() {
                let file = Scanned {
                    kind: "unreadable",
                    ..*file
                };
                upsert(&tx, path, &file, &hash, &bytes)?;
                continue;
            }
            upsert(&tx, path, file, &hash, &bytes)?;
        }
        for path in removed {
            remove(&tx, path)?;
        }
        tx.execute("DELETE FROM skipped", [])
            .map_err(|e| db_err(&e))?;
        for s in &skipped {
            tx.execute(
                "INSERT OR REPLACE INTO skipped (path, reason) VALUES (?1, ?2)",
                params![s.path, s.reason],
            )
            .map_err(|e| db_err(&e))?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO skipped (path, reason)
             SELECT path, 'non_utf8' FROM files WHERE kind = 'unreadable'",
            [],
        )
        .map_err(|e| db_err(&e))?;
        touch(&tx)?;
        tx.commit().map_err(|e| db_err(&e))
    }

    /// Re-indexes specific files (relative paths) after a mutation wrote them.
    pub fn refresh(&mut self, root: &Path, paths: &[&str]) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| db_err(&e))?;
        for path in paths {
            let full = root.join(path);
            match (fs::symlink_metadata(&full), fs::read(&full)) {
                (Ok(meta), Ok(bytes)) if meta.is_file() => {
                    let file = Scanned {
                        kind: kind_of(path),
                        size: size_of(&meta),
                        mtime: mtime_of(&meta),
                    };
                    upsert(&tx, path, &file, &version_of(&bytes), &bytes)?;
                }
                _ => remove(&tx, path)?,
            }
        }
        touch(&tx)?;
        tx.commit().map_err(|e| db_err(&e))
    }

    /// Drops everything and re-indexes `root`, in one transaction: other
    /// processes keep reading the old contents until it commits.
    pub fn rebuild(&mut self, root: &Path) -> Result<()> {
        {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| db_err(&e))?;
            tx.execute_batch("DELETE FROM files; DELETE FROM pages_fts; DELETE FROM skipped;")
                .map_err(|e| db_err(&e))?;
            tx.execute(
                "UPDATE meta SET value = value + 1 WHERE key = 'generation'",
                [],
            )
            .map_err(|e| db_err(&e))?;
            tx.commit().map_err(|e| db_err(&e))?;
        }
        self.reconcile(root)
    }

    /// A canonical dump of everything derived from the files, for comparing
    /// an incrementally maintained Index with a rebuilt one (testing.md).
    #[doc(hidden)]
    pub fn dump(&self) -> Result<Vec<String>> {
        let mut rows = Vec::new();
        for sql in [
            "SELECT 'file', path, kind, hash, coalesce(title, '') FROM files ORDER BY path",
            "SELECT 'fts', path, title, body, '' FROM pages_fts ORDER BY path",
            "SELECT 'skipped', path, reason, '', '' FROM skipped ORDER BY path",
        ] {
            let mut stmt = self.conn.prepare(sql).map_err(|e| db_err(&e))?;
            let found = stmt
                .query_map([], |r| {
                    Ok((0..5)
                        .map(|i| r.get::<_, String>(i))
                        .collect::<rusqlite::Result<Vec<_>>>()?
                        .join("|"))
                })
                .map_err(|e| db_err(&e))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err(&e))?;
            rows.extend(found);
        }
        Ok(rows)
    }

    pub fn skipped(&self) -> Result<Vec<Skipped>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, reason FROM skipped ORDER BY path")
            .map_err(|e| db_err(&e))?;
        stmt.query_map([], |r| {
            Ok(Skipped {
                path: r.get(0)?,
                reason: r.get(1)?,
            })
        })
        .map_err(|e| db_err(&e))?
        .collect::<rusqlite::Result<_>>()
        .map_err(|e| db_err(&e))
    }

    pub fn count(&self, kind: &str) -> Result<i64> {
        self.conn
            .query_row("SELECT count(*) FROM files WHERE kind = ?1", [kind], |r| {
                r.get(0)
            })
            .map_err(|e| db_err(&e))
    }

    pub fn last_updated(&self) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_updated'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| db_err(&e))
    }

    /// Pages under an optional path prefix, sorted and paged.
    pub fn pages(
        &self,
        filter: &Filter,
        sort: Sort,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PageRow>, i64)> {
        let order = match sort {
            Sort::Path => "path",
            Sort::Title => "title COLLATE NOCASE, path",
            Sort::Modified => "mtime DESC, path",
        };
        let (cond, args) = filter.sql();
        let total = self
            .conn
            .query_row(
                &format!("SELECT count(*) FROM files WHERE kind = 'page' AND {cond}"),
                rusqlite::params_from_iter(&args),
                |r| r.get(0),
            )
            .map_err(|e| db_err(&e))?;
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT path, title, mtime FROM files WHERE kind = 'page' AND {cond}
                 ORDER BY {order} LIMIT {limit} OFFSET {offset}"
            ))
            .map_err(|e| db_err(&e))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(&args), |r| {
                Ok(PageRow {
                    path: page_path(&r.get::<_, String>(0)?),
                    title: r.get(1)?,
                    modified: r.get::<_, i64>(2)? / 1_000_000,
                })
            })
            .map_err(|e| db_err(&e))?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| db_err(&e))?;
        Ok((rows, total))
    }

    /// Full-text search. `query` is already an FTS5 expression built by the
    /// caller from plain terms and phrases.
    pub fn search(
        &self,
        query: &str,
        filter: &Filter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<Hit>, i64)> {
        let (cond, filter_args) = filter.sql();
        let mut args = vec![query.to_string()];
        args.extend(filter_args);
        let total = self
            .conn
            .query_row(
                &format!("SELECT count(*) FROM pages_fts WHERE pages_fts MATCH ? AND {cond}"),
                rusqlite::params_from_iter(&args),
                |r| r.get(0),
            )
            .map_err(|e| db_err(&e))?;
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT path, title, snippet(pages_fts, 2, '**', '**', '…', 12), bm25(pages_fts)
                 FROM pages_fts WHERE pages_fts MATCH ? AND {cond}
                 ORDER BY bm25(pages_fts), path LIMIT {limit} OFFSET {offset}"
            ))
            .map_err(|e| db_err(&e))?;
        let hits = stmt
            .query_map(rusqlite::params_from_iter(&args), |r| {
                Ok(Hit {
                    path: page_path(&r.get::<_, String>(0)?),
                    title: r.get(1)?,
                    snippet: r.get(2)?,
                    // bm25 is lower-is-better; flip it so higher scores rank first.
                    score: -r.get::<_, f64>(3)?,
                })
            })
            .map_err(|e| db_err(&e))?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| db_err(&e))?;
        Ok((hits, total))
    }
}

#[derive(Debug, Clone, Copy, Default, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum Sort {
    #[default]
    Path,
    Title,
    Modified,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PageRow {
    pub path: String,
    pub title: String,
    /// Last modified, milliseconds since the Unix epoch.
    pub modified: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Hit {
    pub path: String,
    pub title: String,
    /// Matching text with matches wrapped in `**`.
    pub snippet: String,
    /// Relevance; higher is better.
    pub score: f64,
}

/// `eng/rust.md` → `eng/rust`.
fn page_path(file: &str) -> String {
    file.strip_suffix(".md").unwrap_or(file).to_string()
}

/// Which Pages a query covers (operations.md `Filter`). Tags arrive with the
/// markdown parser.
#[derive(Debug, Clone, Default, serde::Deserialize, JsonSchema)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct Filter {
    /// Only this Space: its home Page and everything under it.
    #[cfg_attr(feature = "clap", arg(long))]
    pub space: Option<String>,
    /// Only Pages whose Page Path starts with this.
    #[cfg_attr(feature = "clap", arg(long))]
    pub path_prefix: Option<String>,
}

impl Filter {
    /// A SQL condition on `path` (the file path, with `.md`) and its arguments.
    fn sql(&self) -> (String, Vec<String>) {
        let mut conds = vec!["1".to_string()];
        let mut args = Vec::new();
        if let Some(space) = &self.space {
            conds.push("(path = ? OR path LIKE ? ESCAPE '\\')".into());
            args.push(format!("{space}.md"));
            args.push(format!("{}/%", escape_like(space)));
        }
        if let Some(prefix) = &self.path_prefix {
            conds.push("path LIKE ? ESCAPE '\\'".into());
            args.push(format!("{}%", escape_like(prefix)));
        }
        (conds.join(" AND "), args)
    }
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn upsert(
    tx: &rusqlite::Transaction,
    path: &str,
    file: &Scanned,
    hash: &str,
    bytes: &[u8],
) -> Result<()> {
    let text = (file.kind == "page").then(|| String::from_utf8_lossy(bytes).into_owned());
    let title = text.as_deref().map(|t| {
        crate::ops::title_of(t).unwrap_or_else(|| {
            page_path(path)
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string()
        })
    });
    tx.execute(
        "INSERT INTO files (path, kind, size, mtime, hash, title) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(path) DO UPDATE SET kind = ?2, size = ?3, mtime = ?4, hash = ?5, title = ?6",
        params![path, file.kind, file.size, file.mtime, hash, title],
    )
    .map_err(|e| db_err(&e))?;
    tx.execute("DELETE FROM pages_fts WHERE path = ?1", [path])
        .map_err(|e| db_err(&e))?;
    if let (Some(text), Some(title)) = (text, title) {
        tx.execute(
            "INSERT INTO pages_fts (path, title, body) VALUES (?1, ?2, ?3)",
            params![path, title, text],
        )
        .map_err(|e| db_err(&e))?;
    }
    Ok(())
}

fn remove(tx: &rusqlite::Transaction, path: &str) -> Result<()> {
    tx.execute("DELETE FROM files WHERE path = ?1", [path])
        .map_err(|e| db_err(&e))?;
    tx.execute("DELETE FROM pages_fts WHERE path = ?1", [path])
        .map_err(|e| db_err(&e))?;
    Ok(())
}

fn touch(tx: &rusqlite::Transaction) -> Result<()> {
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX);
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('last_updated', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [now],
    )
    .map_err(|e| db_err(&e))?;
    Ok(())
}

fn kind_of(path: &str) -> &'static str {
    if Path::new(path).extension().is_some_and(|e| e == "md") {
        "page"
    } else {
        "attachment"
    }
}

fn size_of(meta: &fs::Metadata) -> i64 {
    i64::try_from(meta.len()).unwrap_or(i64::MAX)
}

fn mtime_of(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_nanos()).ok())
        .unwrap_or(0)
}

/// Walks the Wiki (not following symlinks, skipping hidden entries) and
/// returns every file by relative path, plus what was skipped and why.
fn scan(root: &Path) -> (HashMap<String, Scanned>, Vec<Skipped>) {
    let mut files = HashMap::new();
    let mut skipped = Vec::new();
    let mut dirs = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = dirs.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut seen_lower = HashSet::new();
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let raw = entry.file_name();
            let Some(name) = raw.to_str() else {
                skipped.push(Skipped {
                    path: format!("{prefix}{}", raw.to_string_lossy()),
                    reason: "non_utf8".into(),
                });
                continue;
            };
            if name.starts_with('.') {
                continue; // hidden: .git, .wikirs, temp files…
            }
            let rel = format!("{prefix}{name}");
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                skipped.push(Skipped {
                    path: rel,
                    reason: "symlink".into(),
                });
                continue;
            }
            if !seen_lower.insert(name.to_lowercase()) {
                skipped.push(Skipped {
                    path: rel,
                    reason: "case_clash".into(),
                });
                continue;
            }
            if meta.is_dir() {
                dirs.push((entry.path(), format!("{rel}/")));
            } else if meta.is_file() {
                files.insert(
                    rel.clone(),
                    Scanned {
                        kind: kind_of(&rel),
                        size: size_of(&meta),
                        mtime: mtime_of(&meta),
                    },
                );
            }
        }
    }
    (files, skipped)
}
