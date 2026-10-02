//! The open Page, shared by the GUI and TUI (process-model.md#open-page-changed-on-disk-gui-and-tui).
//!
//! A [`Session`] holds the Page tree, the open Page (its buffer, the `version`
//! it's based on, and what's derived from it), and a [`Notice`] for the UI to
//! show. Saves always send `base_version`, so a stale save becomes
//! [`Notice::ChangedOnDisk`] and never overwrites the file. Watch events
//! reload a clean Page, follow a wikirs move, and mark a deleted one.
//!
//! Everything here is synchronous and UI-free: a UI calls these methods from its
//! event loop and draws the state.

use std::{
    collections::BTreeSet, collections::VecDeque, fmt::Write, path::PathBuf, sync::mpsc::Receiver,
};

use serde_json::Value;
use wikirs_core::{
    Error, ErrorKind, OpInfo, Operation, Wiki,
    document::{Document, document},
    find,
    hierarchy::{ChildNode, Children, ChildrenInput, NodeKind},
    index::{Filter, Scope, Sort},
    links::{LinkStatus, Resolved, TargetKind},
    markdown,
    ops::{
        Backlinks, BacklinksInput, CreatePage, CreatePageInput, GetPage, GetPageInput, ListPages,
        ListPagesInput, ResolveLink, ResolveLinkInput, Search, SearchInput, WritePage,
        WritePageInput,
    },
    watch::{EventKind, Watch, WatchEvent, WatchInput},
};

/// Watch events kept for showing `watch` in a UI.
const EVENT_LOG: usize = 200;

/// One row of the Page tree, depth first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub path: String,
    pub title: String,
    pub placeholder: bool,
    pub depth: usize,
}

/// The Page on screen.
#[derive(Debug, Clone)]
pub struct OpenPage {
    pub path: String,
    pub title: String,
    /// The text as edited (maybe unsaved).
    pub buffer: String,
    /// The text as last read or saved.
    pub saved: String,
    /// The `version` the buffer is based on; `None` once deleted.
    pub version: Option<String>,
    /// Deleted (or moved outside wikirs) while open: saving re-creates it.
    pub deleted: bool,
    pub doc: Document,
    /// Each of `doc.links`, resolved by the core.
    pub resolved: Vec<Option<Resolved>>,
    /// Page Paths linking here.
    pub backlinks: Vec<String>,
    pub tags: Vec<String>,
    /// `(level, text)` per heading.
    pub outline: Vec<(u8, String)>,
}

impl OpenPage {
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.buffer != self.saved
    }
}

/// What the UI should tell the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Info(String),
    Error(String),
    /// A Broken Link or Placeholder was followed: offer to create this Page.
    Create(String),
    /// The open Page changed on disk while it has unsaved edits: offer Reload,
    /// Keep mine and Compare.
    ChangedOnDisk {
        disk: String,
        version: String,
    },
}

/// What following a Link did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Followed {
    /// Opened a Page; scroll to this heading if given.
    Opened { heading: Option<String> },
    /// An Attachment: open this file in the system viewer.
    OpenExternal(PathBuf),
    /// Nothing opened; [`Session::notice`] says why.
    Stayed,
}

pub struct Session {
    wiki: Wiki,
    events: Option<Receiver<WatchEvent>>,
    pub tree: Vec<TreeRow>,
    pub page: Option<OpenPage>,
    pub notice: Option<Notice>,
    pub event_log: VecDeque<String>,
}

impl Session {
    /// A session on `wiki`, showing the first Page in the tree. Subscribes to
    /// the Wiki's own mutations; [`Session::start_watcher`] adds every other writer.
    #[must_use]
    pub fn new(wiki: Wiki) -> Self {
        let events = Some(wiki.watch(Scope::default()));
        let mut session = Self {
            wiki,
            events,
            tree: Vec::new(),
            page: None,
            notice: None,
            event_log: VecDeque::new(),
        };
        session.refresh_tree();
        if let Some(first) = session.tree.iter().find(|r| !r.placeholder) {
            let first = first.path.clone();
            session.open(&first);
        }
        session
    }

    #[must_use]
    pub fn wiki(&self) -> &Wiki {
        &self.wiki
    }

    /// Watches the files for other writers' changes (process-model.md).
    pub fn start_watcher(&mut self) -> Result<(), Error> {
        self.events = Some(Watch::subscribe(
            &self.wiki,
            WatchInput {
                scope: Scope::default(),
            },
        )?);
        Ok(())
    }

    /// Whether the open Page has unsaved edits.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.page.as_ref().is_some_and(OpenPage::dirty)
    }

    // ------------------------------------------------------------- loading

    pub fn refresh_tree(&mut self) {
        fn flatten(nodes: Vec<ChildNode>, depth: usize, out: &mut Vec<TreeRow>) {
            for node in nodes {
                out.push(TreeRow {
                    path: node.path,
                    title: node.title,
                    placeholder: node.kind == NodeKind::Placeholder,
                    depth,
                });
                flatten(node.children.unwrap_or_default(), depth + 1, out);
            }
        }
        let input = ChildrenInput {
            parent: None,
            depth: u32::MAX,
        };
        match Children::run(&self.wiki, input) {
            Ok(out) => {
                self.tree.clear();
                flatten(out.children, 0, &mut self.tree);
            }
            Err(err) => self.error(&err),
        }
    }

    /// Opens a Page, and says whether it did. Refuses while the open one has
    /// unsaved edits; a missing Page becomes [`Notice::Create`].
    pub fn open(&mut self, path: &str) -> bool {
        if let Some(page) = &self.page
            && page.dirty()
            && page.path != path
        {
            self.notice = Some(Notice::Info(format!(
                "Unsaved edits to `{}`: save or discard them first",
                page.path
            )));
            return false;
        }
        match self.get(path) {
            Ok(got) => {
                self.page = Some(OpenPage {
                    path: got.path,
                    title: got.title,
                    buffer: got.content.clone(),
                    saved: got.content,
                    version: Some(got.version),
                    deleted: false,
                    doc: Document::default(),
                    resolved: Vec::new(),
                    backlinks: Vec::new(),
                    tags: Vec::new(),
                    outline: Vec::new(),
                });
                self.notice = None;
                self.derive();
                true
            }
            Err(err) if err.kind == ErrorKind::NotFound => {
                self.notice = Some(Notice::Create(path.to_string()));
                false
            }
            Err(err) => {
                self.error(&err);
                false
            }
        }
    }

    fn get(&self, path: &str) -> Result<wikirs_core::ops::GetPageOutput, Error> {
        GetPage::run(
            &self.wiki,
            GetPageInput {
                page: path.to_string(),
            },
        )
    }

    /// Replaces the buffer (an edit), recomputing what's derived from it.
    pub fn set_buffer(&mut self, text: String) {
        if let Some(page) = self.page.as_mut()
            && page.buffer != text
        {
            page.buffer = text;
            self.derive();
        }
    }

    /// Drops unsaved edits.
    pub fn discard(&mut self) {
        if let Some(page) = self.page.as_mut() {
            page.buffer = page.saved.clone();
            self.notice = Some(Notice::Info("Edits discarded".into()));
            self.derive();
        }
    }

    /// Recomputes what's derived from the buffer and the Index: the document,
    /// Link statuses, Backlinks, Tags and outline.
    pub fn derive(&mut self) {
        let Some(page) = self.page.as_mut() else {
            return;
        };
        page.doc = document(&page.buffer);
        page.resolved = page
            .doc
            .links
            .iter()
            .map(|link| {
                ResolveLink::run(
                    &self.wiki,
                    ResolveLinkInput {
                        from_page: page.path.clone(),
                        raw: link.raw.clone(),
                    },
                )
                .ok()
            })
            .collect();
        page.backlinks = Backlinks::run(
            &self.wiki,
            BacklinksInput {
                target: page.path.clone(),
            },
        )
        .map(|out| {
            let from: BTreeSet<String> = out.backlinks.into_iter().map(|b| b.from).collect();
            from.into_iter().collect()
        })
        .unwrap_or_default();
        let parsed = markdown::parse(&page.buffer);
        let tags: BTreeSet<String> = parsed.tags.iter().map(|t| t.tag.to_lowercase()).collect();
        page.tags = tags.into_iter().collect();
        page.outline = parsed
            .headings
            .iter()
            .map(|h| (h.level, h.text.clone()))
            .collect();
    }

    fn error(&mut self, err: &Error) {
        self.notice = Some(Notice::Error(err.message.clone()));
    }

    // -------------------------------------------------------------- saving

    /// Saves the buffer with `base_version`: a stale save gets `Conflict` and
    /// becomes [`Notice::ChangedOnDisk`], never an overwrite. A deleted Page
    /// is re-created.
    pub fn save(&mut self) {
        let Some(page) = self.page.as_ref() else {
            return;
        };
        let (path, content) = (page.path.clone(), page.buffer.clone());
        if page.deleted {
            let created = CreatePage::run(
                &self.wiki,
                CreatePageInput {
                    path: Some(path.clone()),
                    parent: None,
                    title: None,
                    content: Some(content.clone()),
                    dry_run: false,
                },
            );
            match created {
                Ok(_) => self.saved(&path, content, None),
                Err(err) => self.error(&err),
            }
            self.pump();
            return;
        }
        match WritePage::run(
            &self.wiki,
            WritePageInput {
                page: path.clone(),
                content: content.clone(),
                base_version: page.version.clone(),
                dry_run: false,
            },
        ) {
            Ok(out) => self.saved(&path, content, Some(out.version)),
            Err(err) if err.kind == ErrorKind::Conflict => self.conflict(&path),
            Err(err) => self.error(&err),
        }
        self.pump();
    }

    fn saved(&mut self, path: &str, content: String, version: Option<String>) {
        let version = version.or_else(|| self.get(path).ok().map(|p| p.version));
        if let Some(page) = self.page.as_mut() {
            page.saved = content;
            page.version = version;
            page.deleted = false;
        }
        self.notice = Some(Notice::Info(format!("Saved `{path}`")));
        self.derive();
    }

    /// The save was refused: show what's on disk now.
    fn conflict(&mut self, path: &str) {
        match self.get(path) {
            Ok(disk) => {
                self.notice = Some(Notice::ChangedOnDisk {
                    disk: disk.content,
                    version: disk.version,
                });
            }
            Err(err) => self.error(&err),
        }
    }

    /// Changed on disk → Reload: take the disk's text, dropping the buffer's.
    pub fn reload_from_disk(&mut self) {
        let Some(Notice::ChangedOnDisk { disk, version }) = self.notice.take() else {
            return;
        };
        if let Some(page) = self.page.as_mut() {
            page.buffer.clone_from(&disk);
            page.saved = disk;
            page.version = Some(version);
        }
        self.derive();
    }

    /// Changed on disk → Keep mine: write the buffer over what's on disk now
    /// (still checked against that version).
    pub fn keep_mine(&mut self) {
        let Some(Notice::ChangedOnDisk { version, .. }) = self.notice.take() else {
            return;
        };
        if let Some(page) = self.page.as_mut() {
            page.version = Some(version);
        }
        self.save();
    }

    // --------------------------------------------------------- watch events

    /// Takes in pending watch events, and says whether there were any. Call once
    /// per turn of the UI's event loop.
    pub fn pump(&mut self) -> bool {
        let events: Vec<WatchEvent> = self
            .events
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        if events.is_empty() {
            return false;
        }
        for event in &events {
            self.on_watch(event);
        }
        self.refresh_tree();
        self.derive();
        true
    }

    /// One change event (process-model.md#open-page-changed-on-disk-gui-and-tui).
    pub fn on_watch(&mut self, event: &WatchEvent) {
        self.log_event(event);
        let Some(page) = self.page.as_mut() else {
            return;
        };
        let here = event.path.as_deref() == Some(page.path.as_str());
        match event.kind {
            EventKind::PageModified | EventKind::PageCreated if here => {
                if event.version.is_some() && event.version == page.version {
                    return; // our own save, or no change
                }
                let path = page.path.clone();
                let Ok(disk) = GetPage::run(&self.wiki, GetPageInput { page: path.clone() }) else {
                    return;
                };
                if Some(&disk.version) == page.version.as_ref() {
                    return;
                }
                if page.dirty() {
                    self.notice = Some(Notice::ChangedOnDisk {
                        disk: disk.content,
                        version: disk.version,
                    });
                } else {
                    page.buffer.clone_from(&disk.content);
                    page.saved = disk.content;
                    page.version = Some(disk.version);
                    page.deleted = false;
                    page.title = disk.title;
                    self.notice = Some(Notice::Info(format!("`{path}` changed on disk: reloaded")));
                }
            }
            EventKind::PageDeleted if here => {
                page.deleted = true;
                page.version = None;
                self.notice = Some(Notice::Info(format!(
                    "`{}` was deleted: saving re-creates it",
                    page.path
                )));
            }
            EventKind::PageMoved if event.from.as_deref() == Some(page.path.as_str()) => {
                let Some(to) = event.path.clone() else {
                    return;
                };
                let from = std::mem::replace(&mut page.path, to.clone());
                // The move may rewrite the Page's own relative Links.
                if !page.dirty()
                    && let Ok(disk) = GetPage::run(&self.wiki, GetPageInput { page: to.clone() })
                {
                    page.buffer.clone_from(&disk.content);
                    page.saved = disk.content;
                    page.version = Some(disk.version);
                } else {
                    page.version.clone_from(&event.version);
                }
                self.notice = Some(Notice::Info(format!("Moved `{from}` → `{to}`")));
            }
            _ => {}
        }
    }

    fn log_event(&mut self, event: &WatchEvent) {
        let mut line = serde_json::to_value(event.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        if let Some(from) = &event.from {
            let _ = write!(line, " {from} →");
        }
        if let Some(path) = &event.path {
            let _ = write!(line, " {path}");
        }
        self.event_log.push_back(line);
        while self.event_log.len() > EVENT_LOG {
            self.event_log.pop_front();
        }
    }

    // ---------------------------------------------------------- navigation

    /// Follows Link `i` of the open Page.
    pub fn follow(&mut self, i: usize) -> Followed {
        let Some(page) = &self.page else {
            return Followed::Stayed;
        };
        let Some(link) = page.doc.links.get(i) else {
            return Followed::Stayed;
        };
        let (raw, embed) = (link.raw.clone(), link.embed);
        match page.resolved.get(i).cloned().flatten() {
            None => self.notice = Some(Notice::Error(format!("Can't resolve {raw}"))),
            // A broken embed names a missing file, never a Page to create.
            Some(r) if r.status == LinkStatus::Broken => {
                self.notice = Some(if embed || r.target_kind == TargetKind::Attachment {
                    Notice::Error(format!("No Attachment `{}`", r.target))
                } else {
                    Notice::Create(r.target)
                });
            }
            Some(r) if r.target_kind == TargetKind::Attachment => {
                return Followed::OpenExternal(
                    r.target
                        .split('/')
                        .fold(self.wiki.root().to_path_buf(), |p, seg| p.join(seg)),
                );
            }
            Some(r) => {
                if self.open(&r.target) {
                    return Followed::Opened { heading: r.heading };
                }
            }
        }
        Followed::Stayed
    }

    /// Creates an empty Page at `path` and opens it.
    pub fn create(&mut self, path: &str) {
        let created = CreatePage::run(
            &self.wiki,
            CreatePageInput {
                path: Some(path.to_string()),
                parent: None,
                title: None,
                content: None,
                dry_run: false,
            },
        );
        self.pump();
        match created {
            Ok(out) => {
                self.refresh_tree();
                self.open(&out.path);
                self.notice = Some(Notice::Info(format!("Created `{}`", out.path)));
            }
            Err(err) => self.error(&err),
        }
    }

    /// The tree row's Title for `path`, else the path.
    #[must_use]
    pub fn title_of(&self, path: &str) -> String {
        self.tree
            .iter()
            .find(|r| r.path == path)
            .map_or_else(|| path.to_string(), |r| r.title.clone())
    }

    /// The open Page's breadcrumb: the Title of each prefix of its path.
    #[must_use]
    pub fn breadcrumb(&self) -> Vec<String> {
        let Some(page) = &self.page else {
            return Vec::new();
        };
        let mut acc = String::new();
        page.path
            .split('/')
            .map(|seg| {
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(seg);
                self.tree
                    .iter()
                    .find(|r| r.path == acc)
                    .map_or_else(|| seg.to_string(), |r| r.title.clone())
            })
            .collect()
    }

    /// Every Page as `(path, title)`, for quick open.
    #[must_use]
    pub fn all_pages(&self) -> Vec<(String, String)> {
        ListPages::run(
            &self.wiki,
            ListPagesInput {
                filter: Filter::default(),
                sort: Sort::default(),
                limit: Some(u32::MAX),
                offset: None,
            },
        )
        .map(|out| out.pages.into_iter().map(|p| (p.path, p.title)).collect())
        .unwrap_or_default()
    }

    /// The Pages carrying `tag` or one of its descendants, as `(path, title)`.
    #[must_use]
    pub fn pages_with_tag(&self, tag: &str) -> Vec<(String, String)> {
        ListPages::run(
            &self.wiki,
            ListPagesInput {
                filter: Filter {
                    tag: Some(tag.to_string()),
                    ..Filter::default()
                },
                sort: Sort::default(),
                limit: Some(u32::MAX),
                offset: None,
            },
        )
        .map(|out| out.pages.into_iter().map(|p| (p.path, p.title)).collect())
        .unwrap_or_default()
    }

    /// Full-text search: `(path, title, snippet)`; none for a blank query.
    #[must_use]
    pub fn search(&self, text: &str) -> Vec<(String, String, String)> {
        Search::run(
            &self.wiki,
            SearchInput {
                text: text.to_string(),
                filter: Filter::default(),
                limit: Some(50),
                offset: None,
            },
        )
        .map(|out| {
            out.hits
                .into_iter()
                .map(|h| (h.path, h.title, h.snippet.replace('\n', " ")))
                .collect()
        })
        .unwrap_or_default()
    }

    // ------------------------------------------------------------ Operations

    /// The palette's Operations whose names contain `query`.
    #[must_use]
    pub fn palette_matches(query: &str) -> Vec<OpInfo> {
        let query = query.trim().replace([' ', '-'], "_");
        wikirs_core::registry()
            .into_iter()
            .filter(|op| op.name.contains(&query))
            .collect()
    }

    /// Runs an Operation with JSON Input, then takes in what it changed.
    pub fn run_json(&mut self, op: &str, input: Value) -> Result<Value, Error> {
        let outcome = match find(op) {
            Some(info) => info.call(&self.wiki, input),
            None => Err(Error::invalid_input(None, format!("no Operation `{op}`"))),
        };
        self.after_op();
        outcome
    }

    /// Takes in the effects of an Operation run some other way.
    pub fn after_op(&mut self) {
        self.pump();
        self.refresh_tree();
        self.derive();
    }
}
