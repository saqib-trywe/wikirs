//! The Wiki handle, Page Paths, and finding a Wiki's root (docs/spec/wiki-selection.md).

use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use crate::{Error, Result, index::Index};

/// How long a mutation waits for another process's write lock (ADR 0006).
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Wiki {
    root: PathBuf,
    cache_dir: PathBuf,
    index: Arc<Mutex<Index>>,
}

impl Wiki {
    /// Opens the Wiki at `root`, finishing any Plan a crashed process left in
    /// its journal (process-model.md).
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_cache(root, cache_base())
    }

    /// Like [`Wiki::open`], with per-Wiki cache dirs under `cache_base` instead
    /// of the OS cache dir (tests and embedders).
    pub fn open_with_cache(root: impl AsRef<Path>, cache_base: impl AsRef<Path>) -> Result<Self> {
        let given = root.as_ref();
        let root = given
            .canonicalize()
            .ok()
            .filter(|p| p.is_dir())
            .ok_or_else(|| Error::not_found("wiki", &given.display().to_string()))?;
        let cache_dir = cache_base.as_ref().join(wiki_key(&root));
        let mut index = Index::open(&cache_dir.join("index.db"))?;
        crate::plan::recover_at_open(&root, &cache_dir)?;
        // Every open brings the Index up to date (process-model.md#freshness).
        index.reconcile(&root)?;
        Ok(Self {
            root,
            cache_dir,
            index: Arc::new(Mutex::new(index)),
        })
    }

    /// The shared Index connection for this handle.
    pub fn index(&self) -> MutexGuard<'_, Index> {
        self.index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Per-Wiki cache dir: Index, write lock, journal (process-model.md).
    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Takes the Wiki-wide advisory write lock, waiting up to [`LOCK_TIMEOUT`]
    /// for other wikirs processes. Released when the guard drops.
    pub fn lock_writes(&self) -> Result<WriteLock> {
        lock_writes(&self.root, &self.cache_dir)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `.md` file for a (validated) Page Path.
    #[must_use]
    pub fn page_file(&self, page: &PagePath) -> PathBuf {
        self.root.join(format!("{}.md", page.as_str()))
    }
}

/// Takes the write lock of the Wiki at `root` whose cache dir is `cache_dir`
/// (also used by journal recovery before the `Wiki` handle exists).
pub(crate) fn lock_writes(root: &Path, cache_dir: &Path) -> Result<WriteLock> {
    std::fs::create_dir_all(cache_dir).map_err(|e| Error::io(Some("cache dir"), &e))?;
    let root_note = cache_dir.join("wiki_root.txt");
    if !root_note.exists() {
        let _ = std::fs::write(&root_note, root.display().to_string());
    }
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(cache_dir.join("write.lock"))
        .map_err(|e| Error::io(Some("write.lock"), &e))?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(WriteLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() < LOCK_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::WouldBlock) => return Err(Error::conflict_lock_timeout()),
            Err(std::fs::TryLockError::Error(e)) => return Err(Error::io(Some("write.lock"), &e)),
        }
    }
}

/// Holds the Wiki's write lock; the OS releases it when the file closes.
#[derive(Debug)]
pub struct WriteLock {
    _file: File,
}

/// `<os cache>/wikirs`, or `WIKIRS_CACHE_DIR` (used by tests and CI to keep
/// per-Wiki cache dirs out of the user's cache).
fn cache_base() -> PathBuf {
    std::env::var_os("WIKIRS_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::cache_dir().map(|d| d.join("wikirs")))
        .unwrap_or_else(|| std::env::temp_dir().join("wikirs-cache"))
}

/// 16 hex chars of blake3(canonical root path).
fn wiki_key(root: &Path) -> String {
    blake3::hash(root.to_string_lossy().as_bytes()).to_hex()[..16].to_string()
}

/// Finds the root for CLI-style Interfaces: `--wiki`, then `WIKIRS_WIKI`, then
/// walking up from `cwd` to the nearest folder containing `.wikirs/`.
///
/// Walking skeleton: named Wikis and `default_wiki` from the user config come later.
pub fn resolve_root(
    flag: Option<&str>,
    env: Option<&str>,
    cwd: &Path,
    walk_up: bool,
) -> Result<PathBuf> {
    if let Some(given) = flag.or(env) {
        return Ok(expand_home(given));
    }
    if walk_up && let Some(found) = cwd.ancestors().find(|dir| dir.join(".wikirs").is_dir()) {
        return Ok(found.to_path_buf());
    }
    let mut err = Error::not_found("wiki", &cwd.display().to_string());
    err.message = "no Wiki found: run 'wikirs init' here, pass --wiki, or set default_wiki".into();
    Err(err)
}

fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// A validated Page Path: relative to the Wiki root, no `.md`, no hidden segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagePath(String);

impl PagePath {
    pub fn parse(raw: &str) -> Result<Self> {
        let bad = |reason| Err(Error::invalid_path(raw, reason));
        if raw.is_empty() {
            return bad("empty");
        }
        if raw.contains('\\') || raw.contains('\0') {
            return bad("malformed");
        }
        if std::path::Path::new(raw)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            return bad("page paths have no .md extension");
        }
        for segment in raw.split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return bad("malformed");
            }
            if segment.starts_with('.') {
                return bad("hidden segment");
            }
        }
        Ok(Self(raw.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last segment, used as the Title when there is no H1.
    #[must_use]
    pub fn file_stem(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }
}

/// Rejects a new path whose file or any folder differs from an existing entry
/// only by case (Page identity decision, item 10).
pub fn check_case_conflict(wiki: &Wiki, page: &PagePath) -> Result<()> {
    let segments: Vec<&str> = page.as_str().split('/').collect();
    let mut dir = wiki.root().to_path_buf();
    let mut prefix = String::new();
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        let wanted: Vec<String> = if last {
            vec![format!("{segment}.md"), (*segment).to_string()]
        } else {
            vec![(*segment).to_string()]
        };
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                for want in &wanted {
                    if name != *want && name.to_lowercase() == want.to_lowercase() {
                        let existing = format!("{prefix}{}", name.trim_end_matches(".md"));
                        return Err(Error::case_conflict(page.as_str(), &existing));
                    }
                }
            }
        }
        prefix.push_str(segment);
        prefix.push('/');
        dir = dir.join(segment);
    }
    Ok(())
}

/// Filename slug from a Title: Unicode letters and digits kept and lowercased,
/// everything else becomes `-` (On-disk layout decision, item 8).
#[must_use]
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_matches_the_spec_example() {
        assert_eq!(slugify("Async Notes, Part 2"), "async-notes-part-2");
        assert_eq!(slugify("  Größe & Ärger "), "größe-ärger");
    }

    #[test]
    fn page_paths_reject_malformed_input() {
        for bad in ["", "a//b", "/a", "a/", "../a", "a/.hidden", "a.md", "a\\b"] {
            assert!(PagePath::parse(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(PagePath::parse("eng/rust/async-notes").is_ok());
    }
}
