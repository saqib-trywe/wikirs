//! `watch`: a stream of change events (operations.md#subscription,
//! process-model.md#watch-events).
//!
//! Events come from two places. A mutation publishes its Plan's changes right
//! after the Index commit (so a move is a real `page_moved`). The watcher turns
//! debounced filesystem events into the same changes by comparing each touched
//! file with the version this process last saw, then refreshes the Index.
//!
//! "Last saw" is per process, not the shared Index: another process's
//! mutation has already updated the Index by the time our watcher sees its
//! files, and its changes must still reach our subscribers. A mutation updates
//! the versions under the same lock the watcher compares under, so a process
//! never hears its own writes twice.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Component, Path},
    sync::{
        Mutex, MutexGuard, PoisonError,
        mpsc::{Receiver, Sender, channel},
    },
    time::Duration,
};

use notify_debouncer_full::{
    DebounceEventResult, RecommendedCache, new_debouncer, new_debouncer_opt,
    notify::{self, PollWatcher, RecursiveMode},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Kind, Operation, Result, Wiki,
    index::{Scope, files_under, indexable, page_path},
    plan::version_of_file,
};

/// Filesystem events settle for this long before a batch is handled.
const DEBOUNCE: Duration = Duration::from_millis(200);
/// The `poll` watcher's interval.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

// --------------------------------------------------------------------- watch

pub struct Watch;

/// Stream change events for the Wiki, or a part of it.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct WatchInput {
    /// Only changes in this part of the Wiki.
    #[serde(default)]
    #[cfg_attr(feature = "clap", command(flatten))]
    pub scope: Scope,
}

/// One change. Events carry no content: call `get_page` for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct WatchEvent {
    pub kind: EventKind,
    /// Page Path for Page events, Attachment path for `attachment_changed`;
    /// absent for `index_updated`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The old path of a move made by a wikirs Plan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The file's new `Version`; absent once it's gone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    PageCreated,
    PageModified,
    PageDeleted,
    /// Only for moves made by a wikirs Plan; other moves are a delete and a create.
    PageMoved,
    AttachmentChanged,
    /// The Index took in a batch of changes: sent once, after the batch's events.
    IndexUpdated,
}

impl Operation for Watch {
    const NAME: &'static str = "watch";
    const KIND: Kind = Kind::Subscription;
    const DESCRIPTION: &'static str = "Stream change events (Pages, Attachments, the Index) as they happen, from this process and every other writer.";
    type Input = WatchInput;
    /// One event of the stream.
    type Output = WatchEvent;

    fn run(_wiki: &Wiki, _input: WatchInput) -> Result<WatchEvent> {
        Err(Error::invalid_input(
            None,
            "`watch` is a subscription: it streams events and has no single result",
        ))
    }
}

impl Watch {
    /// Starts this process's watcher (if it isn't running) and subscribes.
    /// Events arrive until the receiver is dropped.
    pub fn subscribe(wiki: &Wiki, input: WatchInput) -> Result<Receiver<WatchEvent>> {
        wiki.start_watcher()?;
        Ok(wiki.watch(input.scope))
    }
}

// ----------------------------------------------------------------------- hub

/// How this process watches the files (`index_status.watcher`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatcherMode {
    None,
    Native,
    Poll,
}

impl WatcherMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            WatcherMode::None => "none",
            WatcherMode::Native => "native",
            WatcherMode::Poll => "poll",
        }
    }
}

/// One `Wiki` handle's subscribers, and the file versions its watcher compares against.
#[derive(Default)]
pub(crate) struct Hub {
    subscribers: Mutex<Vec<(Scope, Sender<WatchEvent>)>>,
    /// Each indexable file's version as this process last saw it; `None`
    /// until the watcher starts.
    seen: Mutex<Option<HashMap<String, String>>>,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Hub")
    }
}

/// A running watcher; dropping it stops watching.
pub(crate) struct Watcher {
    pub(crate) mode: WatcherMode,
    _debouncer: Box<dyn Send>,
}

impl std::fmt::Debug for Watcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watcher")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Hub {
    pub(crate) fn subscribe(&self, scope: Scope) -> Receiver<WatchEvent> {
        let (tx, rx) = channel();
        lock(&self.subscribers).push((scope, tx));
        rx
    }

    /// The versions the watcher compares against. Held by a mutation while it
    /// applies, so the watcher never sees its files half-way.
    pub(crate) fn seen(&self) -> MutexGuard<'_, Option<HashMap<String, String>>> {
        lock(&self.seen)
    }

    /// Sends each subscriber the changes in its scope, then `index_updated`
    /// if the Index took them in. Subscribers that hung up are dropped.
    pub(crate) fn publish(&self, changes: &[Change], index_updated: bool) {
        if changes.is_empty() {
            return;
        }
        lock(&self.subscribers).retain(|(scope, tx)| {
            let mine: Vec<&Change> = changes.iter().filter(|c| c.in_scope(scope)).collect();
            if mine.is_empty() {
                return true;
            }
            let events = mine.into_iter().map(Change::event);
            let updated = index_updated.then_some(WatchEvent {
                kind: EventKind::IndexUpdated,
                path: None,
                from: None,
                version: None,
            });
            events.chain(updated).all(|e| tx.send(e).is_ok())
        });
    }
}

// ------------------------------------------------------------------- changes

/// One file's net change: its version before and after, by relative path.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Change {
    pub path: String,
    /// Set when a Plan moved the file here.
    pub from: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
}

impl Change {
    fn in_scope(&self, scope: &Scope) -> bool {
        scope.contains(&self.path) || self.from.as_ref().is_some_and(|f| scope.contains(f))
    }

    fn event(&self) -> WatchEvent {
        let page = Path::new(&self.path).extension().is_some_and(|e| e == "md");
        let kind = match (page, &self.from, &self.before, &self.after) {
            (false, ..) => EventKind::AttachmentChanged,
            (true, Some(_), ..) => EventKind::PageMoved,
            (true, None, None, _) => EventKind::PageCreated,
            (true, None, Some(_), Some(_)) => EventKind::PageModified,
            (true, None, Some(_), None) => EventKind::PageDeleted,
        };
        let shown = |p: &String| if page { page_path(p) } else { p.clone() };
        WatchEvent {
            kind,
            path: Some(shown(&self.path)),
            from: self.from.as_ref().map(shown),
            version: self.after.clone(),
        }
    }
}

/// Folds a Plan's edits, as `(from, path, before, after)` in apply order
/// (`from` set for a move), into one change per affected path. A move and any
/// edit of the same file become one moved change.
pub(crate) fn net_changes(
    edits: impl IntoIterator<Item = (Option<String>, String, Option<String>, Option<String>)>,
) -> Vec<Change> {
    let mut net: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
    let mut moves = Vec::new();
    for (from, path, before, after) in edits {
        if let Some(from) = from {
            net.entry(from.clone()).or_insert((before, None)).1 = None;
            net.entry(path.clone()).or_insert((None, None)).1 = after;
            moves.push((from, path));
        } else {
            net.entry(path).or_insert((before, None)).1 = after;
        }
    }
    let mut changes = Vec::new();
    for (from, to) in moves {
        let (Some((before, _)), Some((_, after))) = (net.remove(&from), net.remove(&to)) else {
            continue; // a chained move, already folded
        };
        changes.push(Change {
            path: to,
            from: Some(from),
            before,
            after,
        });
    }
    changes.extend(
        net.into_iter()
            .filter(|(_, (before, after))| before != after)
            .map(|(path, (before, after))| Change {
                path,
                from: None,
                before,
                after,
            }),
    );
    changes
}

// ------------------------------------------------------------------- watcher

impl Wiki {
    /// Starts watching the files, if this handle isn't already: `watcher =
    /// "native"` (the default) falls back to polling if it can't start.
    pub fn start_watcher(&self) -> Result<()> {
        let mut slot = self.watcher_slot();
        if slot.is_some() {
            return Ok(());
        }
        // Lock order everywhere: `seen`, then the Index.
        let mut seen = self.hub().seen();
        *seen = Some(self.index().versions()?);
        drop(seen);
        let watcher = if self.settings().watcher_poll() {
            self.poll_watcher()?
        } else {
            match self.native_watcher() {
                Ok(watcher) => watcher,
                Err(_) => self.poll_watcher()?,
            }
        };
        *slot = Some(watcher);
        Ok(())
    }

    /// How this handle watches the files.
    #[must_use]
    pub fn watcher_mode(&self) -> WatcherMode {
        self.watcher_slot()
            .as_ref()
            .map_or(WatcherMode::None, |w| w.mode)
    }

    fn native_watcher(&self) -> Result<Watcher> {
        let mut debouncer =
            new_debouncer(DEBOUNCE, None, self.batch_handler()).map_err(|e| watch_err(&e))?;
        debouncer
            .watch(self.root(), RecursiveMode::Recursive)
            .map_err(|e| watch_err(&e))?;
        Ok(Watcher {
            mode: WatcherMode::Native,
            _debouncer: Box::new(debouncer),
        })
    }

    fn poll_watcher(&self) -> Result<Watcher> {
        let config = notify::Config::default().with_poll_interval(POLL_INTERVAL);
        let mut debouncer = new_debouncer_opt::<_, PollWatcher, _>(
            DEBOUNCE,
            None,
            self.batch_handler(),
            RecommendedCache::new(),
            config,
        )
        .map_err(|e| watch_err(&e))?;
        debouncer
            .watch(self.root(), RecursiveMode::Recursive)
            .map_err(|e| watch_err(&e))?;
        Ok(Watcher {
            mode: WatcherMode::Poll,
            _debouncer: Box::new(debouncer),
        })
    }

    /// The debouncer's callback. It holds a copy of this handle without the
    /// watcher itself, so dropping the last handle stops watching.
    fn batch_handler(&self) -> impl FnMut(DebounceEventResult) + Send + 'static {
        let wiki = self.detached();
        move |result| {
            let batch = match result {
                Ok(events) => Batch {
                    paths: events.iter().flat_map(|e| e.paths.clone()).collect(),
                    rescan: events.iter().any(|e| e.need_rescan()),
                    sweep: events.iter().any(|e| {
                        matches!(
                            e.kind,
                            notify::EventKind::Create(_)
                                | notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
                        )
                    }),
                },
                Err(_) => Batch {
                    rescan: true,
                    ..Batch::default()
                },
            };
            wiki.take_in(&batch);
        }
    }

    /// Handles one debounced batch: finds the files that differ from what
    /// this process last saw, refreshes the Index, and publishes the changes.
    pub(crate) fn take_in(&self, batch: &Batch) {
        let ignore = self.settings().ignore();
        let mut seen = self.hub().seen();
        let Some(seen) = seen.as_mut() else {
            return;
        };
        let mut candidates: BTreeSet<String> = BTreeSet::new();
        let mut rescan = batch.rescan;
        if batch.sweep {
            candidates.extend(
                seen.keys()
                    .filter(|k| {
                        !std::fs::symlink_metadata(self.root().join(k)).is_ok_and(|m| m.is_file())
                    })
                    .cloned(),
            );
        }
        for path in &batch.paths {
            match relative(self.root(), path) {
                Some(rel) if rel.is_empty() => rescan = true,
                Some(rel) => {
                    let under = format!("{rel}/");
                    candidates.extend(seen.keys().filter(|k| k.starts_with(&under)).cloned());
                    if self.root().join(&rel).is_dir() {
                        candidates.extend(files_under(self.root(), &rel, &ignore));
                    }
                    candidates.insert(rel);
                }
                None => {}
            }
        }
        if rescan {
            candidates.extend(seen.keys().cloned());
            candidates.extend(files_under(self.root(), "", &ignore));
        }

        let mut changes = Vec::new();
        for rel in candidates {
            let full = self.root().join(&rel);
            let now = (indexable(&rel, &ignore)
                && std::fs::symlink_metadata(&full).is_ok_and(|m| m.is_file()))
            .then(|| version_of_file(&full).ok().map(|(v, _)| v))
            .flatten();
            let before = seen.get(&rel).cloned();
            if now == before {
                continue;
            }
            match &now {
                Some(v) => seen.insert(rel.clone(), v.clone()),
                None => seen.remove(&rel),
            };
            changes.push(Change {
                path: rel,
                from: None,
                before,
                after: now,
            });
        }
        if changes.is_empty() {
            return;
        }
        let touched: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
        let updated = self.index().refresh(self.root(), &touched, &ignore).is_ok();
        self.hub().publish(&changes, updated);
    }
}

/// One debounced batch of filesystem events.
#[derive(Debug, Default)]
pub(crate) struct Batch {
    /// Every path the events name (absolute).
    pub paths: Vec<std::path::PathBuf>,
    /// Compare every file: events were lost (overflow, watcher errors).
    pub rescan: bool,
    /// Check that every known file still exists: the batch has a create or a
    /// rename, and some backends report only a move's new path (macOS
    /// folds a rename of a file it once saw created into a create).
    pub sweep: bool,
}

/// `path` relative to `root`, `/`-separated; `None` if it's outside or not UTF-8.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let rest = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = rest
        .components()
        .map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    Some(parts?.join("/"))
}

fn watch_err(e: &notify::Error) -> Error {
    Error::internal(format!("watcher: {e}"))
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use serde_json::{Value, json};

    use super::*;
    use crate::find;

    fn wiki() -> (tempfile::TempDir, Wiki) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("wiki");
        std::fs::create_dir_all(root.join("eng")).unwrap();
        std::fs::write(root.join("eng.md"), "# Eng\n\n[rust](eng/rust.md)\n").unwrap();
        std::fs::write(root.join("eng/rust.md"), "# Rust\n").unwrap();
        let wiki = Wiki::open_isolated(&root, dir.path().join("base")).unwrap();
        (dir, wiki)
    }

    fn call(wiki: &Wiki, op: &str, input: Value) -> Value {
        find(op).unwrap().call(wiki, input).unwrap()
    }

    /// Everything received until nothing arrives for `quiet`.
    fn drain(rx: &Receiver<WatchEvent>, quiet: Duration) -> Vec<WatchEvent> {
        std::iter::from_fn(|| rx.recv_timeout(quiet).ok()).collect()
    }

    /// Waits up to `within` for events, then collects any stragglers.
    fn next_batch(rx: &Receiver<WatchEvent>, within: Duration) -> Vec<WatchEvent> {
        let first = rx.recv_timeout(within).expect("an event in time");
        std::iter::once(first)
            .chain(drain(rx, Duration::from_millis(600)))
            .collect()
    }

    fn short(events: &[WatchEvent]) -> Vec<String> {
        events
            .iter()
            .map(|e| {
                let kind = serde_json::to_value(e.kind).unwrap();
                let from = e.from.as_ref().map(|f| format!("{f} ->"));
                std::iter::once(kind.as_str().unwrap().to_string())
                    .chain(from)
                    .chain(e.path.clone())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    fn ev(kind: EventKind, path: &str) -> WatchEvent {
        WatchEvent {
            kind,
            path: Some(path.into()),
            from: None,
            version: None,
        }
    }

    #[test]
    fn a_plan_folds_into_one_change_per_path() {
        let s = |v: &str| Some(v.to_string());
        let changes = net_changes([
            // modify then move the same file: one move, planned hash to final hash
            (None, "a.md".into(), s("a0"), s("a1")),
            (s("a.md"), "b.md".into(), s("a1"), s("a1")),
            (None, "c.md".into(), None, s("c1")),
            (None, "d.md".into(), s("d0"), None),
            (None, "e.md".into(), s("e0"), s("e0")),
        ]);
        let change = |from: Option<&str>, path: &str, before, after| Change {
            path: path.into(),
            from: from.map(str::to_string),
            before,
            after,
        };
        assert_eq!(
            changes,
            vec![
                change(Some("a.md"), "b.md", s("a0"), s("a1")),
                change(None, "c.md", None, s("c1")),
                change(None, "d.md", s("d0"), None),
            ],
            "an unchanged file is no change"
        );
        let kinds: Vec<EventKind> = changes.iter().map(|c| c.event().kind).collect();
        assert_eq!(
            kinds,
            [
                EventKind::PageMoved,
                EventKind::PageCreated,
                EventKind::PageDeleted
            ]
        );
        assert_eq!(
            Change {
                path: "x.png".into(),
                from: Some("y.png".into()),
                before: None,
                after: s("1"),
            }
            .event(),
            WatchEvent {
                kind: EventKind::AttachmentChanged,
                path: Some("x.png".into()),
                from: Some("y.png".into()),
                version: s("1"),
            },
            "Attachments keep their extension"
        );
    }

    #[test]
    fn own_mutations_publish_their_plan_then_index_updated() {
        let (_dir, wiki) = wiki();
        let rx = wiki.watch(Scope::default());

        let created = call(
            &wiki,
            "create_page",
            json!({ "path": "notes", "content": "# N\n" }),
        );
        let events = drain(&rx, Duration::from_millis(50));
        assert_eq!(short(&events), ["page_created notes", "index_updated"]);
        let version =
            call(&wiki, "get_page", json!({ "page": "notes" }))["result"]["version"].clone();
        assert_eq!(
            json!(events[0].version),
            version,
            "events carry the new Version"
        );
        assert!(created["result"].is_object());

        call(
            &wiki,
            "move_page",
            json!({ "from": "eng/rust", "to": "eng/rs", "dry_run": true }),
        );
        call(
            &wiki,
            "delete_page",
            json!({ "page": "notes", "dry_run": true }),
        );
        assert_eq!(
            drain(&rx, Duration::from_millis(50)),
            [],
            "dry runs change nothing"
        );

        call(
            &wiki,
            "move_page",
            json!({ "from": "eng/rust", "to": "eng/rs" }),
        );
        assert_eq!(
            short(&drain(&rx, Duration::from_millis(50))),
            [
                "page_moved eng/rust -> eng/rs",
                "page_modified eng",
                "index_updated"
            ],
            "a real move, plus the Page whose Link was rewritten"
        );

        call(&wiki, "delete_page", json!({ "page": "notes" }));
        let events = drain(&rx, Duration::from_millis(50));
        assert_eq!(
            events[0],
            ev(EventKind::PageDeleted, "notes"),
            "no version once gone"
        );

        call(
            &wiki,
            "add_attachment",
            json!({ "page": "eng", "name": "a.bin", "source": { "base64": "AAE=" } }),
        );
        assert_eq!(
            short(&drain(&rx, Duration::from_millis(50))),
            ["attachment_changed eng/a.bin", "index_updated"]
        );

        for scope in ["wiki", "machine"] {
            call(
                &wiki,
                "set_config",
                json!({ "key": "links.syntax", "value": "wikilink", "scope": scope }),
            );
        }
        assert_eq!(
            drain(&rx, Duration::from_millis(50)),
            [],
            "settings files aren't Wiki files"
        );
    }

    #[test]
    fn subscribers_only_hear_their_scope() {
        let (_dir, wiki) = wiki();
        let eng = wiki.watch(Scope {
            space: Some("eng".into()),
            path_prefix: None,
        });
        let prefix = wiki.watch(Scope {
            space: None,
            path_prefix: Some("eng/r".into()),
        });
        call(&wiki, "create_page", json!({ "path": "engineering" }));
        call(&wiki, "create_page", json!({ "path": "eng/go" }));
        call(
            &wiki,
            "move_page",
            json!({ "from": "eng/rust", "to": "rust" }),
        );
        assert_eq!(
            short(&drain(&eng, Duration::from_millis(50))),
            [
                "page_created eng/go",
                "index_updated",
                "page_moved eng/rust -> rust",
                "page_modified eng",
                "index_updated"
            ],
            "a move out of the scope is still heard"
        );
        assert_eq!(
            short(&drain(&prefix, Duration::from_millis(50))),
            ["page_moved eng/rust -> rust", "index_updated"]
        );
    }

    #[test]
    fn the_watcher_reports_external_changes_and_updates_the_index() {
        let (_dir, wiki) = wiki();
        assert_eq!(wiki.watcher_mode(), WatcherMode::None);
        let rx = wiki.watch(Scope::default());
        wiki.start_watcher().unwrap();
        wiki.start_watcher().unwrap(); // idempotent
        assert_eq!(wiki.watcher_mode(), WatcherMode::Native);
        let status = call(&wiki, "index_status", json!({}));
        assert_eq!(status["result"]["watcher"], "native");
        let root = wiki.root().to_path_buf();

        let started = Instant::now();
        std::fs::write(root.join("fresh.md"), "# Fresh\n\nzebra\n").unwrap();
        let events = next_batch(&rx, Duration::from_secs(1));
        assert!(started.elapsed() < Duration::from_secs(1) + Duration::from_millis(700));
        assert_eq!(short(&events), ["page_created fresh", "index_updated"]);
        let found = call(&wiki, "search", json!({ "text": "zebra" }));
        assert_eq!(found["result"]["total"], 1, "the Index took it in: {found}");

        std::fs::write(root.join("fresh.md"), "# Fresh\n\nokapi\n").unwrap();
        assert_eq!(
            short(&next_batch(&rx, Duration::from_secs(2))),
            ["page_modified fresh", "index_updated"]
        );

        std::fs::rename(root.join("fresh.md"), root.join("renamed.md")).unwrap();
        let mut events = short(&next_batch(&rx, Duration::from_secs(2)));
        events.sort();
        assert_eq!(
            events,
            [
                "index_updated",
                "page_created renamed",
                "page_deleted fresh"
            ],
            "a file moved outside wikirs: a delete and a create"
        );

        std::fs::rename(root.join("eng"), root.join("dept")).unwrap();
        let mut events = short(&next_batch(&rx, Duration::from_secs(2)));
        events.sort();
        assert_eq!(
            events,
            [
                "index_updated",
                "page_created dept/rust",
                "page_deleted eng/rust"
            ],
            "a folder moved outside wikirs: a delete and a create per file"
        );

        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/HEAD"), "x").unwrap();
        std::fs::write(root.join(".fresh.md.swp"), "x").unwrap();
        assert_eq!(
            drain(&rx, Duration::from_millis(800)),
            [],
            "hidden files are ignored"
        );
    }

    #[test]
    fn a_process_never_hears_its_own_writes_twice() {
        let (_dir, wiki) = wiki();
        let rx = wiki.watch(Scope::default());
        wiki.start_watcher().unwrap();
        call(&wiki, "create_page", json!({ "path": "mine" }));
        call(
            &wiki,
            "write_page",
            json!({ "page": "eng/rust", "content": "# Rust 2\n" }),
        );
        assert_eq!(
            short(&drain(&rx, Duration::from_millis(1000))),
            [
                "page_created mine",
                "index_updated",
                "page_modified eng/rust",
                "index_updated"
            ]
        );
    }

    #[test]
    fn another_processes_writes_reach_this_one() {
        // The other handle updates the shared Index first; this handle must still hear it.
        let (dir, wiki) = wiki();
        let rx = wiki.watch(Scope::default());
        wiki.start_watcher().unwrap();
        let other = Wiki::open_isolated(wiki.root(), dir.path().join("base")).unwrap();
        call(&other, "create_page", json!({ "path": "theirs" }));
        assert_eq!(
            short(&next_batch(&rx, Duration::from_secs(1))),
            ["page_created theirs", "index_updated"]
        );
    }

    #[test]
    fn batches_find_what_the_backend_left_out() {
        // No watcher: batches are handed in directly, as the debouncer would.
        let (_dir, wiki) = wiki();
        let rx = wiki.watch(Scope::default());
        *wiki.hub().seen() = Some(wiki.index().versions().unwrap());
        let root = wiki.root().to_path_buf();
        let take_in = |paths: &[&str], rescan, sweep| {
            let paths = paths.iter().map(|p| root.join(p)).collect();
            wiki.take_in(&Batch {
                paths,
                rescan,
                sweep,
            });
            short(&drain(&rx, Duration::from_millis(50)))
        };

        std::fs::write(root.join("eng/rust.md"), "# Rust, changed\n").unwrap();
        std::fs::remove_file(root.join("eng.md")).unwrap();
        assert_eq!(
            take_in(&[], false, false),
            [] as [&str; 0],
            "nothing named, nothing checked"
        );
        assert_eq!(
            take_in(&[], true, false),
            [
                "page_deleted eng",
                "page_modified eng/rust",
                "index_updated"
            ],
            "a rescan compares every file"
        );

        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/deep.md"), "# Deep\n").unwrap();
        std::fs::write(root.join("a/b/.hidden.md"), "x").unwrap();
        assert_eq!(
            take_in(&["a"], false, false),
            ["page_created a/b/deep", "index_updated"],
            "a new folder's files, hidden ones aside"
        );

        std::fs::rename(root.join("a"), root.join("z")).unwrap();
        assert_eq!(
            take_in(&["z"], false, false),
            ["page_created z/b/deep", "index_updated"],
            "only the new path named, and no sweep: the old file is missed"
        );
        assert_eq!(
            take_in(&[], false, true),
            ["page_deleted a/b/deep", "index_updated"],
            "a sweep finds known files that are gone"
        );

        std::fs::remove_dir_all(root.join("z")).unwrap();
        assert_eq!(
            take_in(&["z"], false, false),
            ["page_deleted z/b/deep", "index_updated"],
            "a removed folder: every file this process knew under it"
        );
        let status = call(&wiki, "list_pages", json!({}));
        assert_eq!(status["result"]["total"], 1, "the Index followed: {status}");
    }

    #[test]
    fn the_poll_setting_selects_the_poll_watcher() {
        let (_dir, wiki) = wiki();
        call(
            &wiki,
            "set_config",
            json!({ "key": "watcher", "value": "poll", "scope": "machine" }),
        );
        let rx = wiki.watch(Scope::default());
        wiki.start_watcher().unwrap();
        assert_eq!(wiki.watcher_mode(), WatcherMode::Poll);
        let status = call(&wiki, "index_status", json!({}));
        assert_eq!(status["result"]["watcher"], "poll");
        // mtime granularity can hide a write in the same instant as the first poll.
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(wiki.root().join("polled.md"), "# Polled\n").unwrap();
        assert_eq!(
            short(&next_batch(&rx, Duration::from_secs(5))),
            ["page_created polled", "index_updated"]
        );
    }

    #[test]
    fn watch_is_a_subscription_not_a_call() {
        let (_dir, wiki) = wiki();
        let err = find("watch").unwrap().call(&wiki, json!({})).unwrap_err();
        assert_eq!(err.kind, crate::ErrorKind::InvalidInput);
        let rx = Watch::subscribe(&wiki, WatchInput::default()).unwrap();
        assert_eq!(
            wiki.watcher_mode(),
            WatcherMode::Native,
            "subscribing starts the watcher"
        );
        std::fs::write(wiki.root().join("x.md"), "# X\n").unwrap();
        assert_eq!(
            short(&next_batch(&rx, Duration::from_secs(1)))[0],
            "page_created x"
        );
    }
}
