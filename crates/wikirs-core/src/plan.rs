//! Plans and the mutation engine (ADR 0006, process-model.md#mutations).
//!
//! Every mutation runs through [`mutate`]: take the Wiki-wide write lock,
//! finish any crashed Plan, compute the Plan while recording the hash of every
//! file read, stop there on a dry run, re-check those hashes, journal the Plan,
//! apply it (creates/modifies before moves before deletes), drop the journal.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, Wiki, error::ChangedFile};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Plan {
    pub edits: Vec<Edit>,
    pub warnings: Vec<Warning>,
}

/// One file edit. Paths are relative to the Wiki root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Edit {
    /// Create a new file.
    Create { path: String, content: String },
    /// Create a new file with binary content (an Attachment). The bytes are
    /// staged in the cache dir, not carried in the Plan.
    CreateBinary {
        path: String,
        size: u64,
        /// Hash of the content, as a `Version`.
        version: String,
    },
    /// Splice an existing file. Every byte outside the splices is unchanged.
    Modify {
        path: String,
        /// Hash of the file this edit was planned against.
        base_version: String,
        /// Non-overlapping byte ranges of the original file, in order.
        splices: Vec<Splice>,
    },
    /// Move a file, unchanged, to a path that doesn't exist yet.
    Move { from: String, to: String },
    /// Delete a file.
    Delete { path: String },
}

impl Edit {
    /// Apply order within a Plan: creates and modifies, then moves, then
    /// deletes, so a crash never loses content.
    fn rank(&self) -> u8 {
        match self {
            Edit::Create { .. } | Edit::CreateBinary { .. } | Edit::Modify { .. } => 0,
            Edit::Move { .. } => 1,
            Edit::Delete { .. } => 2,
        }
    }

    /// Every path this edit touches.
    fn paths(&self) -> Vec<&str> {
        match self {
            Edit::Create { path, .. }
            | Edit::CreateBinary { path, .. }
            | Edit::Modify { path, .. }
            | Edit::Delete { path } => vec![path],
            Edit::Move { from, to } => vec![from, to],
        }
    }
}

/// Where a `create_binary` edit's bytes come from while planning.
#[derive(Debug)]
pub enum Blob {
    Bytes(Vec<u8>),
    /// A file outside the Wiki, copied (never read whole into memory).
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Splice {
    /// Byte range `[start, end)` in the original file.
    pub range: [usize; 2],
    pub old: String,
    pub new: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Warning {
    pub kind: String,
    pub message: String,
}

/// Content hash used as a file's `Version` (opaque to callers).
#[must_use]
pub fn version_of(bytes: &[u8]) -> String {
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(bytes))
}

/// [`version_of`] a file's content, streamed; and its size.
pub fn version_of_file(path: &Path) -> std::io::Result<(String, u64)> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    let mut buf = vec![0; 64 * 1024];
    let mut size = 0;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok((format!("{:032x}", hasher.digest128()), size));
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
}

// ------------------------------------------------------------ transaction

/// What a mutation sees while planning: reads are recorded so apply can
/// detect files that changed underneath it.
pub struct Tx<'w> {
    wiki: &'w Wiki,
    reads: BTreeMap<String, Option<String>>,
    edits: Vec<Edit>,
    warnings: Vec<Warning>,
    /// Content of `create_binary` edits, by version.
    blobs: BTreeMap<String, Blob>,
}

/// A file as read during planning.
pub struct ReadFile {
    pub content: String,
    pub version: String,
}

impl Tx<'_> {
    /// Reads a file (relative path) and records its hash; `None` if absent.
    pub fn read(&mut self, rel: &str) -> Result<Option<ReadFile>> {
        let file = match fs::read(self.wiki.root().join(rel)) {
            Ok(bytes) => {
                let version = version_of(&bytes);
                let content = String::from_utf8(bytes)
                    .map_err(|_| Error::invalid_input(None, format!("`{rel}` is not UTF-8")))?;
                Some(ReadFile { content, version })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(Error::io(Some(rel), &e)),
        };
        self.reads
            .insert(rel.to_string(), file.as_ref().map(|f| f.version.clone()));
        Ok(file)
    }

    /// Records a file's current hash without decoding it (moves and deletes
    /// carry Attachments too); `false` if it doesn't exist.
    pub fn stat(&mut self, rel: &str) -> Result<bool> {
        let version = match version_of_file(&self.wiki.root().join(rel)) {
            Ok((version, _)) => Some(version),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(Error::io(Some(rel), &e)),
        };
        let exists = version.is_some();
        self.reads.insert(rel.to_string(), version);
        Ok(exists)
    }

    pub fn edit(&mut self, edit: Edit) {
        self.edits.push(edit);
    }

    /// Plans a `create_binary` edit of `blob` at `rel`; returns its size and version.
    pub fn create_binary(&mut self, rel: &str, blob: Blob) -> Result<(u64, String)> {
        let (version, size) = match &blob {
            Blob::Bytes(bytes) => (version_of(bytes), bytes.len() as u64),
            Blob::File(path) => version_of_file(path)
                .map_err(|e| Error::io(Some(&path.display().to_string()), &e))?,
        };
        self.edits.push(Edit::CreateBinary {
            path: rel.to_string(),
            size,
            version: version.clone(),
        });
        self.blobs.insert(version.clone(), blob);
        Ok((size, version))
    }

    pub fn warn(&mut self, kind: &str, message: impl Into<String>) {
        self.warnings.push(Warning {
            kind: kind.to_string(),
            message: message.into(),
        });
    }

    #[must_use]
    pub fn wiki(&self) -> &Wiki {
        self.wiki
    }
}

/// Runs one mutation: `plan` builds the Plan through a [`Tx`]; unless
/// `dry_run`, the Plan is then applied as one unit.
pub fn mutate<T>(
    wiki: &Wiki,
    dry_run: bool,
    plan: impl FnOnce(&mut Tx) -> Result<T>,
) -> Result<(Plan, T)> {
    let _lock = wiki.lock_writes()?;
    recover(wiki.root(), wiki.cache_dir())?;

    let mut tx = Tx {
        wiki,
        reads: BTreeMap::new(),
        edits: Vec::new(),
        warnings: Vec::new(),
        blobs: BTreeMap::new(),
    };
    let value = plan(&mut tx)?;
    let Tx {
        reads,
        mut edits,
        warnings,
        blobs,
        ..
    } = tx;
    edits.sort_by_key(Edit::rank);
    let plan = Plan { edits, warnings };
    if dry_run || plan.edits.is_empty() {
        return Ok((plan, value));
    }

    // Anything read while planning must still be exactly as read.
    let changed: Vec<ChangedFile> = reads
        .into_iter()
        .filter_map(|(path, expected)| {
            let actual = version_at(wiki.root(), &path);
            (actual != expected).then_some(ChangedFile {
                path,
                expected,
                actual,
            })
        })
        .collect();
    if !changed.is_empty() {
        return Err(Error::conflict_changed(&changed));
    }
    stage(wiki.cache_dir(), blobs)?;

    let journal = Journal::new(wiki.root(), &plan)?;
    journal.write(wiki)?;
    for (applied, entry) in journal.entries.iter().enumerate() {
        crash_hook(applied);
        entry
            .apply(wiki.root(), wiki.cache_dir())
            .map_err(|e| Error::io(Some(entry.edit.paths()[0]), &e))?;
    }
    remove_empty_dirs(wiki.root(), &journal);
    Journal::remove(wiki.cache_dir())?;
    unstage(wiki.cache_dir());
    // The Index is a cache: if this fails, the next reconcile repairs it.
    let touched: BTreeSet<&str> = journal
        .entries
        .iter()
        .flat_map(|e| e.edit.paths())
        .collect();
    let ignore = wiki.settings().ignore();
    let _ = wiki.index().refresh(
        wiki.root(),
        &touched.into_iter().collect::<Vec<_>>(),
        &ignore,
    );
    Ok((plan, value))
}

fn version_at(root: &Path, rel: &str) -> Option<String> {
    version_of_file(&root.join(rel)).ok().map(|(v, _)| v)
}

/// Where staged `create_binary` content waits, by version, until its Plan is done.
fn staged(cache_dir: &Path, version: &str) -> PathBuf {
    cache_dir.join("staged").join(version)
}

/// Writes every blob to the staging dir before the journal, so a crashed
/// Plan can still finish its creates. A source file that changed since
/// planning is a `Conflict`.
fn stage(cache_dir: &Path, blobs: BTreeMap<String, Blob>) -> Result<()> {
    for (version, blob) in blobs {
        let target = staged(cache_dir, &version);
        let io = |e: &std::io::Error| Error::io(Some("staged content"), e);
        match blob {
            Blob::Bytes(bytes) => atomic_write(&target, &bytes).map_err(|e| io(&e))?,
            Blob::File(source) => {
                atomic_copy(&source, &target).map_err(|e| io(&e))?;
                let (actual, _) = version_of_file(&target).map_err(|e| io(&e))?;
                if actual != version {
                    let _ = fs::remove_file(&target);
                    return Err(Error::conflict_changed(&[ChangedFile {
                        path: source.display().to_string(),
                        expected: Some(version),
                        actual: Some(actual),
                    }]));
                }
            }
        }
    }
    Ok(())
}

/// Drops staged content once no journal needs it (the caller holds the lock).
fn unstage(cache_dir: &Path) {
    let _ = fs::remove_dir_all(cache_dir.join("staged"));
}

#[cfg(feature = "test-hooks")]
fn crash_hook(applied: usize) {
    if std::env::var("WIKIRS_TEST_CRASH_AFTER_EDITS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        == Some(applied)
    {
        std::process::abort();
    }
}

#[cfg(not(feature = "test-hooks"))]
fn crash_hook(_applied: usize) {}

/// Applies non-overlapping, ordered splices to `original`.
#[must_use]
pub fn splice(original: &str, splices: &[Splice]) -> String {
    let mut out = String::with_capacity(original.len());
    let mut at = 0;
    for s in splices {
        out.push_str(&original[at..s.range[0]]);
        out.push_str(&s.new);
        at = s.range[1];
    }
    out.push_str(&original[at..]);
    out
}

/// Temp file in the same folder, fsync, rename over the target (process-model.md).
fn atomic_write(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = temp_beside(target)?;
    let mut file = fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, target)
}

/// [`atomic_write`] with the content of `source`, copied without loading it whole.
fn atomic_copy(source: &Path, target: &Path) -> std::io::Result<()> {
    let tmp = temp_beside(target)?;
    fs::copy(source, &tmp)?;
    // Flushing needs write access on Windows.
    fs::OpenOptions::new().write(true).open(&tmp)?.sync_all()?;
    fs::rename(&tmp, target)
}

/// `.<name>.wikirs-tmp` next to `target`, creating its folder.
fn temp_beside(target: &Path) -> std::io::Result<PathBuf> {
    let dir = target
        .parent()
        .ok_or_else(|| std::io::Error::other("target has no parent dir"))?;
    fs::create_dir_all(dir)?;
    let name = target
        .file_name()
        .ok_or_else(|| std::io::Error::other("target has no file name"))?
        .to_string_lossy();
    Ok(dir.join(format!(".{name}.wikirs-tmp")))
}

/// Removes folders that moves and deletes left empty, walking up to (not
/// including) the Wiki root. A deleted Page's folder with children stays: it's
/// a Placeholder.
fn remove_empty_dirs(root: &Path, journal: &Journal) {
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for entry in &journal.entries {
        if let Edit::Move { from: path, .. } | Edit::Delete { path } = &entry.edit {
            let mut dir = Path::new(path).parent();
            while let Some(d) = dir.filter(|d| !d.as_os_str().is_empty()) {
                dirs.insert(d.to_path_buf());
                dir = d.parent();
            }
        }
    }
    // Deepest first, so a parent is only tried once its children are gone.
    let mut dirs: Vec<_> = dirs.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for dir in dirs {
        let _ = fs::remove_dir(root.join(dir)); // fails harmlessly unless empty
    }
}

// ---------------------------------------------------------------- journal

/// A Plan being applied: each edit with the state its file(s) must be in
/// before and after it, so recovery can tell done from pending from diverged.
#[derive(Debug, Serialize, Deserialize)]
struct Journal {
    entries: Vec<JournalEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalEntry {
    edit: Edit,
    /// Hash of the file before the edit (at `from` for moves); None = absent.
    before: Option<String>,
    /// Hash of the file after the edit (at `to` for moves); None = absent.
    after: Option<String>,
}

enum State {
    Done,
    Pending,
    Diverged,
}

impl JournalEntry {
    fn state(&self, root: &Path, cache_dir: &Path) -> State {
        let (before_here, after_here) = match &self.edit {
            Edit::Create { path, .. }
            | Edit::CreateBinary { path, .. }
            | Edit::Modify { path, .. }
            | Edit::Delete { path } => {
                let now = version_at(root, path);
                (now == self.before, now == self.after)
            }
            Edit::Move { from, to } => {
                let (at_from, at_to) = (version_at(root, from), version_at(root, to));
                (
                    at_from == self.before && at_to.is_none(),
                    at_from.is_none() && at_to == self.after,
                )
            }
        };
        // Staged content that's gone can't be finished: report it instead.
        let unstaged = matches!(&self.edit, Edit::CreateBinary { version, .. }
            if !staged(cache_dir, version).exists());
        if after_here {
            State::Done
        } else if before_here && !unstaged {
            State::Pending
        } else {
            State::Diverged
        }
    }

    fn apply(&self, root: &Path, cache_dir: &Path) -> std::io::Result<()> {
        match &self.edit {
            Edit::Create { path, content } => atomic_write(&root.join(path), content.as_bytes()),
            Edit::CreateBinary { path, version, .. } => {
                atomic_copy(&staged(cache_dir, version), &root.join(path))
            }
            Edit::Modify { path, splices, .. } => {
                let original = fs::read_to_string(root.join(path))?;
                atomic_write(&root.join(path), splice(&original, splices).as_bytes())
            }
            Edit::Move { from, to } => {
                let target = root.join(to);
                if target.exists() {
                    return Err(std::io::Error::other(format!("`{to}` already exists")));
                }
                if let Some(dir) = target.parent() {
                    fs::create_dir_all(dir)?;
                }
                fs::rename(root.join(from), target)
            }
            Edit::Delete { path } => fs::remove_file(root.join(path)),
        }
    }
}

/// An edit from a crashed Plan that recovery left alone because its file had
/// changed in the meantime. Reported by `index_status` and `check`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UnrecoveredEdit {
    pub path: String,
    pub edit: Edit,
}

impl Journal {
    /// Walks the Plan over a virtual copy of the files it touches, recording
    /// each edit's before/after hashes.
    fn new(root: &Path, plan: &Plan) -> Result<Self> {
        let mut files: BTreeMap<String, Option<Vec<u8>>> = BTreeMap::new();
        let state =
            |path: &str, files: &mut BTreeMap<String, Option<Vec<u8>>>| -> Option<Vec<u8>> {
                files
                    .entry(path.to_string())
                    .or_insert_with(|| fs::read(root.join(path)).ok())
                    .clone()
            };
        let hash = |b: &Option<Vec<u8>>| b.as_deref().map(version_of);
        let mut entries = Vec::with_capacity(plan.edits.len());
        for edit in &plan.edits {
            let (before, after) = match edit {
                Edit::Create { path, content } => {
                    let before = state(path, &mut files);
                    let after = Some(content.as_bytes().to_vec());
                    files.insert(path.clone(), after.clone());
                    (hash(&before), hash(&after))
                }
                Edit::CreateBinary { path, version, .. } => {
                    // Not read back: no Plan touches a created Attachment again.
                    let before = state(path, &mut files);
                    (hash(&before), Some(version.clone()))
                }
                Edit::Modify { path, splices, .. } => {
                    let before = state(path, &mut files);
                    let text = before
                        .as_deref()
                        .and_then(|b| std::str::from_utf8(b).ok())
                        .ok_or_else(|| Error::internal(format!("cannot modify `{path}`")))?;
                    let after = Some(splice(text, splices).into_bytes());
                    files.insert(path.clone(), after.clone());
                    (hash(&before), hash(&after))
                }
                Edit::Move { from, to } => {
                    let moving = state(from, &mut files);
                    files.insert(from.clone(), None);
                    files.insert(to.clone(), moving.clone());
                    (hash(&moving), hash(&moving))
                }
                Edit::Delete { path } => {
                    let before = state(path, &mut files);
                    files.insert(path.clone(), None);
                    (hash(&before), None)
                }
            };
            entries.push(JournalEntry {
                edit: edit.clone(),
                before,
                after,
            });
        }
        Ok(Self { entries })
    }

    fn path(cache_dir: &Path) -> PathBuf {
        cache_dir.join("journal.json")
    }

    fn write(&self, wiki: &Wiki) -> Result<()> {
        let bytes = serde_json::to_vec(self).map_err(|e| Error::internal(e.to_string()))?;
        atomic_write(&Self::path(wiki.cache_dir()), &bytes)
            .map_err(|e| Error::io(Some("journal.json"), &e))
    }

    fn remove(cache_dir: &Path) -> Result<()> {
        match fs::remove_file(Self::path(cache_dir)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(Error::io(Some("journal.json"), &e))
            }
            _ => Ok(()),
        }
    }
}

/// Finishes a crashed Plan, if any. Caller holds the write lock.
fn recover(root: &Path, cache_dir: &Path) -> Result<()> {
    let Ok(bytes) = fs::read(Journal::path(cache_dir)) else {
        return Ok(());
    };
    let journal: Journal = serde_json::from_slice(&bytes)
        .map_err(|e| Error::internal(format!("unreadable journal: {e}")))?;
    let mut unrecovered = Vec::new();
    for entry in &journal.entries {
        match entry.state(root, cache_dir) {
            State::Done => {}
            State::Pending => entry
                .apply(root, cache_dir)
                .map_err(|e| Error::io(Some(entry.edit.paths()[0]), &e))?,
            State::Diverged => unrecovered.push(UnrecoveredEdit {
                path: entry.edit.paths()[0].to_string(),
                edit: entry.edit.clone(),
            }),
        }
    }
    remove_empty_dirs(root, &journal);
    unstage(cache_dir);
    if !unrecovered.is_empty() {
        let mut all = read_unrecovered(cache_dir);
        all.extend(unrecovered);
        let bytes = serde_json::to_vec_pretty(&all).map_err(|e| Error::internal(e.to_string()))?;
        atomic_write(&cache_dir.join("unrecovered.json"), &bytes)
            .map_err(|e| Error::io(Some("unrecovered.json"), &e))?;
    }
    Journal::remove(cache_dir)
}

/// Recovery "at open": only when a journal exists, so ordinary opens take no lock.
pub(crate) fn recover_at_open(root: &Path, cache_dir: &Path) -> Result<()> {
    if !Journal::path(cache_dir).exists() {
        return Ok(());
    }
    match crate::wiki::lock_writes(root, cache_dir) {
        Ok(_lock) => recover(root, cache_dir),
        // Another process holds the lock and will recover it itself.
        Err(e) if e.kind == crate::ErrorKind::Conflict => Ok(()),
        Err(e) => Err(e),
    }
}

fn read_unrecovered(cache_dir: &Path) -> Vec<UnrecoveredEdit> {
    fs::read(cache_dir.join("unrecovered.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Edits left alone by journal recovery on this machine.
#[must_use]
pub fn unrecovered_edits(wiki: &Wiki) -> Vec<UnrecoveredEdit> {
    read_unrecovered(wiki.cache_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recovery must tell a finished move from a pending one, and treat
    /// anything else (e.g. the file copied to both places) as diverged.
    #[test]
    fn move_recovery_states() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let entry = JournalEntry {
            edit: Edit::Move {
                from: "a.md".into(),
                to: "b.md".into(),
            },
            before: Some(version_of(b"x")),
            after: Some(version_of(b"x")),
        };
        let state = |a: Option<&str>, b: Option<&str>| {
            for (name, content) in [("a.md", a), ("b.md", b)] {
                match content {
                    Some(c) => fs::write(root.join(name), c).unwrap(),
                    None => {
                        let _ = fs::remove_file(root.join(name));
                    }
                }
            }
            match entry.state(root, root) {
                State::Done => "done",
                State::Pending => "pending",
                State::Diverged => "diverged",
            }
        };
        assert_eq!(state(Some("x"), None), "pending");
        assert_eq!(state(None, Some("x")), "done");
        assert_eq!(state(Some("x"), Some("x")), "diverged", "copied, not moved");
        assert_eq!(state(Some("y"), None), "diverged", "edited since");
        assert_eq!(state(None, None), "diverged", "gone");
    }

    #[test]
    fn binary_creates_finish_from_staged_content_or_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (root, cache) = (dir.path().join("wiki"), dir.path().join("cache"));
        fs::create_dir_all(&root).unwrap();
        let bytes = b"\x89PNG\x00\xff".to_vec();
        let version = version_of(&bytes);
        let entry = JournalEntry {
            edit: Edit::CreateBinary {
                path: "a/d.png".into(),
                size: bytes.len() as u64,
                version: version.clone(),
            },
            before: None,
            after: Some(version.clone()),
        };
        let journal = Journal {
            entries: vec![entry],
        };
        let write_journal = || {
            atomic_write(
                &Journal::path(&cache),
                &serde_json::to_vec(&journal).unwrap(),
            )
            .unwrap();
        };

        write_journal();
        stage(
            &cache,
            BTreeMap::from([(version.clone(), Blob::Bytes(bytes.clone()))]),
        )
        .unwrap();
        recover(&root, &cache).unwrap();
        assert_eq!(fs::read(root.join("a/d.png")).unwrap(), bytes, "finished");
        assert!(!cache.join("staged").exists(), "staging is cleared");

        fs::remove_file(root.join("a/d.png")).unwrap();
        write_journal();
        recover(&root, &cache).unwrap();
        assert!(!root.join("a/d.png").exists());
        assert_eq!(
            read_unrecovered(&cache).len(),
            1,
            "no staged bytes: reported"
        );
        assert!(!Journal::path(&cache).exists());
    }

    #[test]
    fn a_source_file_changed_after_planning_is_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("src.bin");
        fs::write(&source, b"one").unwrap();
        let err = stage(
            dir.path(),
            BTreeMap::from([(version_of(b"two"), Blob::File(source))]),
        )
        .unwrap_err();
        assert_eq!(err.kind, crate::ErrorKind::Conflict);
        assert_eq!(
            version_of_file(&dir.path().join("src.bin")).unwrap(),
            (version_of(b"one"), 3),
            "streamed hash equals the one-shot hash"
        );
    }
}
