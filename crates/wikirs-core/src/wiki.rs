//! The Wiki handle, Page Paths, and finding a Wiki's root (docs/spec/wiki-selection.md).

use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use crate::{
    Error, Result,
    index::{Index, Scope},
    settings::Settings,
    watch::{Hub, WatchEvent, Watcher},
};

/// How long a mutation waits for another process's write lock (ADR 0006).
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Wiki {
    root: PathBuf,
    cache_dir: PathBuf,
    /// Per-Wiki state outside the Wiki: the `serve` token.
    state_dir: PathBuf,
    machine_file: PathBuf,
    index: Arc<Mutex<Index>>,
    /// The adapter can read and write local paths (interfaces.md#capabilities).
    local_fs: bool,
    /// `watch` subscribers and what the watcher last saw.
    hub: Arc<Hub>,
    /// This handle's file watcher, once started (long-lived processes).
    watcher: Arc<Mutex<Option<Watcher>>>,
}

impl Wiki {
    /// Opens the Wiki at `root`, finishing any Plan a crashed process left in
    /// its journal (process-model.md).
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_in(root.as_ref(), &cache_base(), &config_base(), &state_base())
    }

    /// Like [`Wiki::open`], with everything per machine under `base` instead of
    /// the user's dirs: cache dirs in `base/<wiki key>/`, machine settings in
    /// `base/config/`, state in `base/state/` (tests and embedders).
    pub fn open_isolated(root: impl AsRef<Path>, base: impl AsRef<Path>) -> Result<Self> {
        let base = base.as_ref();
        Self::open_in(
            root.as_ref(),
            base,
            &base.join("config"),
            &base.join("state"),
        )
    }

    fn open_in(
        given: &Path,
        cache_base: &Path,
        config_base: &Path,
        state_base: &Path,
    ) -> Result<Self> {
        let root = given
            .canonicalize()
            .ok()
            .filter(|p| p.is_dir())
            .ok_or_else(|| Error::not_found("wiki", &given.display().to_string()))?;
        let key = wiki_key(&root);
        let machine_file = config_base.join("wikis").join(format!("{key}.toml"));
        let settings = Settings::load(&root, &machine_file);
        let cache_dir = settings
            .cache_dir()
            .unwrap_or_else(|| cache_base.join(&key));
        let mut index = Index::open(&cache_dir.join("index.db"), settings.stemming())?;
        crate::plan::recover_at_open(&root, &cache_dir)?;
        // Every open brings the Index up to date (process-model.md#freshness).
        index.reconcile(&root, &settings.ignore())?;
        Ok(Self {
            root,
            cache_dir,
            state_dir: state_base.join(&key),
            machine_file,
            index: Arc::new(Mutex::new(index)),
            local_fs: true,
            hub: Arc::default(),
            watcher: Arc::default(),
        })
    }

    /// This handle for an adapter without local file access (HTTP): inputs
    /// naming a `local_path` are refused with `InvalidInput`.
    #[must_use]
    pub fn without_local_fs(mut self) -> Self {
        self.local_fs = false;
        self
    }

    /// Whether `local_path` inputs are allowed.
    #[must_use]
    pub fn local_fs(&self) -> bool {
        self.local_fs
    }

    /// Both settings files, as they are now.
    #[must_use]
    pub fn settings(&self) -> Settings {
        Settings::load(&self.root, &self.machine_file)
    }

    /// This machine's settings file for the Wiki (outside it).
    #[must_use]
    pub fn machine_file(&self) -> &Path {
        &self.machine_file
    }

    /// Applies changed Index settings (`ignore`, `search.stemming`) to this
    /// handle: reopening the Index rebuilds it if the tokenizer changed, and a
    /// reconcile drops newly ignored files and adds unignored ones.
    pub fn reload_index_settings(&self) -> Result<()> {
        let settings = self.settings();
        let mut index = self.index();
        *index = Index::open(&self.cache_dir.join("index.db"), settings.stemming())?;
        index.reconcile(&self.root, &settings.ignore())
    }

    /// The shared Index connection for this handle.
    pub fn index(&self) -> MutexGuard<'_, Index> {
        self.index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Subscribes to change events in `scope`: this handle's own mutations
    /// always, and other writers' changes once [`Wiki::start_watcher`] runs.
    /// Events arrive until the receiver is dropped.
    #[must_use]
    pub fn watch(&self, scope: Scope) -> std::sync::mpsc::Receiver<WatchEvent> {
        self.hub.subscribe(scope)
    }

    pub(crate) fn hub(&self) -> &Hub {
        &self.hub
    }

    pub(crate) fn watcher_slot(&self) -> MutexGuard<'_, Option<Watcher>> {
        self.watcher
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// This handle without its watcher, for the watcher's own callback (so
    /// the watcher doesn't keep itself alive).
    pub(crate) fn detached(&self) -> Self {
        Self {
            watcher: Arc::default(),
            ..self.clone()
        }
    }

    /// Per-Wiki cache dir: Index, write lock, journal (process-model.md).
    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Per-Wiki state dir, outside the Wiki and the cache: `<state dir>/wikirs/<wiki key>/`.
    #[must_use]
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
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

/// `<state dir>/wikirs` (`~/.local/state`, or Application Support on macOS),
/// or `WIKIRS_STATE_DIR` (tests and CI).
fn state_base() -> PathBuf {
    std::env::var_os("WIKIRS_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            dirs::state_dir()
                .or_else(dirs::data_local_dir)
                .map(|d| d.join("wikirs"))
        })
        .unwrap_or_else(|| std::env::temp_dir().join("wikirs-state"))
}

/// 16 hex chars of blake3(canonical root path).
fn wiki_key(root: &Path) -> String {
    blake3::hash(root.to_string_lossy().as_bytes()).to_hex()[..16].to_string()
}

/// `~/.config/wikirs` on every OS (or `$XDG_CONFIG_HOME/wikirs`), or
/// `WIKIRS_CONFIG_DIR` (tests and CI), for the user config and machine settings.
#[must_use]
pub fn config_base() -> PathBuf {
    if let Some(dir) = std::env::var_os("WIKIRS_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| std::env::temp_dir().join("wikirs-config"))
        .join("wikirs")
}

/// Which steps of the resolution order apply (wiki-selection.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discovery {
    /// CLI, TUI, `serve`: flag, env, walk up from the cwd, `default_wiki`.
    Cli,
    /// `mcp`: like `Cli` but never walks up (a client's cwd is unpredictable).
    Mcp,
    /// `init`: flag or env, else the cwd itself.
    Init,
}

/// Finds the Wiki root: `--wiki`, then `WIKIRS_WIKI`, then (for `Cli`) the
/// nearest folder above `cwd` containing `.wikirs/`, then `default_wiki` from
/// the user config in `config_base`.
pub fn resolve_root(
    flag: Option<&str>,
    env: Option<&str>,
    cwd: &Path,
    discovery: Discovery,
    config_base: &Path,
) -> Result<PathBuf> {
    let user = UserConfig::load(config_base);
    if let Some(given) = flag.or(env) {
        return Ok(user.wiki_value(given, cwd));
    }
    match discovery {
        Discovery::Init => return Ok(cwd.to_path_buf()),
        Discovery::Cli => {
            if let Some(found) = cwd.ancestors().find(|dir| dir.join(".wikirs").is_dir()) {
                return Ok(found.to_path_buf());
            }
        }
        Discovery::Mcp => {}
    }
    if let Some(default) = &user.default_wiki {
        return Ok(user.wiki_value(default, cwd));
    }
    let mut err = Error::not_found("wiki", &cwd.display().to_string());
    err.message = "no Wiki found: run 'wikirs init' here, pass --wiki, or set default_wiki".into();
    Err(err)
}

/// `<config base>/config.toml`, written by the user: `default_wiki` and named
/// Wikis (`[wikis] notes = "~/notes"`). Unreadable means empty.
#[derive(Debug, Default)]
struct UserConfig {
    default_wiki: Option<String>,
    wikis: Vec<(String, String)>,
}

impl UserConfig {
    fn load(config_base: &Path) -> Self {
        let Some(doc) = std::fs::read_to_string(config_base.join("config.toml"))
            .ok()
            .and_then(|t| t.parse::<toml_edit::DocumentMut>().ok())
        else {
            return Self::default();
        };
        Self {
            default_wiki: doc
                .get("default_wiki")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            wikis: doc
                .get("wikis")
                .and_then(toml_edit::Item::as_table_like)
                .into_iter()
                .flat_map(|t| t.iter())
                .filter_map(|(name, v)| Some((name.to_string(), v.as_str()?.to_string())))
                .collect(),
        }
    }

    /// A `--wiki` value: a path if it contains `/` or starts with `~` or `.`;
    /// otherwise a named Wiki, else a path relative to `cwd`.
    fn wiki_value(&self, value: &str, cwd: &Path) -> PathBuf {
        let is_path = value.contains('/') || value.starts_with(['~', '.']);
        let named = (!is_path)
            .then(|| self.wikis.iter().find(|(n, _)| n == value))
            .flatten();
        match named {
            Some((_, path)) => cwd.join(expand_home(path)),
            None => cwd.join(expand_home(value)),
        }
    }
}

pub(crate) fn expand_home(path: &str) -> PathBuf {
    let home = dirs::home_dir();
    match (path, path.strip_prefix("~/"), home) {
        ("~", _, Some(home)) => home,
        (_, Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// A validated Page Path: relative to the Wiki root, no `.md`, no hidden segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagePath(String);

/// Checks a path from the Wiki root: `/`-separated, no empty, `.`, `..` or
/// hidden segments, and no `.md` extension (`md_reason` says why).
fn check_rel_path(raw: &str, md_reason: &'static str) -> Result<()> {
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
        return bad(md_reason);
    }
    for segment in raw.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return bad("malformed");
        }
        if segment.starts_with('.') {
            return bad("hidden segment");
        }
    }
    Ok(())
}

impl PagePath {
    pub fn parse(raw: &str) -> Result<Self> {
        check_rel_path(raw, "page paths have no .md extension")?;
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

/// An Attachment's path from the Wiki root, extension included (if any).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentPath(String);

impl AttachmentPath {
    pub fn parse(raw: &str) -> Result<Self> {
        check_rel_path(raw, "a .md file is a Page, not an Attachment")?;
        Ok(Self(raw.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Rejects a new path whose file or any folder differs from an existing entry
/// only by case (Page identity decision, item 10).
pub fn check_case_conflict(wiki: &Wiki, page: &PagePath) -> Result<()> {
    case_conflict(wiki, page.as_str(), true)
}

/// [`check_case_conflict`] for an Attachment's exact file name.
pub fn check_attachment_case(wiki: &Wiki, path: &AttachmentPath) -> Result<()> {
    case_conflict(wiki, path.as_str(), false)
}

/// For a Page, the last segment is compared as both `name.md` and folder `name`.
fn case_conflict(wiki: &Wiki, rel: &str, page: bool) -> Result<()> {
    let segments: Vec<&str> = rel.split('/').collect();
    let mut dir = wiki.root().to_path_buf();
    let mut prefix = String::new();
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        let wanted: Vec<String> = if last && page {
            vec![format!("{segment}.md"), (*segment).to_string()]
        } else {
            vec![(*segment).to_string()]
        };
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                for want in &wanted {
                    if name != *want && name.to_lowercase() == want.to_lowercase() {
                        let shown = if page {
                            name.trim_end_matches(".md")
                        } else {
                            &name
                        };
                        return Err(Error::case_conflict(rel, &format!("{prefix}{shown}")));
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
    fn roots_resolve_in_the_documented_order() {
        let dir = tempfile::tempdir().unwrap();
        let (home, config) = (dir.path().join("w"), dir.path().join("config"));
        let inner = home.join("a").join("b");
        std::fs::create_dir_all(home.join(".wikirs")).unwrap();
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        let resolve = |flag: Option<&str>, env: Option<&str>, discovery| {
            resolve_root(flag, env, &inner, discovery, &config)
        };
        // Absolute paths elsewhere, as strings (`--wiki` values).
        let abs = |name: &str| dir.path().join(name);
        let text = |path: &Path| path.to_str().unwrap().to_string();
        let (x, y, n) = (abs("x"), abs("y"), abs("n"));

        assert_eq!(
            resolve(None, None, Discovery::Cli).unwrap(),
            home,
            "walk up"
        );
        let err = resolve(None, None, Discovery::Mcp).unwrap_err();
        assert_eq!(err.kind, crate::ErrorKind::NotFound, "mcp never walks up");
        assert_eq!(
            resolve(None, None, Discovery::Init).unwrap(),
            inner,
            "init: the cwd"
        );
        assert_eq!(
            resolve(Some(&text(&x)), Some(&text(&y)), Discovery::Init).unwrap(),
            x,
            "the flag beats the env var"
        );
        assert_eq!(resolve(None, Some(&text(&y)), Discovery::Cli).unwrap(), y);

        // TOML literal strings, so Windows backslashes need no escaping.
        std::fs::write(
            config.join("config.toml"),
            format!(
                "default_wiki = \"notes\"\n[wikis]\nnotes = '{}'\n\".n\" = '{}'\n",
                text(&n),
                text(&abs("dot"))
            ),
        )
        .unwrap();
        assert_eq!(resolve(None, None, Discovery::Mcp).unwrap(), n);
        assert_eq!(
            resolve(None, None, Discovery::Cli).unwrap(),
            home,
            "walk-up comes first"
        );
        assert_eq!(resolve(Some("notes"), None, Discovery::Cli).unwrap(), n);
        assert_eq!(
            resolve(Some("./notes"), None, Discovery::Cli).unwrap(),
            inner.join("./notes"),
            "a path-looking value is never a name"
        );
        assert_eq!(
            resolve(Some(".n"), None, Discovery::Cli).unwrap(),
            inner.join(".n"),
            "nor is one starting with `.`"
        );
        assert_eq!(
            resolve(Some("other"), None, Discovery::Cli).unwrap(),
            inner.join("other"),
            "an unknown name is a relative path"
        );
    }

    #[test]
    fn page_paths_reject_malformed_input() {
        for bad in ["", "a//b", "/a", "a/", "../a", "a/.hidden", "a.md", "a\\b"] {
            assert!(PagePath::parse(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(PagePath::parse("eng/rust/async-notes").is_ok());
    }
}
