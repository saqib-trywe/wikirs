//! Plans and the mutation engine (ADR 0006, process-model.md#mutations).
//!
//! Every mutation runs through [`mutate`]: take the Wiki-wide write lock,
//! finish any crashed Plan, compute the Plan while recording the hash of every
//! file read, stop there on a dry run, re-check those hashes, journal the Plan,
//! apply it (creates/modifies before moves before deletes), drop the journal.

use std::{
    collections::BTreeMap,
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
    /// Splice an existing file. Every byte outside the splices is unchanged.
    Modify {
        path: String,
        /// Hash of the file this edit was planned against.
        base_version: String,
        /// Non-overlapping byte ranges of the original file, in order.
        splices: Vec<Splice>,
    },
}

impl Edit {
    fn path(&self) -> &str {
        match self {
            Edit::Create { path, .. } | Edit::Modify { path, .. } => path,
        }
    }
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

// ------------------------------------------------------------ transaction

/// What a mutation sees while planning: reads are recorded so apply can
/// detect files that changed underneath it.
pub struct Tx<'w> {
    wiki: &'w Wiki,
    reads: BTreeMap<String, Option<String>>,
    edits: Vec<Edit>,
    warnings: Vec<Warning>,
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

    pub fn edit(&mut self, edit: Edit) {
        self.edits.push(edit);
    }

    pub fn warn(&mut self, kind: &str, message: impl Into<String>) {
        self.warnings.push(Warning {
            kind: kind.to_string(),
            message: message.into(),
        });
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
    };
    let value = plan(&mut tx)?;
    let Tx {
        reads,
        edits,
        warnings,
        ..
    } = tx;
    let plan = Plan { edits, warnings };
    if dry_run || plan.edits.is_empty() {
        return Ok((plan, value));
    }

    // Anything read while planning must still be exactly as read.
    let changed: Vec<ChangedFile> = reads
        .into_iter()
        .filter_map(|(path, expected)| {
            let actual = current_version(wiki, &path);
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

    let journal = Journal::new(wiki, &plan)?;
    journal.write(wiki)?;
    for (applied, entry) in journal.entries.iter().enumerate() {
        crash_hook(applied);
        entry
            .edit
            .apply(wiki.root())
            .map_err(|e| Error::io(Some(entry.edit.path()), &e))?;
    }
    Journal::remove(wiki.cache_dir())?;
    // The Index is a cache: if this fails, the next reconcile repairs it.
    let touched: Vec<&str> = journal.entries.iter().map(|e| e.edit.path()).collect();
    let _ = wiki.index().refresh(wiki.root(), &touched);
    Ok((plan, value))
}

fn current_version(wiki: &Wiki, rel: &str) -> Option<String> {
    version_at(wiki.root(), rel)
}

fn version_at(root: &Path, rel: &str) -> Option<String> {
    fs::read(root.join(rel)).ok().map(|b| version_of(&b))
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

impl Edit {
    /// The file content after this edit, given the content before it.
    fn result(&self, before: Option<&str>) -> std::io::Result<String> {
        match (self, before) {
            (Edit::Create { content, .. }, None) => Ok(content.clone()),
            (Edit::Modify { splices, .. }, Some(original)) => Ok(splice(original, splices)),
            _ => Err(std::io::Error::other(
                "file is not in the state this edit expects",
            )),
        }
    }

    fn apply(&self, root: &Path) -> std::io::Result<()> {
        let target = root.join(self.path());
        let before = match fs::read_to_string(&target) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        let after = self.result(before.as_deref())?;
        atomic_write(&target, after.as_bytes())
    }
}

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
    let dir = target.parent().expect("target is inside the Wiki");
    fs::create_dir_all(dir)?;
    let name = target
        .file_name()
        .expect("target has a file name")
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.wikirs-tmp"));
    let mut file = fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, target)
}

// ---------------------------------------------------------------- journal

/// A Plan being applied, with each file's hash before and after its edit.
#[derive(Debug, Serialize, Deserialize)]
struct Journal {
    entries: Vec<JournalEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalEntry {
    edit: Edit,
    before: Option<String>,
    after: String,
}

/// An edit from a crashed Plan that recovery left alone because its file had
/// changed in the meantime. Reported by `index_status` and `check`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UnrecoveredEdit {
    pub path: String,
    pub edit: Edit,
}

impl Journal {
    fn new(wiki: &Wiki, plan: &Plan) -> Result<Self> {
        // Creates and modifies come before moves and deletes (none exist yet),
        // so a crash never loses content.
        let entries = plan
            .edits
            .iter()
            .map(|edit| {
                let before = fs::read_to_string(wiki.root().join(edit.path())).ok();
                let after = edit
                    .result(before.as_deref())
                    .map_err(|e| Error::io(Some(edit.path()), &e))?;
                Ok(JournalEntry {
                    edit: edit.clone(),
                    before: before.map(|b| version_of(b.as_bytes())),
                    after: version_of(after.as_bytes()),
                })
            })
            .collect::<Result<_>>()?;
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
        let current = version_at(root, entry.edit.path());
        if current.as_deref() == Some(entry.after.as_str()) {
            continue; // already applied
        }
        if current == entry.before {
            entry
                .edit
                .apply(root)
                .map_err(|e| Error::io(Some(entry.edit.path()), &e))?;
        } else {
            unrecovered.push(UnrecoveredEdit {
                path: entry.edit.path().to_string(),
                edit: entry.edit.clone(),
            });
        }
    }
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
