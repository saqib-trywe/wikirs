//! The TUI's state and behaviour (docs/spec/tui.md): keys and watch events in,
//! Operations called in-process, state out for [`crate::ui`] to draw.
//!
//! The open Page and its reactions to saves and changes on disk are the
//! [`Session`] the GUI shares; this keeps only the TUI's view state. Nothing here
//! touches the terminal, so tests drive an [`App`] with keys and draw it to a
//! `TestBackend`. The one thing that needs the terminal, the `$EDITOR`
//! hand-off, is asked for through [`App::take_request`].

use std::path::PathBuf;

use clap::Parser;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui_textarea::TextArea;
use serde_json::{Value, json};
use wikirs_core::{Command, Error, Kind, Operation, Wiki, find, markdown, watch::Watch};
use wikirs_ui::{Followed, Form, Notice, Session};

use crate::{
    form::{FormAction, FormView},
    render, result,
};

/// Built-in `:` commands besides the Operations.
const BUILTINS: [&str; 3] = ["q", "w", "wq"];

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
    pub session: Session,
    pub tree_sel: usize,
    pub focus: Focus,
    /// The selected Link of the open Page (Links focus).
    pub link_sel: usize,
    /// The open Page's scroll position, in lines.
    pub scroll: u16,
    pub mode: Mode,
    /// Changed on disk: show mine and disk side by side.
    pub compare: bool,
    /// Digits typed so far towards a numbered Link.
    pub digits: String,
    /// Draws inline images when set (the terminal loop detects how).
    pub images: Option<crate::images::Images>,
    quit: bool,
    request: Option<Request>,
}

impl App {
    /// A TUI on `wiki`, showing the first Page in the tree.
    #[must_use]
    pub fn new(wiki: Wiki) -> Self {
        let mut app = Self {
            session: Session::new(wiki),
            tree_sel: 0,
            focus: Focus::Tree,
            link_sel: 0,
            scroll: 0,
            mode: Mode::View,
            compare: false,
            digits: String::new(),
            images: None,
            quit: false,
            request: None,
        };
        app.select_current();
        app
    }

    /// Watches the files for other writers' changes (process-model.md).
    pub fn start_watcher(&mut self) -> Result<(), Error> {
        self.session.start_watcher()
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
        match outcome {
            Ok(text) => {
                let changed = self.session.page.as_ref().is_some_and(|p| p.buffer != text);
                self.session.set_buffer(text);
                self.mode = Mode::View;
                self.session.notice = Some(Notice::Info(if changed {
                    "Back from the editor: ctrl-s saves".into()
                } else {
                    "Back from the editor: no changes".into()
                }));
            }
            Err(err) => self.session.notice = Some(Notice::Error(format!("editor: {err}"))),
        }
    }

    /// Opens a Page (unless the open one has unsaved edits).
    pub fn open(&mut self, path: &str) {
        if self.session.open(path) {
            self.opened();
        } else if self.session.dirty() {
            self.session.notice = Some(Notice::Info(format!(
                "Unsaved edits to `{}`: ctrl-s saves, D discards them",
                self.session.page.as_ref().map_or("", |p| p.path.as_str())
            )));
        }
    }

    /// A Page was opened: reset the view onto it.
    fn opened(&mut self) {
        self.scroll = 0;
        self.link_sel = 0;
        self.compare = false;
        self.select_current();
    }

    /// Selects the open Page in the tree.
    fn select_current(&mut self) {
        if let Some(page) = &self.session.page
            && let Some(i) = self.session.tree.iter().position(|r| r.path == page.path)
        {
            self.tree_sel = i;
        }
        self.tree_sel = self.tree_sel.min(self.session.tree.len().saturating_sub(1));
    }

    fn save(&mut self) {
        self.sync_edit();
        self.session.save();
        self.after_change();
    }

    /// The textarea's text into the buffer.
    fn sync_edit(&mut self) {
        if let (Mode::Edit(area), Some(page)) = (&self.mode, &self.session.page) {
            let mut text = area.lines().join("\n");
            // A textarea has no final newline; keep the file's.
            if page.saved.ends_with('\n') || page.buffer.ends_with('\n') {
                text.push('\n');
            }
            self.session.set_buffer(text);
        }
    }

    /// Takes in pending watch events (call once per loop turn).
    pub fn pump(&mut self) {
        if self.session.pump() {
            if let Some(images) = &self.images {
                images.clear();
            }
            self.after_change();
        }
    }

    /// One change event, as the watcher sends it.
    pub fn on_watch(&mut self, event: &wikirs_core::watch::WatchEvent) {
        self.session.on_watch(event);
        self.after_change();
    }

    /// Keeps the view's selections inside what's there now.
    fn after_change(&mut self) {
        self.select_current();
        let links = self.session.page.as_ref().map_or(0, |p| p.doc.links.len());
        self.link_sel = self.link_sel.min(links.saturating_sub(1));
        if !matches!(self.session.notice, Some(Notice::ChangedOnDisk { .. })) {
            self.compare = false;
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
        if !matches!(self.session.notice, Some(Notice::ChangedOnDisk { .. })) {
            return false;
        }
        match key.code {
            KeyCode::Char('r') => self.session.reload_from_disk(),
            KeyCode::Char('m') => self.session.keep_mine(),
            KeyCode::Char('d') => {
                self.compare = !self.compare;
                return true;
            }
            _ => return false,
        }
        self.after_change();
        true
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
        let links = self.session.page.as_ref().map_or(0, |p| p.doc.links.len());
        match key.code {
            KeyCode::Enter if pending => {}
            KeyCode::Char('q') => {
                if self.session.dirty() {
                    self.session.notice = Some(Notice::Info(
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
            KeyCode::Char('d') if ctrl => self.scroll_by(10),
            KeyCode::Char('u') if ctrl => self.scroll_by(-10),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_by(20),
            KeyCode::PageUp => self.scroll_by(-20),
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
                if let Some(page) = &self.session.page {
                    let mut area = TextArea::new(page.buffer.lines().map(String::from).collect());
                    area.set_cursor_line_style(ratatui::style::Style::new());
                    self.mode = Mode::Edit(Box::new(area));
                }
            }
            KeyCode::Char('E') => {
                if let Some(page) = &self.session.page {
                    self.request = Some(Request::Editor(page.buffer.clone()));
                }
            }
            KeyCode::Char('D') => self.session.discard(),
            KeyCode::Char('R') => {
                if let Some(page) = &self.session.page {
                    let path = page.path.clone();
                    self.open_form("move_page", &json!({ "from": path, "to": path }));
                }
            }
            KeyCode::Char('c') => {
                if let Some(Notice::Create(path)) = &self.session.notice {
                    let path = path.clone();
                    self.session.create(&path);
                    self.opened();
                }
            }
            KeyCode::Char('x') | KeyCode::Esc => {
                if !matches!(self.session.notice, Some(Notice::ChangedOnDisk { .. })) {
                    self.session.notice = None;
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
                if let Some(row) = self.session.tree.get(self.tree_sel) {
                    let (path, placeholder) = (row.path.clone(), row.placeholder);
                    if placeholder {
                        self.session.notice = Some(Notice::Create(path));
                    } else {
                        self.open(&path);
                    }
                }
            }
            Focus::Links => self.follow(self.link_sel),
        }
    }

    fn move_selection(&mut self, by: isize) {
        match self.focus {
            Focus::Tree => {
                self.tree_sel = self
                    .tree_sel
                    .saturating_add_signed(by)
                    .min(self.session.tree.len().saturating_sub(1));
            }
            Focus::Links => {
                let links = self.session.page.as_ref().map_or(0, |p| p.doc.links.len());
                self.link_sel = self
                    .link_sel
                    .saturating_add_signed(by)
                    .min(links.saturating_sub(1));
            }
        }
    }

    fn scroll_by(&mut self, by: i16) {
        self.scroll = self.scroll.saturating_add_signed(by);
    }

    /// A digit towards a numbered Link: follows as soon as no more digits could
    /// make a different Link, else waits for more (or Enter).
    fn digit(&mut self, d: char) {
        self.digits.push(d);
        let count = self.session.page.as_ref().map_or(0, |p| p.doc.links.len());
        let n: usize = self.digits.parse().unwrap_or(0);
        if n == 0 || n > count {
            self.session.notice = Some(Notice::Info(format!("No Link [{}]", self.digits)));
            self.digits.clear();
        } else if n * 10 > count {
            self.digits.clear();
            self.follow(n - 1);
        }
    }

    /// Follows Link `i` of the open Page, scrolling to its heading if it names one.
    pub fn follow(&mut self, i: usize) {
        match self.session.follow(i) {
            Followed::Opened { heading } => {
                self.opened();
                if let (Some(heading), Some(page)) = (heading, &self.session.page) {
                    let anchor = markdown::anchor(&heading);
                    let rendered =
                        render::render(&page.doc, &page.resolved, None, self.images.is_some());
                    if let Some((_, line)) = rendered.headings.iter().find(|(a, _)| *a == anchor) {
                        self.scroll = u16::try_from(*line).unwrap_or(u16::MAX);
                    }
                }
            }
            Followed::OpenExternal(file) => self.request = Some(Request::OpenExternal(file)),
            Followed::Stayed => {}
        }
    }

    // --------------------------------------------------------------- pickers

    fn open_picker(&mut self, kind: PickKind) {
        let all: Vec<(String, String, String)> = match kind {
            PickKind::Open => self
                .session
                .all_pages()
                .into_iter()
                .map(|(path, title)| (path.clone(), title, path))
                .collect(),
            PickKind::Backlinks => self.session.page.as_ref().map_or_else(Vec::new, |p| {
                p.backlinks
                    .iter()
                    .map(|b| (b.clone(), self.session.title_of(b), b.clone()))
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
            PickKind::Search => self.session.search(&picker.query),
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

    fn palette_key(&mut self, key: KeyEvent) {
        let Mode::Palette { query, sel } = &mut self.mode else {
            return;
        };
        let matches = Session::palette_matches(query);
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
                    let path = self.session.page.as_ref().map(|p| p.path.clone());
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
            self.session.notice = Some(Notice::Error(format!("No form for `{op}`")));
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
        let mutation = find(op).is_some_and(|info| info.kind == Kind::Mutation);
        let outcome = self.session.run_json(op, input.clone());
        let apply = (mutation && input["dry_run"] == json!(true) && outcome.is_ok()).then(|| {
            let mut input = input;
            input["dry_run"] = json!(false);
            Apply::Input {
                op: op.to_string(),
                input,
            }
        });
        self.show(op, &outcome, apply);
    }

    fn show(&mut self, title: &str, outcome: &Result<Value, Error>, apply: Option<Apply>) {
        self.after_change();
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
            .session
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
                self.session.notice = Some(Notice::Error(message));
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
                if !self.session.dirty() {
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
                let outcome = command.run(self.session.wiki());
                self.session.after_op();
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
