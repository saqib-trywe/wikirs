//! The TUI's state and behaviour (docs/spec/tui.md): keys and watch events in,
//! Operations called in-process, state out for [`crate::ui`] to draw.
//!
//! Nothing here touches the terminal, so tests drive an [`App`] with keys and
//! draw it to a `TestBackend`. The one thing that needs the terminal, the
//! `$EDITOR` hand-off, is asked for through [`App::take_request`].

use std::{
    collections::{BTreeSet, VecDeque},
    fmt::Write,
    path::PathBuf,
    sync::mpsc::Receiver,
};

use clap::Parser;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui_textarea::TextArea;
use serde_json::{Value, json};
use wikirs_core::{
    Command, Error, ErrorKind, Kind, Operation, Wiki,
    document::{Document, document},
    find,
    hierarchy::{ChildNode, Children, ChildrenInput, NodeKind},
    index::Scope,
    links::{LinkStatus, Resolved, TargetKind},
    markdown,
    ops::{
        Backlinks, BacklinksInput, CreatePage, CreatePageInput, GetPage, GetPageInput, ListPages,
        ListPagesInput, ResolveLink, ResolveLinkInput, Search, SearchInput, WritePage,
        WritePageInput,
    },
    watch::{EventKind, Watch, WatchEvent, WatchInput},
};
use wikirs_forms::Form;

use crate::{
    form::{FormAction, FormView},
    render, result,
};

/// Built-in `:` commands besides the Operations.
const BUILTINS: [&str; 3] = ["q", "w", "wq"];
/// Events kept for `watch` run from the palette or `:`.
const EVENT_LOG: usize = 200;

/// Something only the terminal loop can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Suspend the TUI and edit this text in `$EDITOR`; answer with
    /// [`App::editor_returned`].
    Editor(String),
    /// Open a file in the system viewer.
    OpenExternal(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Links,
}

pub struct TreeRow {
    pub path: String,
    pub title: String,
    pub placeholder: bool,
    pub depth: usize,
}

/// The Page on screen.
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
    pub backlinks: Vec<String>,
    pub tags: Vec<String>,
    pub outline: Vec<(u8, String)>,
    pub link_sel: usize,
    pub scroll: u16,
}

impl OpenPage {
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.buffer != self.saved
    }
}

pub enum Banner {
    Info(String),
    Error(String),
    /// A Broken Link or Placeholder was followed: `c` creates this Page.
    Create(String),
    /// The open Page changed on disk while it has unsaved edits.
    ChangedOnDisk {
        disk: String,
        version: String,
        compare: bool,
    },
}

/// How to apply a dry run from the result popup.
pub enum Apply {
    Input { op: String, input: Value },
    Line(String),
}

pub struct ResultPopup {
    pub title: String,
    pub lines: Vec<ratatui::text::Line<'static>>,
    pub scroll: u16,
    pub apply: Option<Apply>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    Open,
    Search,
    Backlinks,
}

pub struct Picker {
    pub kind: PickKind,
    pub query: String,
    pub sel: usize,
    /// `(path, label, detail)`.
    pub items: Vec<(String, String, String)>,
    /// Quick open's full list, filtered as you type.
    all: Vec<(String, String, String)>,
}

pub enum Mode {
    View,
    Edit(Box<TextArea<'static>>),
    Command { line: String, hint: Option<String> },
    Palette { query: String, sel: usize },
    Form(Box<FormView>),
    Pick(Picker),
    Result(ResultPopup),
}

pub struct App {
    pub(crate) wiki: Wiki,
    events: Option<Receiver<WatchEvent>>,
    pub tree: Vec<TreeRow>,
    pub tree_sel: usize,
    pub focus: Focus,
    pub page: Option<OpenPage>,
    pub mode: Mode,
    pub banner: Option<Banner>,
    /// Digits typed so far towards a numbered Link.
    pub digits: String,
    pub event_log: VecDeque<String>,
    quit: bool,
    request: Option<Request>,
}

impl App {
    /// A TUI on `wiki`, showing the first Page in the tree. Subscribes to the
    /// Wiki's own mutations; [`App::start_watcher`] adds every other writer.
    #[must_use]
    pub fn new(wiki: Wiki) -> Self {
        let events = Some(wiki.watch(Scope::default()));
        let mut app = Self {
            wiki,
            events,
            tree: Vec::new(),
            tree_sel: 0,
            focus: Focus::Tree,
            page: None,
            mode: Mode::View,
            banner: None,
            digits: String::new(),
            event_log: VecDeque::new(),
            quit: false,
            request: None,
        };
        app.refresh_tree();
        if let Some(first) = app.tree.iter().find(|r| !r.placeholder) {
            let first = first.path.clone();
            app.open(&first);
        }
        app
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

    #[must_use]
    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn take_request(&mut self) -> Option<Request> {
        self.request.take()
    }

    /// The text back from `$EDITOR` goes into the buffer, unsaved.
    pub fn editor_returned(&mut self, outcome: std::io::Result<String>) {
        match (outcome, self.page.as_mut()) {
            (Ok(text), Some(page)) => {
                let changed = text != page.buffer;
                page.buffer = text;
                self.mode = Mode::View;
                self.derive();
                self.banner = Some(Banner::Info(if changed {
                    "Back from the editor: ctrl-s saves".into()
                } else {
                    "Back from the editor: no changes".into()
                }));
            }
            (Err(err), _) => self.banner = Some(Banner::Error(format!("editor: {err}"))),
            (Ok(_), None) => {}
        }
    }

    // ------------------------------------------------------------- loading

    fn refresh_tree(&mut self) {
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
        if let Some(page) = &self.page
            && let Some(i) = self.tree.iter().position(|r| r.path == page.path)
        {
            self.tree_sel = i;
        }
        self.tree_sel = self.tree_sel.min(self.tree.len().saturating_sub(1));
    }

    /// Opens a Page. Refuses while the open one has unsaved edits.
    pub fn open(&mut self, path: &str) {
        if let Some(page) = &self.page
            && page.dirty()
            && page.path != path
        {
            self.banner = Some(Banner::Info(format!(
                "Unsaved edits to `{}`: ctrl-s saves, D discards them",
                page.path
            )));
            return;
        }
        match GetPage::run(
            &self.wiki,
            GetPageInput {
                page: path.to_string(),
            },
        ) {
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
                    link_sel: 0,
                    scroll: 0,
                });
                self.banner = None;
                self.derive();
                if let Some(i) = self.tree.iter().position(|r| r.path == path) {
                    self.tree_sel = i;
                }
            }
            Err(err) if err.kind == ErrorKind::NotFound => {
                self.banner = Some(Banner::Create(path.to_string()));
            }
            Err(err) => self.error(&err),
        }
    }

    /// Recomputes what's derived from the buffer and the Index: the document,
    /// Link statuses, Backlinks, Tags and outline.
    fn derive(&mut self) {
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
        page.link_sel = page.link_sel.min(page.doc.links.len().saturating_sub(1));
    }

    fn error(&mut self, err: &Error) {
        self.banner = Some(Banner::Error(err.message.clone()));
    }

    // -------------------------------------------------------------- saving

    /// Saves the buffer with `base_version` (process-model.md): a stale save
    /// gets `Conflict` and the changed-on-disk banner, never an overwrite.
    pub fn save(&mut self) {
        self.sync_edit();
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
        let base_version = page.version.clone();
        match WritePage::run(
            &self.wiki,
            WritePageInput {
                page: path.clone(),
                content: content.clone(),
                base_version,
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
        let version = version.or_else(|| {
            GetPage::run(
                &self.wiki,
                GetPageInput {
                    page: path.to_string(),
                },
            )
            .ok()
            .map(|p| p.version)
        });
        if let Some(page) = self.page.as_mut() {
            page.saved = content;
            page.version = version;
            page.deleted = false;
        }
        self.banner = Some(Banner::Info(format!("Saved `{path}`")));
        self.derive();
    }

    /// The save was refused: show what's on disk now.
    fn conflict(&mut self, path: &str) {
        match GetPage::run(
            &self.wiki,
            GetPageInput {
                page: path.to_string(),
            },
        ) {
            Ok(disk) => {
                self.banner = Some(Banner::ChangedOnDisk {
                    disk: disk.content,
                    version: disk.version,
                    compare: false,
                });
            }
            Err(err) => self.error(&err),
        }
    }

    /// The textarea's text into the buffer.
    fn sync_edit(&mut self) {
        if let (Mode::Edit(area), Some(page)) = (&self.mode, self.page.as_mut()) {
            let mut text = area.lines().join("\n");
            // A textarea has no final newline; keep the file's.
            if page.saved.ends_with('\n') || page.buffer.ends_with('\n') {
                text.push('\n');
            }
            if text != page.buffer {
                page.buffer = text;
                self.derive();
            }
        }
    }

    // --------------------------------------------------------- watch events

    /// Takes in pending watch events (call once per loop turn).
    pub fn pump(&mut self) {
        let events: Vec<WatchEvent> = self
            .events
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        if events.is_empty() {
            return;
        }
        for event in events {
            self.on_watch(&event);
        }
        self.refresh_tree();
        self.derive();
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
                    self.banner = Some(Banner::ChangedOnDisk {
                        disk: disk.content,
                        version: disk.version,
                        compare: false,
                    });
                } else {
                    page.buffer.clone_from(&disk.content);
                    page.saved = disk.content;
                    page.version = Some(disk.version);
                    page.deleted = false;
                    page.title = disk.title;
                    self.banner = Some(Banner::Info(format!("`{path}` changed on disk: reloaded")));
                }
            }
            EventKind::PageDeleted if here => {
                page.deleted = true;
                page.version = None;
                self.banner = Some(Banner::Info(format!(
                    "`{}` was deleted: ctrl-s re-creates it",
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
                self.banner = Some(Banner::Info(format!("Moved `{from}` → `{to}`")));
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

    // ----------------------------------------------------------------- keys

    pub fn key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if ctrl && key.code == KeyCode::Char('s') {
            self.save();
            return;
        }
        match &mut self.mode {
            Mode::View => self.view_key(key),
            Mode::Edit(area) => {
                if key.code == KeyCode::Esc {
                    self.sync_edit();
                    self.mode = Mode::View;
                } else {
                    area.input(key);
                }
            }
            Mode::Command { .. } => self.command_key(key),
            Mode::Palette { .. } => self.palette_key(key),
            Mode::Form(form) => match form.key(key) {
                FormAction::None => {}
                FormAction::Close => {
                    self.mode = Mode::Palette {
                        query: String::new(),
                        sel: 0,
                    }
                }
                FormAction::Submit(input) => {
                    let op = form.form.op.to_string();
                    self.mode = Mode::View;
                    self.run_json(&op, input);
                }
            },
            Mode::Pick(_) => self.pick_key(key),
            Mode::Result(popup) => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.mode = Mode::View,
                KeyCode::Down | KeyCode::Char('j') => popup.scroll = popup.scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => popup.scroll = popup.scroll.saturating_sub(1),
                KeyCode::Char('a') => {
                    if let Some(apply) = popup.apply.take() {
                        self.mode = Mode::View;
                        match apply {
                            Apply::Input { op, input } => self.run_json(&op, input),
                            Apply::Line(line) => self.run_line(&line),
                        }
                    }
                }
                _ => {}
            },
        }
    }

    /// Changed on disk: r / m / d (process-model.md). Whether it took the key.
    fn changed_on_disk_key(&mut self, key: KeyEvent) -> bool {
        if let Some(Banner::ChangedOnDisk {
            disk,
            version,
            compare,
        }) = &mut self.banner
        {
            match key.code {
                KeyCode::Char('r') => {
                    let (disk, version) = (disk.clone(), version.clone());
                    if let Some(page) = self.page.as_mut() {
                        page.buffer.clone_from(&disk);
                        page.saved = disk;
                        page.version = Some(version);
                    }
                    self.banner = None;
                    self.derive();
                    return true;
                }
                KeyCode::Char('m') => {
                    // Keep mine: write over what's on disk now, still checked.
                    let version = version.clone();
                    if let Some(page) = self.page.as_mut() {
                        page.version = Some(version);
                    }
                    self.banner = None;
                    self.save();
                    return true;
                }
                KeyCode::Char('d') => {
                    *compare = !*compare;
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    fn view_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.changed_on_disk_key(key) {
            return;
        }
        if let KeyCode::Char(d @ '0'..='9') = key.code {
            self.digit(d);
            return;
        }
        let pending = !self.digits.is_empty();
        self.digits.clear();
        let links = self.page.as_ref().map_or(0, |p| p.doc.links.len());
        match key.code {
            KeyCode::Enter if pending => {}
            KeyCode::Char('q') => {
                if self.page.as_ref().is_some_and(OpenPage::dirty) {
                    self.banner = Some(Banner::Info(
                        "Unsaved edits: ctrl-s saves, Q quits without saving".into(),
                    ));
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Char('Q') => self.quit = true,
            KeyCode::Char('k') if ctrl => {
                self.mode = Mode::Palette {
                    query: String::new(),
                    sel: 0,
                };
            }
            KeyCode::Char('d') if ctrl => self.scroll(10),
            KeyCode::Char('u') if ctrl => self.scroll(-10),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll(20),
            KeyCode::PageUp => self.scroll(-20),
            KeyCode::Char(':') => {
                self.mode = Mode::Command {
                    line: String::new(),
                    hint: None,
                };
            }
            KeyCode::Char('o') => self.open_picker(PickKind::Open),
            KeyCode::Char('/') => self.open_picker(PickKind::Search),
            KeyCode::Char('b') => self.open_picker(PickKind::Backlinks),
            KeyCode::Char('e') => {
                if let Some(page) = &self.page {
                    let mut area = TextArea::new(page.buffer.lines().map(String::from).collect());
                    area.set_cursor_line_style(ratatui::style::Style::new());
                    self.mode = Mode::Edit(Box::new(area));
                }
            }
            KeyCode::Char('E') => {
                if let Some(page) = &self.page {
                    self.request = Some(Request::Editor(page.buffer.clone()));
                }
            }
            KeyCode::Char('D') => {
                if let Some(page) = self.page.as_mut() {
                    page.buffer = page.saved.clone();
                    self.banner = Some(Banner::Info("Edits discarded".into()));
                    self.derive();
                }
            }
            KeyCode::Char('R') => {
                if let Some(page) = &self.page {
                    let path = page.path.clone();
                    self.open_form("move_page", &json!({ "from": path, "to": path }));
                }
            }
            KeyCode::Char('c') => {
                if let Some(Banner::Create(path)) = &self.banner {
                    let path = path.clone();
                    self.create(&path);
                }
            }
            KeyCode::Char('x') | KeyCode::Esc => {
                if !matches!(self.banner, Some(Banner::ChangedOnDisk { .. })) {
                    self.banner = None;
                }
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Tree if links > 0 => Focus::Links,
                    _ => Focus::Tree,
                };
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Enter => self.enter(),
            _ => {}
        }
    }

    /// Enter: open the tree's selection, or follow the selected Link.
    fn enter(&mut self) {
        match self.focus {
            Focus::Tree => {
                if let Some(row) = self.tree.get(self.tree_sel) {
                    let (path, placeholder) = (row.path.clone(), row.placeholder);
                    if placeholder {
                        self.banner = Some(Banner::Create(path));
                    } else {
                        self.open(&path);
                    }
                }
            }
            Focus::Links => {
                if let Some(i) = self.page.as_ref().map(|p| p.link_sel) {
                    self.follow(i);
                }
            }
        }
    }

    fn move_selection(&mut self, by: isize) {
        match self.focus {
            Focus::Tree => {
                self.tree_sel = self
                    .tree_sel
                    .saturating_add_signed(by)
                    .min(self.tree.len().saturating_sub(1));
            }
            Focus::Links => {
                if let Some(page) = self.page.as_mut() {
                    page.link_sel = page
                        .link_sel
                        .saturating_add_signed(by)
                        .min(page.doc.links.len().saturating_sub(1));
                }
            }
        }
    }

    fn scroll(&mut self, by: i32) {
        if let Some(page) = self.page.as_mut() {
            page.scroll = page
                .scroll
                .saturating_add_signed(i16::try_from(by).unwrap_or(0));
        }
    }

    /// A digit towards a numbered Link: follows as soon as no more digits could
    /// make a different Link, else waits for more (or Enter).
    fn digit(&mut self, d: char) {
        self.digits.push(d);
        let count = self.page.as_ref().map_or(0, |p| p.doc.links.len());
        let n: usize = self.digits.parse().unwrap_or(0);
        if n == 0 || n > count {
            self.banner = Some(Banner::Info(format!("No Link [{}]", self.digits)));
            self.digits.clear();
        } else if n * 10 > count {
            self.digits.clear();
            self.follow(n - 1);
        }
    }

    /// Follows Link `i` of the open Page.
    pub fn follow(&mut self, i: usize) {
        let Some(page) = &self.page else {
            return;
        };
        let Some(link) = page.doc.links.get(i) else {
            return;
        };
        let (raw, embed) = (link.raw.clone(), link.embed);
        match page.resolved.get(i).cloned().flatten() {
            None => self.banner = Some(Banner::Error(format!("Can't resolve {raw}"))),
            // A broken embed names a missing file, never a Page to create.
            Some(r) if r.status == LinkStatus::Broken => {
                if embed || r.target_kind == TargetKind::Attachment {
                    self.banner = Some(Banner::Error(format!("No Attachment `{}`", r.target)));
                } else {
                    self.banner = Some(Banner::Create(r.target));
                }
            }
            Some(r) if r.target_kind == TargetKind::Attachment => {
                let file = r
                    .target
                    .split('/')
                    .fold(self.wiki.root().to_path_buf(), |p, seg| p.join(seg));
                self.request = Some(Request::OpenExternal(file));
            }
            Some(r) => {
                self.open(&r.target);
                if let (Some(heading), Some(page)) = (&r.heading, self.page.as_mut())
                    && page.path == r.target
                {
                    let anchor = markdown::anchor(heading);
                    let rendered = render::render(&page.doc, &page.resolved, None);
                    if let Some((_, line)) = rendered.headings.iter().find(|(a, _)| *a == anchor) {
                        page.scroll = u16::try_from(*line).unwrap_or(u16::MAX);
                    }
                }
            }
        }
    }

    fn create(&mut self, path: &str) {
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
                self.banner = Some(Banner::Info(format!("Created `{}`", out.path)));
            }
            Err(err) => self.error(&err),
        }
    }

    // --------------------------------------------------------------- pickers

    fn open_picker(&mut self, kind: PickKind) {
        let all = match kind {
            PickKind::Open => ListPages::run(
                &self.wiki,
                ListPagesInput {
                    filter: wikirs_core::index::Filter::default(),
                    sort: wikirs_core::index::Sort::default(),
                    limit: Some(u32::MAX),
                    offset: None,
                },
            )
            .map(|out| {
                out.pages
                    .into_iter()
                    .map(|p| (p.path.clone(), p.title, p.path))
                    .collect()
            })
            .unwrap_or_default(),
            PickKind::Backlinks => self.page.as_ref().map_or_else(Vec::new, |p| {
                p.backlinks
                    .iter()
                    .map(|b| (b.clone(), self.title_of(b), b.clone()))
                    .collect()
            }),
            PickKind::Search => Vec::new(),
        };
        self.mode = Mode::Pick(Picker {
            kind,
            query: String::new(),
            sel: 0,
            items: all.clone(),
            all,
        });
    }

    fn title_of(&self, path: &str) -> String {
        self.tree
            .iter()
            .find(|r| r.path == path)
            .map_or_else(|| path.to_string(), |r| r.title.clone())
    }

    fn pick_key(&mut self, key: KeyEvent) {
        let Mode::Pick(picker) = &mut self.mode else {
            return;
        };
        let mut requery = false;
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::View;
                return;
            }
            KeyCode::Down => {
                picker.sel = (picker.sel + 1).min(picker.items.len().saturating_sub(1));
            }
            KeyCode::Up => picker.sel = picker.sel.saturating_sub(1),
            KeyCode::Enter => {
                if let Some((path, _, _)) = picker.items.get(picker.sel).cloned() {
                    self.mode = Mode::View;
                    self.open(&path);
                }
                return;
            }
            KeyCode::Backspace if picker.kind != PickKind::Backlinks => {
                picker.query.pop();
                requery = true;
            }
            KeyCode::Char(c) if picker.kind != PickKind::Backlinks => {
                picker.query.push(c);
                requery = true;
            }
            _ => {}
        }
        if !requery {
            return;
        }
        picker.sel = 0;
        let query = picker.query.to_lowercase();
        picker.items = match picker.kind {
            PickKind::Search if query.trim().is_empty() => Vec::new(),
            PickKind::Search => Search::run(
                &self.wiki,
                SearchInput {
                    text: picker.query.clone(),
                    filter: wikirs_core::index::Filter::default(),
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
            .unwrap_or_default(),
            _ => picker
                .all
                .iter()
                .filter(|(path, title, _)| {
                    path.to_lowercase().contains(&query) || title.to_lowercase().contains(&query)
                })
                .cloned()
                .collect(),
        };
    }

    // ------------------------------------------------------------ Operations

    /// The palette's Operations matching `query`.
    #[must_use]
    pub fn palette_matches(query: &str) -> Vec<wikirs_core::OpInfo> {
        let query = query.trim().replace([' ', '-'], "_");
        wikirs_core::registry()
            .into_iter()
            .filter(|op| op.name.contains(&query))
            .collect()
    }

    fn palette_key(&mut self, key: KeyEvent) {
        let Mode::Palette { query, sel } = &mut self.mode else {
            return;
        };
        let matches = Self::palette_matches(query);
        match key.code {
            KeyCode::Esc => self.mode = Mode::View,
            KeyCode::Down => *sel = (*sel + 1).min(matches.len().saturating_sub(1)),
            KeyCode::Up => *sel = sel.saturating_sub(1),
            KeyCode::Backspace => {
                query.pop();
                *sel = 0;
            }
            KeyCode::Enter => {
                if let Some(op) = matches.get(*sel) {
                    let path = self.page.as_ref().map(|p| p.path.clone());
                    let prefill = path.map_or(
                        Value::Null,
                        |p| json!({ "page": p, "from": p, "target": p, "from_page": p }),
                    );
                    self.open_form(op.name, &prefill);
                }
            }
            KeyCode::Char(c) => {
                query.push(c);
                *sel = 0;
            }
            _ => {}
        }
    }

    /// Opens an Operation's form, filled from `prefill`; a mutation starts as a dry run.
    fn open_form(&mut self, op: &str, prefill: &Value) {
        let Some(Ok(mut form)) = Form::for_op(op) else {
            self.banner = Some(Banner::Error(format!("No form for `{op}`")));
            return;
        };
        form.fill(prefill);
        if form.dry_run.is_some() {
            form.dry_run = Some(true);
        }
        self.mode = Mode::Form(Box::new(FormView::new(form)));
    }

    /// Runs an Operation with JSON Input and shows the outcome.
    pub fn run_json(&mut self, op: &str, input: Value) {
        if op == Watch::NAME {
            self.show_events();
            return;
        }
        let Some(info) = find(op) else {
            self.banner = Some(Banner::Error(format!("No Operation `{op}`")));
            return;
        };
        let outcome = info.call(&self.wiki, input.clone());
        let apply = (info.kind == Kind::Mutation && input["dry_run"] == json!(true)).then(|| {
            let mut input = input;
            input["dry_run"] = json!(false);
            Apply::Input {
                op: op.to_string(),
                input,
            }
        });
        self.show(op, &outcome, apply.filter(|_| outcome.is_ok()));
    }

    fn show(&mut self, title: &str, outcome: &Result<Value, Error>, apply: Option<Apply>) {
        self.pump();
        self.refresh_tree();
        self.derive();
        self.mode = Mode::Result(ResultPopup {
            title: title.to_string(),
            lines: result::lines(outcome),
            scroll: 0,
            apply,
        });
    }

    fn show_events(&mut self) {
        self.pump();
        let mut lines: Vec<ratatui::text::Line<'static>> = self
            .event_log
            .iter()
            .map(|e| ratatui::text::Line::from(e.clone()))
            .collect();
        if lines.is_empty() {
            lines.push(ratatui::text::Line::from(
                "No changes yet: the TUI is watching",
            ));
        }
        self.mode = Mode::Result(ResultPopup {
            title: "watch".into(),
            lines,
            scroll: 0,
            apply: None,
        });
    }

    // ---------------------------------------------------------- command line

    fn command_key(&mut self, key: KeyEvent) {
        let Mode::Command { line, hint } = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.mode = Mode::View,
            KeyCode::Backspace => {
                if line.pop().is_none() {
                    self.mode = Mode::View;
                }
            }
            KeyCode::Tab => {
                if !line.contains(' ') {
                    let names = completions(line);
                    match names.as_slice() {
                        [one] => {
                            *line = format!("{one} ");
                            *hint = None;
                        }
                        [] => *hint = Some("no match".into()),
                        many => {
                            *line = common_prefix(many);
                            *hint = Some(many.join("  "));
                        }
                    }
                }
            }
            KeyCode::Enter => {
                let line = line.trim().to_string();
                self.mode = Mode::View;
                self.run_line(&line);
            }
            KeyCode::Char(c) => line.push(c),
            _ => {}
        }
    }

    /// Runs one `:` line: `q`, `w`, `wq`, or an Operation CLI-style
    /// (`move_page a b --dry-run`; `move-page` works too).
    pub fn run_line(&mut self, line: &str) {
        let words = match split_words(line) {
            Ok(words) => words,
            Err(message) => {
                self.banner = Some(Banner::Error(message));
                return;
            }
        };
        let Some(name) = words.first() else {
            return;
        };
        match name.as_str() {
            "q" => {
                self.view_key(KeyEvent::from(KeyCode::Char('q')));
                return;
            }
            "w" => {
                self.save();
                return;
            }
            "wq" => {
                self.save();
                if !self.page.as_ref().is_some_and(OpenPage::dirty) {
                    self.quit = true;
                }
                return;
            }
            _ => {}
        }
        let op = name.replace('_', "-");
        if op == "watch" {
            self.show_events();
            return;
        }
        let args = std::iter::once(op).chain(words[1..].iter().cloned());
        let parsed = CommandLine::try_parse_from(args);
        match parsed {
            Err(err) => {
                let text = err.render().to_string();
                self.mode = Mode::Result(ResultPopup {
                    title: name.clone(),
                    lines: text
                        .lines()
                        .map(|l| ratatui::text::Line::from(l.to_string()))
                        .collect(),
                    scroll: 0,
                    apply: None,
                });
            }
            Ok(CommandLine { op: command }) => {
                let title = command.name().to_string();
                let dry_run = words.iter().any(|w| w == "--dry-run");
                let outcome = command.run(&self.wiki);
                let apply = (dry_run && outcome.is_ok()).then(|| {
                    Apply::Line(
                        words
                            .iter()
                            .filter(|w| *w != "--dry-run")
                            .map(|w| quote(w))
                            .collect::<Vec<_>>()
                            .join(" "),
                    )
                });
                self.show(&title, &outcome, apply);
            }
        }
    }
}

/// The `:` line, parsed by the same clap definitions as the CLI's subcommands.
#[derive(Parser)]
#[command(name = ":", no_binary_name = true, disable_help_subcommand = true)]
struct CommandLine {
    #[command(subcommand)]
    op: Command,
}

/// `:` names starting with `prefix`: Operations (catalogue names) and built-ins.
#[must_use]
pub fn completions(prefix: &str) -> Vec<String> {
    let prefix = prefix.replace('-', "_");
    wikirs_core::registry()
        .iter()
        .map(|op| op.name)
        .chain(BUILTINS)
        .filter(|n| n.starts_with(&prefix))
        .map(str::to_string)
        .collect()
}

fn common_prefix(names: &[String]) -> String {
    let first = &names[0];
    let mut len = first.len();
    for name in &names[1..] {
        len = len.min(
            first
                .chars()
                .zip(name.chars())
                .take_while(|(a, b)| a == b)
                .count(),
        );
    }
    first[..len].to_string()
}

/// Splits a `:` line into words: whitespace separates, `"…"` and `'…'` quote,
/// and `\` escapes inside double quotes.
fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if let Some(w) = word.take() {
                    words.push(w);
                }
            }
            '"' | '\'' => {
                let w = word.get_or_insert_with(String::new);
                loop {
                    match chars.next() {
                        None => return Err(format!("unclosed {c}")),
                        Some(q) if q == c => break,
                        Some('\\') if c == '"' => match chars.next() {
                            Some('n') => w.push('\n'),
                            Some(e) => w.push(e),
                            None => return Err(format!("unclosed {c}")),
                        },
                        Some(ch) => w.push(ch),
                    }
                }
            }
            c => word.get_or_insert_with(String::new).push(c),
        }
    }
    words.extend(word);
    Ok(words)
}

/// A word as `split_words` reads it back.
fn quote(word: &str) -> String {
    if !word.is_empty() && !word.contains(|c: char| c.is_whitespace() || "\"'\\".contains(c)) {
        return word.to_string();
    }
    let escaped = word
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_split_and_quote_back() {
        let words = split_words(r#"create_page --path "a b" --content "x\n\"y\"" 'q z'"#).unwrap();
        assert_eq!(
            words,
            [
                "create_page",
                "--path",
                "a b",
                "--content",
                "x\n\"y\"",
                "q z"
            ]
        );
        let line = words.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ");
        assert_eq!(split_words(&line).unwrap(), words);
        assert!(split_words("say \"hi").is_err());
        assert_eq!(split_words("a  ''  b").unwrap(), ["a", "", "b"]);
    }

    #[test]
    fn completion_and_prefixes() {
        assert_eq!(completions("move"), ["move_page", "move_attachment"]);
        assert_eq!(completions("move-p"), ["move_page"]);
        assert!(completions("w").contains(&"w".to_string()));
        assert_eq!(
            common_prefix(&["move_page".into(), "move_attachment".into()]),
            "move_"
        );
    }
}
