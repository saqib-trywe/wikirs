//! PROTOTYPE, throwaway. Ticket 12: "What should the wikirs TUI look like?"
//!
//! Three structurally different TUIs over the same in-memory fake Wiki.
//! F2 cycles variants (or `cargo run -- --variant B`):
//!
//! - A "Columns": tree | rendered Page | Backlinks/Links/Outline/Tags. Like the GUI Workbench.
//! - B "Miller": ranger-style hierarchy columns (parent | siblings | Page preview);
//!   h/l walk up/down the hierarchy, Tab walks the Page's Links.
//! - C "Pager": one full-screen Page like `less`/`man`, Links numbered [1] [2]…
//!   and followed by typing the number; everything else through `:` and ctrl-k.
//!
//! Shared keys: e edit inline (textarea) · E hand off to $EDITOR · ctrl-s save ·
//! Esc leave edit · : command line (Tab completes Operation names) ·
//! ctrl-k palette (all 32 Operations, generated forms, dry run) · o quick open ·
//! b Backlinks popup · X simulate an external edit · q quit.
//!
//! `cargo run -- --snapshot A [default|palette|form|banner|edit|cmdline|backlinks]`
//! renders one frame as text instead of opening the terminal UI.
//!
//! Nothing is written except the temp file used for the $EDITOR hand-off.

use std::{collections::BTreeMap, io, time::SystemTime};

use ratatui::{
    DefaultTerminal, Frame, Terminal,
    backend::TestBackend,
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use ratatui_textarea::TextArea;

// ---------------------------------------------------------------- fake Wiki

#[derive(Clone)]
struct Page {
    path: &'static str,
    title: &'static str,
    tags: Vec<&'static str>,
    body: String,
}

fn fake_wiki() -> Vec<Page> {
    let p = |path, title, tags: &[&'static str], body: &str| Page { path, title, tags: tags.to_vec(), body: body.to_string() };
    vec![
        p("inbox", "Inbox", &[], "# Inbox\n\n- look at [[eng/rust/async-notes]]\n- buy flour for [[personal/recipes/bread]]\n- #todo sort this out\n"),
        p("eng", "Engineering", &[], "# Engineering\n\nHome of the eng Space. Start at [[eng/rust]] or the [wikirs design](eng/wikirs/design.md).\n"),
        p("eng/rust", "Rust", &["lang/rust"], "# Rust\n\nNotes on Rust.\n\n## Topics\n\n- [[eng/rust/async-notes]]\n- [[eng/rust/error-handling]]\n\n## Links\n\nSee also [[eng/wikirs/design]].\n"),
        p("eng/rust/async-notes", "Async notes", &["lang/rust/async"], "---\ntags: [lang/rust/async]\n---\n# Async notes\n\nPinning is the hard part. Compare with [[eng/rust/error-handling]].\n\n## Executors\n\n- tokio\n- smol\n\n## Cancellation\n\nDrop is cancellation. #lang/rust\n\n![diagram](async-notes/diagram.png)\n"),
        p("eng/rust/error-handling", "Error handling", &["lang/rust"], "# Error handling\n\nthiserror for libraries, anyhow for apps.\n\nBack to [[eng/rust]].\n"),
        p("eng/wikirs/design", "wikirs design", &["project/wikirs"], "# wikirs design\n\nOne binary, no daemon. Parity through the Operation registry.\n\n| Interface | Adapter |\n|---|---|\n| CLI | clap |\n| MCP | rmcp |\n\nRelated: [[eng/rust/async-notes]], [[eng/missing-page]].\n"),
        p("personal/reading-list", "Reading list", &["books"], "# Reading list\n\n- [ ] A Philosophy of Software Design\n- [x] Working Effectively with Legacy Code\n"),
        p("personal/recipes/bread", "Bread", &["cooking", "cooking/baking"], "# Bread\n\n500g flour, 350g water, 10g salt, 3g yeast.\n\nFrom the [[inbox]].\n"),
    ]
}

/// Links in document order: (target, is_wikilink).
fn links_of(body: &str) -> Vec<String> {
    let mut out = vec![];
    let b = body.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if &body[i..i + 2] == "[[" {
            if let Some(j) = body[i + 2..].find("]]") {
                out.push(body[i + 2..i + 2 + j].split('|').next().unwrap().to_string());
                i += j + 4;
                continue;
            }
        }
        if &body[i..i + 2] == "](" {
            if let Some(j) = body[i + 2..].find(')') {
                if let Some(t) = body[i + 2..i + 2 + j].strip_suffix(".md") {
                    out.push(t.to_string());
                }
                i += j + 3;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn outline_of(body: &str) -> Vec<(usize, String)> {
    body.lines()
        .filter(|l| l.starts_with('#') && l.contains("# "))
        .map(|l| {
            let lvl = l.chars().take_while(|c| *c == '#').count();
            (lvl, l[lvl..].trim().to_string())
        })
        .collect()
}

const OPS: &[(&str, &str, &[&str])] = &[
    ("init", "mutation", &[]),
    ("get_config", "query", &["key?"]),
    ("set_config", "mutation", &["key", "value", "scope"]),
    ("get_page", "query", &["page"]),
    ("list_pages", "query", &["filter?", "sort?", "limit?"]),
    ("create_page", "mutation", &["path", "content?", "tags?"]),
    ("write_page", "mutation", &["page", "content", "base_version?"]),
    ("edit_page", "mutation", &["page", "edits", "base_version?"]),
    ("set_page_meta", "mutation", &["page", "meta"]),
    ("move_page", "mutation", &["from", "to"]),
    ("delete_page", "mutation", &["page", "recursive?"]),
    ("children", "query", &["parent?", "depth"]),
    ("reorder_page", "mutation", &["page", "before|after"]),
    ("list_spaces", "query", &[]),
    ("links", "query", &["page"]),
    ("backlinks", "query", &["target"]),
    ("resolve_link", "query", &["from_page", "raw"]),
    ("outline", "query", &["page"]),
    ("check", "query", &["scope?", "kinds?"]),
    ("tag_tree", "query", &["scope?"]),
    ("tag_page", "mutation", &["page", "tags"]),
    ("untag_page", "mutation", &["page", "tags"]),
    ("rename_tag", "mutation", &["from", "to"]),
    ("search", "query", &["text", "filter?", "limit?"]),
    ("add_attachment", "mutation", &["page", "name", "source"]),
    ("list_attachments", "query", &["page?"]),
    ("read_attachment", "query", &["path", "as"]),
    ("move_attachment", "mutation", &["from", "to"]),
    ("delete_attachment", "mutation", &["path"]),
    ("index_status", "query", &[]),
    ("rebuild_index", "maintenance", &[]),
    ("watch", "subscription", &["scope?"]),
];

// ------------------------------------------------------------------ state

#[derive(Clone, Copy, PartialEq)]
enum Variant {
    Columns,
    Miller,
    Pager,
}
const VARIANTS: [Variant; 3] = [Variant::Columns, Variant::Miller, Variant::Pager];
impl Variant {
    fn name(self) -> &'static str {
        match self {
            Variant::Columns => "A · Columns",
            Variant::Miller => "B · Miller",
            Variant::Pager => "C · Pager",
        }
    }
}

enum Mode {
    View,
    Edit(TextArea<'static>),
    CommandLine(String),
    Palette { query: String, sel: usize, form: Option<Form> },
    QuickOpen { query: String, sel: usize },
    Backlinks { sel: usize },
}

struct Form {
    op: usize,
    fields: Vec<String>,
    active: usize,
    dry_run: bool,
}

enum Banner {
    ChangedOnDisk { disk: String, compare: bool },
    Info(String),
}

struct App {
    variant: usize,
    pages: Vec<Page>,
    current: String,
    buffer: String, // current (possibly unsaved) text of the open Page
    saved: String,
    tree_sel: usize,
    link_sel: usize,
    focus_tree: bool,
    mode: Mode,
    banner: Option<Banner>,
    result: Option<(String, String)>, // popup: title, body
    pager_digits: String,
    quit: bool,
    spawn_editor: bool,
}

impl App {
    fn new(variant: usize) -> Self {
        let pages = fake_wiki();
        let first = pages[3].clone();
        let tree_sel = tree_rows(&pages).iter().position(|r| r.path == first.path).unwrap_or(0);
        Self {
            variant,
            current: first.path.to_string(),
            buffer: first.body.clone(),
            saved: first.body,
            pages,
            tree_sel,
            link_sel: 0,
            focus_tree: true,
            mode: Mode::View,
            banner: None,
            result: None,
            pager_digits: String::new(),
            quit: false,
            spawn_editor: false,
        }
    }
    fn v(&self) -> Variant {
        VARIANTS[self.variant]
    }
    fn dirty(&self) -> bool {
        let live = match &self.mode {
            Mode::Edit(ta) => ta.lines().join("\n"),
            _ => self.buffer.clone(),
        };
        live.trim_end() != self.saved.trim_end()
    }
    fn page(&self, path: &str) -> Option<&Page> {
        self.pages.iter().find(|p| p.path == path)
    }
    fn title(&self) -> String {
        self.page(&self.current).map(|p| p.title.to_string()).unwrap_or_default()
    }
    fn backlinks(&self) -> Vec<&Page> {
        self.pages.iter().filter(|p| p.path != self.current && links_of(&p.body).contains(&self.current)).collect()
    }
    fn links(&self) -> Vec<String> {
        links_of(&self.buffer)
    }

    fn open(&mut self, path: &str) {
        let Some(p) = self.page(path).cloned() else {
            self.banner = Some(Banner::Info(format!("Broken Link `{path}`: press `c` to create it (create_page{{path}})")));
            return;
        };
        if self.dirty() {
            self.save();
        }
        self.current = p.path.to_string();
        self.buffer = p.body.clone();
        self.saved = p.body;
        self.link_sel = 0;
        self.banner = None;
        if let Some(i) = tree_rows(&self.pages).iter().position(|r| r.path == self.current) {
            self.tree_sel = i;
        }
    }

    fn save(&mut self) {
        if let Mode::Edit(ta) = &self.mode {
            self.buffer = ta.lines().join("\n");
        }
        let cur = self.current.clone();
        if let Some(p) = self.pages.iter_mut().find(|p| p.path == cur) {
            p.body = self.buffer.clone();
        }
        self.saved = self.buffer.clone();
        self.banner = Some(Banner::Info("Saved: write_page with base_version ok".into()));
    }

    fn simulate_external_edit(&mut self) {
        let t = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() % 100_000;
        let disk = format!("{}\n> Edited elsewhere ({t})\n", self.saved.trim_end());
        let cur = self.current.clone();
        if let Some(p) = self.pages.iter_mut().find(|p| p.path == cur) {
            p.body = disk.clone();
        }
        if self.dirty() {
            self.banner = Some(Banner::ChangedOnDisk { disk, compare: false });
        } else {
            self.buffer = disk.clone();
            self.saved = disk;
            self.banner = Some(Banner::Info("Reloaded: changed on disk, you had no unsaved edits".into()));
        }
    }

    fn run_op(&mut self, name: &str, args: &[String], dry_run: bool) {
        let Some((_, kind, _)) = OPS.iter().find(|(n, _, _)| *n == name) else {
            self.banner = Some(Banner::Info(format!("E: no Operation `{name}` (InvalidInput)")));
            return;
        };
        let body = match *kind {
            "mutation" => format!(
                "{} {name}({})\n\n--- a/eng/rust.md\n+++ b/eng/rust.md\n@@ -6 +6 @@\n-- [[eng/rust/async-notes]]\n+- [[{}]]\n\nwarnings: none\n{}",
                if dry_run { "DRY RUN Plan for" } else { "APPLIED Plan for" },
                args.join(", "),
                args.get(1).cloned().unwrap_or_default(),
                if dry_run { "\nnothing written. Re-run without --dry-run (or ctrl-d in the form) to apply" } else { "" }
            ),
            "subscription" => "watch: streams events into a log pane in the real TUI".into(),
            _ => format!("{name}({}) →\n{{ \"stub\": \"output rendered by the Render trait\" }}", args.join(", ")),
        };
        self.result = Some((name.to_string(), body));
    }
}

struct Row {
    path: String,
    label: String,
    depth: usize,
    placeholder: bool,
}

fn tree_rows(pages: &[Page]) -> Vec<Row> {
    let mut all: BTreeMap<String, bool> = BTreeMap::new();
    for p in pages {
        let segs: Vec<&str> = p.path.split('/').collect();
        for i in 1..segs.len() {
            all.entry(segs[..i].join("/")).or_insert(false);
        }
        all.insert(p.path.to_string(), true);
    }
    // BTreeMap order puts "eng" before "eng/rust": depth-first by path.
    all.into_iter()
        .map(|(path, is_page)| {
            let depth = path.matches('/').count();
            let label = match pages.iter().find(|p| p.path == path) {
                Some(p) => p.title.to_string(),
                None => format!("{} ·", path.rsplit('/').next().unwrap()),
            };
            Row { path, label, depth, placeholder: !is_page }
        })
        .collect()
}

// ------------------------------------------------------------- rendering

/// A rough terminal rendering of markdown: styled headings, bullets, and
/// Links (numbered when `numbered`), with the selected Link reversed.
fn render_md(app: &App, body: &str, numbered: bool, link_sel: Option<usize>) -> Text<'static> {
    let mut lines = vec![];
    let mut link_ix = 0;
    let mut in_fm = false;
    for (n, raw) in body.lines().enumerate() {
        if n == 0 && raw == "---" {
            in_fm = true;
            continue;
        }
        if in_fm {
            if raw == "---" {
                in_fm = false;
            }
            continue;
        }
        let (style, text) = if let Some(t) = raw.strip_prefix("# ") {
            (Style::new().bold().fg(Color::Cyan).add_modifier(Modifier::UNDERLINED), t.to_uppercase())
        } else if let Some(t) = raw.strip_prefix("## ") {
            (Style::new().bold().fg(Color::Cyan), t.to_string())
        } else if let Some(t) = raw.strip_prefix("- ") {
            (Style::new(), format!("  • {t}"))
        } else if raw.starts_with("![") {
            (Style::new().dim().italic(), format!("[image: {}]  (o to open externally)", raw.split('(').nth(1).unwrap_or("").trim_end_matches(')')))
        } else {
            (Style::new(), raw.to_string())
        };
        // split out links
        let mut spans = vec![];
        let mut rest = text.as_str();
        loop {
            let wl = rest.find("[[");
            let ml = rest.find("](").and_then(|i| rest[..i].rfind('[').map(|s| (s, i)));
            let next = match (wl, ml) {
                (Some(a), Some((b, _))) if b < a => ml.map(|(s, i)| (s, false, i)),
                (Some(a), _) => Some((a, true, a)),
                (None, Some((s, i))) => Some((s, false, i)),
                (None, None) => None,
            };
            let Some((start, is_wiki, mid)) = next else { break };
            let (label, target, end) = if is_wiki {
                let Some(j) = rest[start + 2..].find("]]") else { break };
                let t = &rest[start + 2..start + 2 + j];
                (t.split('|').last().unwrap().to_string(), t.split('|').next().unwrap().to_string(), start + 2 + j + 2)
            } else {
                let Some(j) = rest[mid + 2..].find(')') else { break };
                let t = &rest[mid + 2..mid + 2 + j];
                if !t.ends_with(".md") {
                    break;
                }
                (rest[start + 1..mid].to_string(), t.trim_end_matches(".md").to_string(), mid + 2 + j + 1)
            };
            spans.push(Span::styled(rest[..start].to_string(), style));
            let broken = app.page(&target).is_none();
            let mut ls = Style::new().fg(if broken { Color::Red } else { Color::Blue }).underlined();
            if link_sel == Some(link_ix) {
                ls = ls.reversed();
            }
            if numbered {
                spans.push(Span::styled(format!("[{}]", link_ix + 1), Style::new().fg(Color::Yellow).bold()));
            }
            spans.push(Span::styled(label, ls));
            link_ix += 1;
            rest = &rest[end..];
        }
        spans.push(Span::styled(rest.to_string(), style));
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}

fn breadcrumb(app: &App) -> Line<'static> {
    let mut acc = String::new();
    let mut parts = vec![];
    for seg in app.current.split('/') {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(seg);
        parts.push(app.page(&acc).map(|p| p.title.to_string()).unwrap_or(seg.to_string()));
    }
    let mut l = Line::from(vec![Span::raw(parts.join(" / ")).dim()]);
    if app.dirty() {
        l.push_span(Span::raw("  ● unsaved").red());
    }
    l
}

fn side_panel(app: &App) -> Text<'static> {
    let mut t = vec![Line::from("BACKLINKS").dim().bold()];
    let bl = app.backlinks();
    if bl.is_empty() {
        t.push(Line::from("  none").dim());
    }
    for p in bl {
        t.push(Line::from(format!("  {}", p.title)).blue());
    }
    t.push(Line::from(""));
    t.push(Line::from("LINKS").dim().bold());
    for (i, l) in app.links().iter().enumerate() {
        let broken = app.page(l).is_none();
        let mut line = Line::from(format!("  {l}{}", if broken { "  ✗" } else { "" }));
        line = if broken { line.red() } else { line.blue() };
        if !app.focus_tree && i == app.link_sel {
            line = line.reversed();
        }
        t.push(line);
    }
    t.push(Line::from(""));
    t.push(Line::from("OUTLINE").dim().bold());
    for (lvl, h) in outline_of(&app.buffer) {
        t.push(Line::from(format!("{}{}", "  ".repeat(lvl), h)));
    }
    t.push(Line::from(""));
    t.push(Line::from("TAGS").dim().bold());
    let tags = app.page(&app.current).map(|p| p.tags.clone()).unwrap_or_default();
    t.push(Line::from(format!("  {}", tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "))).magenta());
    Text::from(t)
}

fn page_area(f: &mut Frame, app: &mut App, area: Rect, numbered: bool) {
    let title = format!(" {} ", app.title());
    match &mut app.mode {
        Mode::Edit(ta) => {
            ta.set_block(Block::bordered().title(format!("{title}— EDIT (ctrl-s save, Esc done) ")).border_style(Style::new().yellow()));
            f.render_widget(&*ta, area);
        }
        _ => {
            let sel = if app.focus_tree && app.v() != Variant::Pager { None } else { Some(app.link_sel) };
            let text = render_md(app, &app.buffer, numbered, sel);
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).block(Block::bordered().title(title)), area);
        }
    }
}

fn draw(f: &mut Frame, app: &mut App) {
    let [top, banner_area, main, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if app.banner.is_some() { 1 } else { 0 }),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    f.render_widget(
        Line::from(vec![
            Span::raw(" wikirs ").bold().reversed(),
            Span::raw("  "),
            breadcrumb(app).spans.into_iter().map(|s| s.content.to_string()).collect::<String>().dim(),
            Span::raw(format!("   [{}  F2 next]", app.v().name())).yellow(),
        ]),
        top,
    );

    if let Some(b) = &app.banner {
        let line = match b {
            Banner::Info(m) => Line::from(format!(" {m}  (x dismiss)")).black().on_cyan(),
            Banner::ChangedOnDisk { .. } => {
                Line::from(" CHANGED ON DISK while you have unsaved edits:  (Esc, then)  [r] Reload   [m] Keep mine (overwrite)   [d] Compare ").white().on_red().bold()
            }
        };
        f.render_widget(line, banner_area);
    }

    match app.v() {
        Variant::Columns => {
            let [left, centre, right] = Layout::horizontal([Constraint::Length(28), Constraint::Fill(1), Constraint::Length(34)]).areas(main);
            let rows = tree_rows(&app.pages);
            let items: Vec<ListItem> = rows
                .iter()
                .map(|r| {
                    let s = format!("{}{}", "  ".repeat(r.depth), r.label);
                    let li = ListItem::new(s);
                    if r.placeholder { li.dim().italic() } else if r.path == app.current { li.bold() } else { li }
                })
                .collect();
            let mut st = ListState::default().with_selected(Some(app.tree_sel));
            let border = if app.focus_tree { Style::new().cyan() } else { Style::new() };
            f.render_stateful_widget(
                List::new(items).block(Block::bordered().title(" Pages ").border_style(border)).highlight_style(Style::new().reversed()),
                left,
                &mut st,
            );
            page_area(f, app, centre, false);
            let border = if !app.focus_tree { Style::new().cyan() } else { Style::new() };
            f.render_widget(Paragraph::new(side_panel(app)).block(Block::bordered().border_style(border)), right);
        }
        Variant::Miller => {
            let [parent_a, here_a, page_a] = Layout::horizontal([Constraint::Length(20), Constraint::Length(30), Constraint::Fill(1)]).areas(main);
            let cur = app.current.clone();
            let parent = cur.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default();
            let grand = parent.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default();
            let rows = tree_rows(&app.pages);
            let kids = |of: &str| -> Vec<&Row> {
                rows.iter()
                    .filter(|r| if of.is_empty() { r.depth == 0 } else { r.path.starts_with(&format!("{of}/")) && r.depth == of.matches('/').count() + 1 })
                    .collect()
            };
            let col = |f: &mut Frame, of: &str, hl: &str, title: String, area: Rect, active: bool| {
                let items: Vec<ListItem> = kids(of)
                    .into_iter()
                    .map(|r| {
                        let has_kids = rows.iter().any(|x| x.path.starts_with(&format!("{}/", r.path)));
                        let li = ListItem::new(format!("{}{}", r.label, if has_kids { "  ›" } else { "" }));
                        let li = if r.placeholder { li.dim().italic() } else { li };
                        if r.path == hl { li.reversed() } else { li }
                    })
                    .collect();
                let b = Block::bordered().title(title).border_style(if active { Style::new().cyan() } else { Style::new().dim() });
                f.render_widget(List::new(items).block(b), area);
            };
            col(f, &grand, &parent, " ‹ h ".into(), parent_a, false);
            col(f, &parent, &cur, format!(" {} (j/k) ", if parent.is_empty() { "Wiki" } else { &parent }), here_a, app.focus_tree);
            let [page_main, bl] = Layout::vertical([Constraint::Fill(1), Constraint::Length(5)]).areas(page_a);
            page_area(f, app, page_main, false);
            let bls = app.backlinks().iter().map(|p| p.title).collect::<Vec<_>>().join(" · ");
            let kids_line = kids(&cur).iter().map(|r| r.label.clone()).collect::<Vec<_>>().join(" · ");
            f.render_widget(
                Paragraph::new(vec![
                    Line::from(vec![Span::raw("Backlinks: ").dim(), Span::raw(bls).blue()]),
                    Line::from(vec![Span::raw("Children (l): ").dim(), Span::raw(kids_line)]),
                    Line::from("Tab cycles Links in the Page, Enter follows").dim(),
                ])
                .block(Block::default().borders(Borders::TOP)),
                bl,
            );
        }
        Variant::Pager => {
            let [pad_l, centre, _] = Layout::horizontal([Constraint::Fill(1), Constraint::Max(96), Constraint::Fill(1)]).areas(main);
            let _ = pad_l;
            page_area(f, app, centre, true);
        }
    }

    // status / command line
    let status_line = match &app.mode {
        Mode::CommandLine(s) => Line::from(format!(":{s}█")).bold(),
        Mode::Edit(_) => Line::from(" -- EDIT --   ctrl-s save   Esc done   E: open in $EDITOR instead").yellow(),
        _ => {
            let hints = match app.v() {
                Variant::Columns => "j/k move  Enter open  Tab panel  e edit  E $EDITOR  b backlinks  o open  : cmd  ^k ops  X ext-edit  q",
                Variant::Miller => "h/l up/down hierarchy  j/k siblings  Tab link  Enter follow  e edit  E $EDITOR  o open  : cmd  ^k ops  X  q",
                Variant::Pager => "1-9 follow link  Space/b page  e edit  E $EDITOR  B backlinks  o open  : cmd  ^k ops  X ext-edit  q",
            };
            let bl = app.backlinks().len();
            Line::from(vec![Span::raw(format!(" {bl} backlinks ")).on_dark_gray(), Span::raw(format!(" {hints}")).dim()])
        }
    };
    f.render_widget(status_line, status);

    // popups
    let popup = |f: &mut Frame, w: u16, h: u16| -> Rect {
        let a = f.area();
        let r = Rect { x: a.width.saturating_sub(w) / 2, y: 3, width: w.min(a.width), height: h.min(a.height - 3) };
        f.render_widget(Clear, r);
        r
    };
    match &app.mode {
        Mode::Palette { query, sel, form: None } => {
            let r = popup(f, 80, 22);
            let ops: Vec<_> = OPS.iter().filter(|(n, _, _)| n.contains(&query.replace(' ', "_"))).collect();
            let items: Vec<ListItem> = ops
                .iter()
                .map(|(n, k, ins)| ListItem::new(Line::from(vec![Span::raw(format!("{n:<18}")), Span::raw(format!("{k:<13}")).dim(), Span::raw(ins.join(", ")).dim()])))
                .collect();
            let [q, list] = Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(r);
            f.render_widget(Paragraph::new(format!("{query}█")).block(Block::bordered().title(" Operations (all 32, from the registry) ")), q);
            let mut st = ListState::default().with_selected(Some(*sel));
            f.render_stateful_widget(List::new(items).block(Block::bordered()).highlight_style(Style::new().reversed()), list, &mut st);
        }
        Mode::Palette { form: Some(form), .. } => {
            let (name, kind, ins) = OPS[form.op];
            let r = popup(f, 70, 6 + ins.len() as u16 * 1 + 3);
            let mut lines = vec![Line::from(vec![Span::raw(name).bold(), Span::raw(format!("  {kind}")).dim()]), Line::from("generated from the Input schema").dim(), Line::from("")];
            for (i, (label, val)) in ins.iter().zip(&form.fields).enumerate() {
                let l = Line::from(vec![Span::raw(format!("{label:>14}: ")).dim(), Span::raw(format!("{val}{}", if i == form.active { "█" } else { "" }))]);
                lines.push(if i == form.active { l.bold() } else { l });
            }
            if kind == "mutation" {
                lines.push(Line::from(format!("{:>14}  [{}] (ctrl-d)", "dry run", if form.dry_run { "x" } else { " " })));
            }
            lines.push(Line::from(""));
            lines.push(Line::from("Tab next field · Enter run · Esc back").dim());
            f.render_widget(Paragraph::new(lines).block(Block::bordered().title(" ctrl-k ")), r);
        }
        Mode::QuickOpen { query, sel } => {
            let r = popup(f, 70, 14);
            let q = query.to_lowercase();
            let items: Vec<ListItem> = app
                .pages
                .iter()
                .filter(|p| p.path.contains(&q) || p.title.to_lowercase().contains(&q))
                .map(|p| ListItem::new(Line::from(vec![Span::raw(format!("{:<18}", p.title)), Span::raw(p.path).dim()])))
                .collect();
            let [qa, la] = Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(r);
            f.render_widget(Paragraph::new(format!("{query}█")).block(Block::bordered().title(" Open Page (list_pages / search) ")), qa);
            let mut st = ListState::default().with_selected(Some(*sel));
            f.render_stateful_widget(List::new(items).block(Block::bordered()).highlight_style(Style::new().reversed()), la, &mut st);
        }
        Mode::Backlinks { sel } => {
            let r = popup(f, 60, 10);
            let items: Vec<ListItem> = app.backlinks().iter().map(|p| ListItem::new(format!("{}  ·  {}", p.title, p.path))).collect();
            let mut st = ListState::default().with_selected(Some(*sel));
            f.render_stateful_widget(List::new(items).block(Block::bordered().title(" Backlinks (Enter open) ")).highlight_style(Style::new().reversed()), r, &mut st);
        }
        _ => {}
    }
    if let Some(Banner::ChangedOnDisk { disk, compare: true }) = &app.banner {
        let r = popup(f, 110, 20);
        let [a, b] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(r);
        let mine = match &app.mode {
            Mode::Edit(ta) => ta.lines().join("\n"),
            _ => app.buffer.clone(),
        };
        f.render_widget(Paragraph::new(mine).block(Block::bordered().title(" Mine (unsaved) ")), a);
        f.render_widget(Paragraph::new(disk.clone()).block(Block::bordered().title(" On disk ")), b);
    }
    if let Some((title, body)) = &app.result {
        let r = popup(f, 76, 16);
        f.render_widget(Paragraph::new(body.clone()).block(Block::bordered().title(format!(" {title} (Esc) "))), r);
    }
}

// ----------------------------------------------------------------- input

fn key(app: &mut App, k: KeyEvent) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    if k.code == KeyCode::F(2) {
        app.variant = (app.variant + 1) % 3;
        return;
    }
    if app.result.is_some() {
        if matches!(k.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
            app.result = None;
        }
        return;
    }
    // banner actions work from any mode except text entry
    if let Some(Banner::ChangedOnDisk { disk, compare }) = &mut app.banner {
        if !matches!(app.mode, Mode::Edit(_)) {
            match k.code {
                KeyCode::Char('r') => {
                    let d = disk.clone();
                    app.buffer = d.clone();
                    app.saved = d;
                    app.mode = Mode::View;
                    app.banner = None;
                    return;
                }
                KeyCode::Char('m') => {
                    app.saved = disk.clone();
                    app.save();
                    return;
                }
                KeyCode::Char('d') => {
                    *compare = !*compare;
                    return;
                }
                _ => {}
            }
        }
    }

    let bl_paths: Vec<String> = app.backlinks().iter().map(|p| p.path.to_string()).collect();
    match &mut app.mode {
        Mode::Edit(ta) => {
            if ctrl && k.code == KeyCode::Char('s') {
                app.save();
            } else if k.code == KeyCode::Esc {
                app.buffer = ta.lines().join("\n");
                app.mode = Mode::View;
            } else if ctrl && k.code == KeyCode::Char('x') {
                app.simulate_external_edit();
            } else {
                ta.input(k);
            }
            return;
        }
        Mode::CommandLine(s) => {
            match k.code {
                KeyCode::Esc => app.mode = Mode::View,
                KeyCode::Backspace => {
                    s.pop();
                }
                KeyCode::Tab => {
                    let word = s.split(' ').next().unwrap_or("").to_string();
                    let m: Vec<_> = OPS.iter().filter(|(n, _, _)| n.starts_with(&word)).collect();
                    if m.len() == 1 && !s.contains(' ') {
                        *s = format!("{} ", m[0].0);
                    } else if !s.contains(' ') {
                        app.banner = Some(Banner::Info(m.iter().map(|(n, _, _)| *n).collect::<Vec<_>>().join("  ")));
                    }
                }
                KeyCode::Enter => {
                    let line = s.clone();
                    app.mode = Mode::View;
                    let mut parts: Vec<String> = line.split_whitespace().map(String::from).collect();
                    if parts.first().map(|p| p == "q").unwrap_or(false) {
                        app.quit = true;
                        return;
                    }
                    let dry = parts.iter().any(|p| p == "--dry-run" || p == "-n");
                    parts.retain(|p| !p.starts_with('-'));
                    if let Some((name, args)) = parts.split_first() {
                        app.run_op(name, args, dry);
                    }
                }
                KeyCode::Char(c) => s.push(c),
                _ => {}
            }
            return;
        }
        Mode::Palette { query, sel, form } => {
            if let Some(fm) = form {
                let nfields = fm.fields.len();
                match k.code {
                    KeyCode::Esc => *form = None,
                    KeyCode::Tab if nfields > 0 => fm.active = (fm.active + 1) % nfields,
                    KeyCode::Char('d') if ctrl => fm.dry_run = !fm.dry_run,
                    KeyCode::Backspace if nfields > 0 => {
                        fm.fields[fm.active].pop();
                    }
                    KeyCode::Enter => {
                        let (name, _, _) = OPS[fm.op];
                        let (args, dry) = (fm.fields.clone(), fm.dry_run);
                        app.mode = Mode::View;
                        app.run_op(name, &args, dry);
                    }
                    KeyCode::Char(c) if nfields > 0 => fm.fields[fm.active].push(c),
                    _ => {}
                }
                return;
            }
            let matches: Vec<usize> = OPS.iter().enumerate().filter(|(_, (n, _, _))| n.contains(&query.replace(' ', "_"))).map(|(i, _)| i).collect();
            match k.code {
                KeyCode::Esc => app.mode = Mode::View,
                KeyCode::Down => *sel = (*sel + 1).min(matches.len().saturating_sub(1)),
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Backspace => {
                    query.pop();
                    *sel = 0;
                }
                KeyCode::Enter => {
                    if let Some(&op) = matches.get(*sel) {
                        let cur = app.current.clone();
                        let fields = OPS[op].2.iter().map(|n| if n.starts_with("page") || *n == "from" || *n == "target" { cur.clone() } else { String::new() }).collect();
                        *form = Some(Form { op, fields, active: 0, dry_run: true });
                    }
                }
                KeyCode::Char(c) => {
                    query.push(c);
                    *sel = 0;
                }
                _ => {}
            }
            return;
        }
        Mode::QuickOpen { query, sel } => {
            let q = query.to_lowercase();
            let matches: Vec<String> = app.pages.iter().filter(|p| p.path.contains(&q) || p.title.to_lowercase().contains(&q)).map(|p| p.path.to_string()).collect();
            match k.code {
                KeyCode::Esc => app.mode = Mode::View,
                KeyCode::Down => *sel = (*sel + 1).min(matches.len().saturating_sub(1)),
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Backspace => {
                    query.pop();
                    *sel = 0;
                }
                KeyCode::Enter => {
                    if let Some(p) = matches.get(*sel).cloned() {
                        app.mode = Mode::View;
                        app.open(&p);
                    }
                }
                KeyCode::Char(c) => {
                    query.push(c);
                    *sel = 0;
                }
                _ => {}
            }
            return;
        }
        Mode::Backlinks { sel } => {
            let bl = bl_paths;
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::View,
                KeyCode::Down | KeyCode::Char('j') => *sel = (*sel + 1).min(bl.len().saturating_sub(1)),
                KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
                KeyCode::Enter => {
                    if let Some(p) = bl.get(*sel).cloned() {
                        app.mode = Mode::View;
                        app.open(&p);
                    }
                }
                _ => {}
            }
            return;
        }
        Mode::View => {}
    }

    // ---- view mode
    let links = app.links();
    match k.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('k') if ctrl => app.mode = Mode::Palette { query: String::new(), sel: 0, form: None },
        KeyCode::Char(':') => app.mode = Mode::CommandLine(String::new()),
        KeyCode::Char('o') | KeyCode::Char('p') if !ctrl || k.code == KeyCode::Char('p') => app.mode = Mode::QuickOpen { query: String::new(), sel: 0 },
        KeyCode::Char('e') => {
            let mut ta = TextArea::new(app.buffer.lines().map(String::from).collect());
            ta.set_cursor_line_style(Style::new());
            app.mode = Mode::Edit(ta);
        }
        KeyCode::Char('E') => app.spawn_editor = true,
        KeyCode::Char('X') => app.simulate_external_edit(),
        KeyCode::Char('x') => app.banner = None,
        KeyCode::Char('c') => {
            if let Some(Banner::Info(m)) = &app.banner {
                if m.starts_with("Broken Link") {
                    let path = m.split('`').nth(1).unwrap_or("").to_string();
                    app.run_op("create_page", &[path], true);
                }
            }
        }
        KeyCode::Char('b') if app.v() != Variant::Pager => app.mode = Mode::Backlinks { sel: 0 },
        KeyCode::Char('B') => app.mode = Mode::Backlinks { sel: 0 },
        KeyCode::Char(d @ '1'..='9') if app.v() == Variant::Pager => {
            app.pager_digits.push(d);
            let n: usize = app.pager_digits.parse().unwrap();
            app.pager_digits.clear();
            if let Some(t) = links.get(n - 1).cloned() {
                app.open(&t);
            }
        }
        KeyCode::Tab => {
            if app.v() == Variant::Columns && app.focus_tree {
                app.focus_tree = false;
            } else if !links.is_empty() {
                if app.v() == Variant::Columns && app.link_sel + 1 >= links.len() {
                    app.focus_tree = true;
                    app.link_sel = 0;
                } else {
                    app.focus_tree = false;
                    app.link_sel = (app.link_sel + 1) % links.len();
                }
            }
        }
        KeyCode::Enter => {
            if app.focus_tree && app.v() == Variant::Columns {
                let rows = tree_rows(&app.pages);
                if let Some(r) = rows.get(app.tree_sel) {
                    if !r.placeholder {
                        let p = r.path.clone();
                        app.open(&p);
                    }
                }
            } else if let Some(t) = links.get(app.link_sel).cloned() {
                app.open(&t);
            }
        }
        KeyCode::Char('j') | KeyCode::Down => match app.v() {
            Variant::Columns if app.focus_tree => app.tree_sel = (app.tree_sel + 1).min(tree_rows(&app.pages).len() - 1),
            Variant::Columns => app.link_sel = (app.link_sel + 1).min(links.len().saturating_sub(1)),
            Variant::Miller => miller_step(app, 1),
            Variant::Pager => {}
        },
        KeyCode::Char('k') | KeyCode::Up => match app.v() {
            Variant::Columns if app.focus_tree => app.tree_sel = app.tree_sel.saturating_sub(1),
            Variant::Columns => app.link_sel = app.link_sel.saturating_sub(1),
            Variant::Miller => miller_step(app, -1),
            Variant::Pager => {}
        },
        KeyCode::Char('h') | KeyCode::Left if app.v() == Variant::Miller => {
            if let Some((parent, _)) = app.current.clone().rsplit_once('/') {
                let parent = parent.to_string();
                if app.page(&parent).is_some() {
                    app.open(&parent);
                } else {
                    app.banner = Some(Banner::Info(format!("`{parent}` is a Placeholder (no Page)")));
                }
            }
        }
        KeyCode::Char('l') | KeyCode::Right if app.v() == Variant::Miller => {
            let rows = tree_rows(&app.pages);
            let prefix = format!("{}/", app.current);
            if let Some(r) = rows.iter().find(|r| r.path.starts_with(&prefix) && !r.placeholder) {
                let p = r.path.clone();
                app.open(&p);
            }
        }
        _ => {}
    }
}

fn miller_step(app: &mut App, d: i32) {
    let rows = tree_rows(&app.pages);
    let parent = app.current.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default();
    let sibs: Vec<&Row> = rows
        .iter()
        .filter(|r| !r.placeholder && if parent.is_empty() { r.depth == 0 } else { r.path.starts_with(&format!("{parent}/")) && r.depth == parent.matches('/').count() + 1 })
        .collect();
    if let Some(i) = sibs.iter().position(|r| r.path == app.current) {
        let j = (i as i32 + d).clamp(0, sibs.len() as i32 - 1) as usize;
        let p = sibs[j].path.clone();
        app.open(&p);
    }
}

// ------------------------------------------------------------------ main

fn run_editor(app: &mut App) -> io::Result<()> {
    // PROTOTYPE, wipe me: the hand-off needs a real file.
    let path = std::env::temp_dir().join(format!("wikirs-proto-{}.md", app.current.replace('/', "_")));
    std::fs::write(&path, &app.buffer)?;
    ratatui::restore();
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
    let status = std::process::Command::new(&editor).arg(&path).status();
    let text = std::fs::read_to_string(&path)?;
    let _ = std::fs::remove_file(&path);
    app.buffer = text;
    app.mode = Mode::View;
    app.banner = Some(Banner::Info(match status {
        Ok(_) if app.dirty() => format!("Back from {editor}: changes in buffer, ctrl-s / e then ctrl-s to save (write_page with base_version)"),
        Ok(_) => format!("Back from {editor}: no changes"),
        Err(e) => format!("could not start {editor}: {e}"),
    }));
    Ok(())
}

fn event_loop(mut terminal: DefaultTerminal, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|f| draw(f, app))?;
        if let Event::Key(k) = event::read()? {
            if k.kind != KeyEventKind::Press {
                continue;
            }
            // ctrl-s saves from view mode too (after an $EDITOR round trip)
            if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('s') && !matches!(app.mode, Mode::Edit(_)) {
                app.save();
                continue;
            }
            key(app, k);
        }
        if app.spawn_editor {
            app.spawn_editor = false;
            run_editor(app)?;
            terminal = ratatui::init();
        }
        if app.quit {
            return Ok(());
        }
    }
}

fn snapshot(variant: usize, scenario: &str) {
    let mut app = App::new(variant);
    match scenario {
        "palette" => app.mode = Mode::Palette { query: "page".into(), sel: 3, form: None },
        "form" => app.mode = Mode::Palette { query: String::new(), sel: 0, form: Some(Form { op: 9, fields: vec!["eng/rust/async-notes".into(), "eng/rust/async".into()], active: 1, dry_run: true }) },
        "banner" => {
            let mut ta = TextArea::new(app.buffer.lines().map(String::from).collect());
            ta.insert_str("typing… ");
            app.mode = Mode::Edit(ta);
            app.simulate_external_edit();
        }
        "edit" => app.mode = Mode::Edit(TextArea::new(app.buffer.lines().map(String::from).collect())),
        "cmdline" => app.mode = Mode::CommandLine("move_page eng/rust/async-notes eng/rust/async --dry-run".into()),
        "backlinks" => app.mode = Mode::Backlinks { sel: 0 },
        "links" => {
            app.focus_tree = false;
            app.link_sel = 0;
        }
        _ => {}
    }
    let mut t = Terminal::new(TestBackend::new(130, 36)).unwrap();
    t.draw(|f| draw(f, &mut app)).unwrap();
    let buf = t.backend().buffer();
    for y in 0..buf.area.height {
        let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        println!("{}", line.trim_end());
    }
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let pick = |s: &str| match s {
        "B" | "b" => 1,
        "C" | "c" => 2,
        _ => 0,
    };
    let variant = args.iter().position(|a| a == "--variant").and_then(|i| args.get(i + 1)).map(|s| pick(s)).unwrap_or(0);
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        let v = args.get(i + 1).map(|s| pick(s)).unwrap_or(0);
        snapshot(v, args.get(i + 2).map(String::as_str).unwrap_or("default"));
        return Ok(());
    }
    let mut app = App::new(variant);
    let terminal = ratatui::init();
    let r = event_loop(terminal, &mut app);
    ratatui::restore();
    r
}
