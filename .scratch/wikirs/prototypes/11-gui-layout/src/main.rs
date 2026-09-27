//! PROTOTYPE, throwaway. Ticket 11: "What should the wikirs GUI look like?"
//!
//! Three structurally different layouts of the same fake Wiki, switchable from
//! the floating bar at the bottom (or cmd-alt-left / cmd-alt-right, or
//! `WIKIRS_VARIANT=A|B|C cargo run`):
//!
//! - A "Workbench": sidebar (Pages / Tags trees), source|preview split, right
//!   panel with Backlinks, Outline and Tags.
//! - B "Reader": Space switcher on top, rendered Page by default, Edit toggles
//!   to full-width source; Backlinks and Tags sit under the content.
//! - C "Focus": no panels. A centred source column, a status bar, and
//!   everything else through the palette (cmd-p open, cmd-k commands).
//!
//! Shared in every variant: the registry-generated command palette (cmd-k),
//! `[[` link autocomplete in the editor, cmd-s save, cmd-e toggle preview, and
//! the "changed on disk" behaviour (simulate it from the floating bar).
//!
//! Nothing touches disk: the Wiki is an in-memory fake.

use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

use gpui_kit::component::{
    ActiveTheme as _, Root, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{CompletionProvider, Editor, EditorState, Input, InputEvent, InputState, Rope, RopeExt},
    list::ListItem,
    resizable::{h_resizable, resizable_panel},
    text::{TextView, TextViewState},
    tree::{TreeItem, TreeState, tree},
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Range as LspRange, TextEdit,
};

actions!(
    proto,
    [TogglePalette, QuickOpen, ToggleMode, Save, NextVariant, PrevVariant, ClosePalette]
);

// ---------------------------------------------------------------- fake Wiki

#[derive(Clone)]
struct Page {
    path: &'static str,
    title: &'static str,
    tags: Vec<&'static str>,
    body: String,
}

fn fake_wiki() -> Vec<Page> {
    let p = |path, title, tags: &[&'static str], body: &str| Page {
        path,
        title,
        tags: tags.to_vec(),
        body: body.to_string(),
    };
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

fn links_of(body: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = body;
    while let Some(i) = rest.find("[[") {
        rest = &rest[i + 2..];
        if let Some(j) = rest.find("]]") {
            out.push(rest[..j].split('|').next().unwrap().to_string());
            rest = &rest[j + 2..];
        }
    }
    let mut rest = body;
    while let Some(i) = rest.find("](") {
        rest = &rest[i + 2..];
        if let Some(j) = rest.find(')') {
            let t = &rest[..j];
            if let Some(t) = t.strip_suffix(".md") {
                out.push(t.to_string());
            }
            rest = &rest[j..];
        }
    }
    out
}

fn outline_of(body: &str) -> Vec<(usize, String)> {
    body.lines()
        .filter(|l| l.starts_with('#') && l.contains(' '))
        .map(|l| {
            let level = l.chars().take_while(|c| *c == '#').count();
            (level, l[level..].trim().to_string())
        })
        .filter(|(l, _)| *l <= 6)
        .collect()
}

// A fake slice of the Operation registry: name, kind, inputs.
const OPS: &[(&str, &str, &[&str])] = &[
    ("init", "mutation", &[]),
    ("get_config", "query", &["key?"]),
    ("set_config", "mutation", &["key", "value", "scope"]),
    ("get_page", "query", &["page"]),
    ("list_pages", "query", &["filter?", "sort?", "limit?", "offset?"]),
    ("create_page", "mutation", &["path | parent+title", "content?", "tags?"]),
    ("write_page", "mutation", &["page", "content", "base_version?"]),
    ("edit_page", "mutation", &["page", "edits[]", "base_version?"]),
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
    ("tag_page", "mutation", &["page", "tags[]"]),
    ("untag_page", "mutation", &["page", "tags[]"]),
    ("rename_tag", "mutation", &["from", "to"]),
    ("search", "query", &["text", "filter?", "limit?"]),
    ("add_attachment", "mutation", &["page", "name", "source"]),
    ("list_attachments", "query", &["page? | scope?"]),
    ("read_attachment", "query", &["path", "as"]),
    ("move_attachment", "mutation", &["from", "to"]),
    ("delete_attachment", "mutation", &["path"]),
    ("index_status", "query", &[]),
    ("rebuild_index", "maintenance", &[]),
    ("watch", "subscription", &["scope?"]),
];

// ---------------------------------------------------------- [[ completion

struct LinkCompletion {
    pages: Rc<RefCell<Vec<Page>>>,
}

impl CompletionProvider for LinkCompletion {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let s = text.to_string();
        let before = &s[..offset.min(s.len())];
        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line = &before[line_start..];
        let Some(open) = line.rfind("[[") else {
            return Task::ready(Ok(CompletionResponse::Array(vec![])));
        };
        let query = &line[open + 2..];
        if query.contains("]]") {
            return Task::ready(Ok(CompletionResponse::Array(vec![])));
        }
        let q = query.to_lowercase();
        let start = text.offset_to_position(line_start + open + 2);
        let end = text.offset_to_position(offset);
        let items = self
            .pages
            .borrow()
            .iter()
            .filter(|p| p.path.contains(&q) || p.title.to_lowercase().contains(&q))
            .map(|p| CompletionItem {
                label: p.path.to_string(),
                detail: Some(p.title.to_string()),
                kind: Some(CompletionItemKind::FILE),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: LspRange { start, end },
                    new_text: format!("{}]]", p.path),
                })),
                ..Default::default()
            })
            .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _offset: usize, _new_text: &str, _cx: &mut App) -> bool {
        true // completions() itself returns nothing unless the cursor is inside `[[`
    }
}

// ------------------------------------------------------------------ state

#[derive(Clone, Copy, PartialEq)]
enum Variant {
    Workbench,
    Reader,
    Focus,
}
const VARIANTS: [Variant; 3] = [Variant::Workbench, Variant::Reader, Variant::Focus];
impl Variant {
    fn name(self) -> &'static str {
        match self {
            Variant::Workbench => "A · Workbench",
            Variant::Reader => "B · Reader",
            Variant::Focus => "C · Focus",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum PaletteMode {
    Commands,
    QuickOpen,
}

enum Banner {
    ChangedOnDisk { disk: String },
    Info(String),
}

struct Proto {
    variant: usize,
    pages: Rc<RefCell<Vec<Page>>>,
    current: &'static str,
    saved: String,
    editor: Entity<EditorState>,
    preview: Entity<TextViewState>,
    page_tree: Entity<TreeState>,
    tag_tree: Entity<TreeState>,
    left_tab_tags: bool,
    show_preview: bool, // A: split vs source only; B: reading vs editing; C: overlay preview
    reader_space: &'static str,
    palette: Option<PaletteMode>,
    palette_input: Entity<InputState>,
    palette_op: Option<usize>,
    palette_fields: Vec<Entity<InputState>>,
    dry_run: bool,
    plan: Option<String>,
    banner: Option<Banner>,
    compare: bool,
    _subs: Vec<Subscription>,
}

impl Proto {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pages = Rc::new(RefCell::new(fake_wiki()));
        let first = pages.borrow()[3].clone();
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("markdown")
                .line_number(false)
                .soft_wrap(true)
                .default_value(first.body.clone())
        });
        editor.update(cx, |st, cx| {
            st.lsp_mut().completion_provider = Some(Rc::new(LinkCompletion { pages: pages.clone() }));
            cx.notify();
        });
        let preview = cx.new(|cx| TextViewState::markdown(&first.body, cx));
        let page_tree = cx.new(|cx| TreeState::new(cx).items(page_tree_items(&pages.borrow())));
        let tag_tree = cx.new(|cx| TreeState::new(cx).items(tag_tree_items(&pages.borrow())));
        let palette_input = cx.new(|cx| InputState::new(window, cx).placeholder("Type an Operation…"));

        let subs = vec![
            cx.subscribe(&editor, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&palette_input, |_, _, _: &InputEvent, cx| cx.notify()),
        ];

        let variant = match std::env::var("WIKIRS_VARIANT").as_deref() {
            Ok("B") | Ok("b") => 1,
            Ok("C") | Ok("c") => 2,
            _ => 0,
        };
        let fh = editor.focus_handle(cx);
        window.defer(cx, move |window, cx| fh.focus(window, cx));

        Self {
            variant,
            pages,
            current: first.path,
            saved: first.body,
            editor,
            preview,
            page_tree,
            tag_tree,
            left_tab_tags: false,
            show_preview: true,
            reader_space: "eng",
            palette: None,
            palette_input,
            palette_op: None,
            palette_fields: vec![],
            dry_run: true,
            plan: None,
            banner: None,
            compare: false,
            _subs: subs,
        }
    }

    fn v(&self) -> Variant {
        VARIANTS[self.variant]
    }
    fn source(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }
    fn dirty(&self, cx: &App) -> bool {
        self.source(cx) != self.saved
    }
    fn page(&self, path: &str) -> Option<Page> {
        self.pages.borrow().iter().find(|p| p.path == path).cloned()
    }
    fn backlinks(&self, path: &str) -> Vec<Page> {
        self.pages
            .borrow()
            .iter()
            .filter(|p| p.path != path && links_of(&p.body).iter().any(|l| l == path))
            .cloned()
            .collect()
    }

    fn open(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.page(path) else {
            self.banner = Some(Banner::Info(format!(
                "Broken Link: `{path}` does not exist. (Real app: offer create_page{{path}}.)"
            )));
            cx.notify();
            return;
        };
        if self.dirty(cx) {
            self.save(cx);
        }
        self.current = page.path;
        self.saved = page.body.clone();
        self.banner = None;
        self.compare = false;
        self.editor.update(cx, |st, cx| st.set_value(page.body.clone(), window, cx));
        if let Some(space) = page.path.split('/').next().filter(|_| page.path.contains('/')) {
            if let Some(s) = ["eng", "personal"].into_iter().find(|s| *s == space) {
                self.reader_space = s;
            }
        }
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let src = self.source(cx);
        if let Some(p) = self.pages.borrow_mut().iter_mut().find(|p| p.path == self.current) {
            p.body = src.clone();
        }
        self.saved = src;
        self.banner = Some(Banner::Info("Saved (write_page with base_version: ok)".into()));
        cx.notify();
    }

    /// Simulates another process (vim, git pull, an MCP agent) editing the open Page.
    fn simulate_external_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let disk = format!("{}\n> Edited elsewhere at {:?}\n", self.saved.trim_end(), std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() % 100_000);
        if let Some(p) = self.pages.borrow_mut().iter_mut().find(|p| p.path == self.current) {
            p.body = disk.clone();
        }
        if self.dirty(cx) {
            self.banner = Some(Banner::ChangedOnDisk { disk });
        } else {
            self.saved = disk.clone();
            self.editor.update(cx, |st, cx| st.set_value(disk, window, cx));
            self.banner = Some(Banner::Info("Reloaded: the file changed on disk (no unsaved edits)".into()));
        }
        cx.notify();
    }

    fn open_palette(&mut self, mode: PaletteMode, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = Some(mode);
        self.palette_op = None;
        self.plan = None;
        let ph = match mode {
            PaletteMode::Commands => "Operation… (all 32 from the registry)",
            PaletteMode::QuickOpen => "Open Page by path or Title…",
        };
        self.palette_input.update(cx, |st, cx| {
            st.set_value("", window, cx);
            st.set_placeholder(ph, window, cx);
            st.focus(window, cx);
        });
        cx.notify();
    }

    fn pick_op(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_op = Some(ix);
        self.plan = None;
        let (_, _, inputs) = OPS[ix];
        let current = self.current;
        self.palette_fields = inputs
            .iter()
            .map(|name| {
                let name = name.to_string();
                cx.new(|cx| {
                    let st = InputState::new(window, cx).placeholder(name.clone());
                    if name.starts_with("page") || name == "from" || name == "target" {
                        st.default_value(current)
                    } else {
                        st
                    }
                })
            })
            .collect();
        cx.notify();
    }

    fn run_op(&mut self, cx: &mut Context<Self>) {
        let Some(ix) = self.palette_op else { return };
        let (name, kind, _) = OPS[ix];
        let args: Vec<String> = self.palette_fields.iter().map(|f| f.read(cx).value().to_string()).collect();
        self.plan = Some(match kind {
            "mutation" => format!(
                "{} Plan for {name}({})\n\n--- a/eng/rust.md\n+++ b/eng/rust.md\n@@ -6 +6 @@\n-- [[eng/rust/async-notes]]\n+- [[{}]]\n\nwarnings: none{}",
                if self.dry_run { "DRY RUN:" } else { "APPLIED:" },
                args.join(", "),
                args.get(1).cloned().unwrap_or_default(),
                if self.dry_run { "\n(nothing written; untick dry run to apply)" } else { "" }
            ),
            _ => format!("{name}({}) →\n{{ \"stub\": \"JSON output rendered generically\" }}", args.join(", ")),
        });
        cx.notify();
    }
}

fn for_preview(src: &str) -> String {
    let mut body = src;
    if let Some(rest) = src.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            body = &rest[end + 5..];
        }
    }
    let mut out = String::new();
    let mut rest = body;
    while let Some(i) = rest.find("[[") {
        out.push_str(&rest[..i]);
        rest = &rest[i + 2..];
        match rest.find("]]") {
            Some(j) => {
                let target = &rest[..j];
                let (path, text) = target.split_once('|').unwrap_or((target, target));
                out.push_str(&format!("[{text}](wiki:{path})"));
                rest = &rest[j + 2..];
            }
            None => out.push_str("[["),
        }
    }
    out.push_str(rest);
    out
}

fn page_tree_items(pages: &[Page]) -> Vec<TreeItem> {
    // Build the folder tree from Page Paths; folders without a Page are Placeholders.
    #[derive(Default)]
    struct Node(BTreeMap<String, Node>);
    let mut root = Node::default();
    for p in pages {
        let mut n = &mut root;
        for seg in p.path.split('/') {
            n = n.0.entry(seg.to_string()).or_default();
        }
    }
    fn build(prefix: &str, n: &Node, pages: &[Page]) -> Vec<TreeItem> {
        n.0.iter()
            .map(|(seg, child)| {
                let path = if prefix.is_empty() { seg.clone() } else { format!("{prefix}/{seg}") };
                let label = match pages.iter().find(|p| p.path == path) {
                    Some(p) => p.title.to_string(),
                    None => format!("{seg}  · placeholder"),
                };
                TreeItem::new(path.clone(), label)
                    .expanded(true)
                    .children(build(&path, child, pages))
            })
            .collect()
    }
    build("", &root, pages)
}

fn tag_tree_items(pages: &[Page]) -> Vec<TreeItem> {
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for p in pages {
        for t in &p.tags {
            let segs: Vec<&str> = t.split('/').collect();
            for i in 1..=segs.len() {
                let e = counts.entry(segs[..i].join("/")).or_default();
                e.1 += 1;
                if i == segs.len() {
                    e.0 += 1;
                }
            }
        }
    }
    fn build(prefix: &str, counts: &BTreeMap<String, (usize, usize)>) -> Vec<TreeItem> {
        counts
            .iter()
            .filter(|(k, _)| {
                let parent = k.rsplit_once('/').map(|(a, _)| a).unwrap_or("");
                parent == prefix
            })
            .map(|(k, (d, i))| {
                let leaf = k.rsplit('/').next().unwrap();
                TreeItem::new(format!("tag:{k}"), format!("#{leaf}   {d} / {i}"))
                    .expanded(true)
                    .children(build(k, counts))
            })
            .collect()
    }
    build("", &counts)
}

// ------------------------------------------------------------- rendering

impl Proto {
    fn tree_view(&self, state: &Entity<TreeState>, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        tree(state, move |ix, entry, selected, _window, cx| {
            let item = entry.item().clone();
            let is_current = view.read(cx).current == item.id.as_ref();
            let placeholder = item.label.contains("placeholder");
            ListItem::new(ix)
                .w_full()
                .py_0p5()
                .pl(px(14.) * entry.depth() + px(8.))
                .selected(selected || is_current)
                .child(
                    div()
                        .text_sm()
                        .when(placeholder, |d| d.italic().text_color(cx.theme().muted_foreground))
                        .child(item.label.clone()),
                )
                .on_click({
                    let view = view.clone();
                    move |_, window, cx| {
                        let id = item.id.to_string();
                        if !id.starts_with("tag:") && !placeholder {
                            view.update(cx, |this, cx| this.open(&id, window, cx));
                        }
                    }
                })
        })
    }

    fn editor_el(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .font_family(cx.theme().mono_font_family.clone())
            .child(Editor::new(&self.editor).h_full().p_2().border_0())
    }

    fn preview_el(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // gpui-kit's renderer knows neither frontmatter-as-metadata nor [[wikilinks]]:
        // the real app needs MarkdownPlugins for both. Faked here by rewriting the source.
        let src = for_preview(&self.source(cx));
        self.preview.update(cx, |st, cx| st.set_text(&src, cx));
        div()
            .id("preview")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .child(TextView::new(&self.preview))
    }

    fn section(title: &str, cx: &App) -> Div {
        div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(cx.theme().muted_foreground)
            .mt_3()
            .mb_1()
            .child(title.to_uppercase())
    }

    fn backlinks_el(&self, cx: &mut Context<Self>) -> Div {
        let view = cx.entity();
        let bl = self.backlinks(self.current);
        v_flex().gap_1().children(if bl.is_empty() {
            vec![div().text_sm().text_color(cx.theme().muted_foreground).child("No Backlinks").into_any_element()]
        } else {
            bl.into_iter()
                .map(|p| {
                    let view = view.clone();
                    let path = p.path;
                    div()
                        .id(SharedString::from(format!("bl-{path}")))
                        .text_sm()
                        .text_color(cx.theme().link)
                        .cursor_pointer()
                        .child(format!("{}  ·  {}", p.title, p.path))
                        .on_click(move |_, window, cx| view.update(cx, |this, cx| this.open(path, window, cx)))
                        .into_any_element()
                })
                .collect()
        })
    }

    fn outgoing_el(&self, cx: &mut Context<Self>) -> Div {
        let view = cx.entity();
        let links = links_of(&self.source(cx));
        v_flex().gap_1().children(links.into_iter().map(|l| {
            let broken = self.page(&l).is_none();
            let view = view.clone();
            div()
                .id(SharedString::from(format!("ol-{l}")))
                .text_sm()
                .cursor_pointer()
                .text_color(if broken { cx.theme().danger } else { cx.theme().link })
                .child(if broken { format!("{l}  (broken)") } else { l.clone() })
                .on_click(move |_, window, cx| view.update(cx, |this, cx| this.open(&l, window, cx)))
        }))
    }

    fn outline_el(&self, cx: &mut Context<Self>) -> Div {
        v_flex().gap_0p5().children(outline_of(&self.source(cx)).into_iter().map(|(lvl, t)| {
            div().text_sm().pl(px(10. * (lvl as f32 - 1.))).child(t)
        }))
    }

    fn tags_el(&self, cx: &mut Context<Self>) -> Div {
        let tags = self.page(self.current).map(|p| p.tags).unwrap_or_default();
        h_flex().gap_1().flex_wrap().children(tags.into_iter().map(|t| {
            div()
                .px_2()
                .rounded_full()
                .bg(cx.theme().secondary)
                .text_xs()
                .child(format!("#{t}"))
        }))
    }

    fn breadcrumb(&self, cx: &App) -> Div {
        let mut parts = vec![];
        let mut acc = String::new();
        for seg in self.current.split('/') {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(seg);
            parts.push(self.page(&acc).map(|p| p.title.to_string()).unwrap_or(seg.to_string()));
        }
        h_flex()
            .gap_1()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(parts.join("  /  "))
            .when(self.dirty(cx), |d| d.child(div().text_color(cx.theme().danger).child("●  unsaved")))
    }

    fn banner_el(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let b = self.banner.as_ref()?;
        Some(match b {
            Banner::Info(msg) => h_flex()
                .px_3()
                .py_1()
                .bg(cx.theme().secondary)
                .text_sm()
                .justify_between()
                .child(msg.clone())
                .child(Button::new("dismiss").label("×").ghost().xsmall().on_click(cx.listener(|this, _, _, cx| {
                    this.banner = None;
                    cx.notify();
                })))
                .into_any_element(),
            Banner::ChangedOnDisk { disk } => {
                let disk = disk.clone();
                let disk2 = disk.clone();
                h_flex()
                    .px_3()
                    .py_1p5()
                    .gap_2()
                    .bg(cx.theme().danger)
                    .text_color(cx.theme().danger_foreground)
                    .text_sm()
                    .child(div().flex_1().child("This Page changed on disk while you have unsaved edits."))
                    .child(Button::new("reload").label("Reload").small().on_click(cx.listener(move |this, _, window, cx| {
                        this.saved = disk.clone();
                        let d = disk.clone();
                        this.editor.update(cx, |st, cx| st.set_value(d, window, cx));
                        this.banner = None;
                        this.compare = false;
                        cx.notify();
                    })))
                    .child(Button::new("keep").label("Keep mine (overwrite)").small().on_click(cx.listener(move |this, _, _, cx| {
                        this.saved = disk2.clone(); // adopt disk as base_version, then save ours over it
                        this.compare = false;
                        this.save(cx);
                    })))
                    .child(Button::new("compare").label("Compare").small().on_click(cx.listener(|this, _, _, cx| {
                        this.compare = !this.compare;
                        cx.notify();
                    })))
                    .into_any_element()
            }
        })
    }

    fn compare_el(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Banner::ChangedOnDisk { disk }) = &self.banner else { return None };
        if !self.compare {
            return None;
        }
        let col = |title: &str, body: String, cx: &App| {
            v_flex()
                .flex_1()
                .p_2()
                .border_1()
                .border_color(cx.theme().border)
                .child(Self::section(title, cx))
                .child(div().font_family(cx.theme().mono_font_family.clone()).text_xs().child(body))
        };
        Some(
            h_flex()
                .h(px(220.))
                .gap_2()
                .p_2()
                .child(col("Mine (unsaved)", self.source(cx), cx))
                .child(col("On disk", disk.clone(), cx))
                .into_any_element(),
        )
    }

    // ---- Variant A: Workbench
    fn render_workbench(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let left = v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .p_1()
                    .gap_1()
                    .child(Button::new("tab-pages").label("Pages").small().when(!self.left_tab_tags, |b| b.primary()).on_click(cx.listener(|t, _, _, cx| { t.left_tab_tags = false; cx.notify(); })))
                    .child(Button::new("tab-tags").label("Tags").small().when(self.left_tab_tags, |b| b.primary()).on_click(cx.listener(|t, _, _, cx| { t.left_tab_tags = true; cx.notify(); }))),
            )
            .child(div().flex_1().child(if self.left_tab_tags {
                self.tree_view(&self.tag_tree.clone(), cx).into_any_element()
            } else {
                self.tree_view(&self.page_tree.clone(), cx).into_any_element()
            }));

        let centre = v_flex()
            .size_full()
            .child(h_flex().px_3().py_1().justify_between().child(self.breadcrumb(cx)).child(
                Button::new("mode").label(if self.show_preview { "Source only  ⌘E" } else { "Split  ⌘E" }).ghost().xsmall()
                    .on_click(cx.listener(|t, _, _, cx| { t.show_preview = !t.show_preview; cx.notify(); })),
            ))
            .children(self.banner_el(cx))
            .children(self.compare_el(cx))
            .child(div().flex_1().child(if self.show_preview {
                h_resizable("split")
                    .child(resizable_panel().child(self.editor_el(cx)))
                    .child(resizable_panel().child(self.preview_el(cx)))
                    .into_any_element()
            } else {
                self.editor_el(cx).into_any_element()
            }));

        let right = v_flex()
            .id("right")
            .size_full()
            .p_3()
            .overflow_y_scroll()
            .child(Self::section("Backlinks", cx))
            .child(self.backlinks_el(cx))
            .child(Self::section("Links", cx))
            .child(self.outgoing_el(cx))
            .child(Self::section("Outline", cx))
            .child(self.outline_el(cx))
            .child(Self::section("Tags", cx))
            .child(self.tags_el(cx));

        h_resizable("workbench")
            .child(resizable_panel().size(px(240.)).child(left))
            .child(resizable_panel().child(centre))
            .child(resizable_panel().size(px(260.)).child(right))
            .into_any_element()
    }

    // ---- Variant B: Reader
    fn render_reader(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let space = self.reader_space;
        let space_pages: Vec<Page> = self.pages.borrow().iter().filter(|p| p.path == space || p.path.starts_with(&format!("{space}/"))).cloned().collect();
        self.page_tree.update(cx, |st, cx| st.set_items(page_tree_items(&space_pages), cx));

        let top = h_flex()
            .px_3()
            .py_2()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(div().font_weight(FontWeight::BOLD).child("wikirs"))
            .children(["eng", "personal"].into_iter().map(|s| {
                Button::new(SharedString::from(format!("space-{s}")))
                    .label(s)
                    .small()
                    .when(s == space, |b| b.primary())
                    .when(s != space, |b| b.ghost())
                    .on_click(cx.listener(move |t, _, window, cx| {
                        t.reader_space = s;
                        t.open(s, window, cx);
                    }))
            }))
            .child(div().flex_1())
            .child(
                Button::new("search").label("Search / open…  ⌘P").ghost().small()
                    .on_click(cx.listener(|t, _, window, cx| t.open_palette(PaletteMode::QuickOpen, window, cx))),
            );

        let editing = !self.show_preview;
        let title = self.page(self.current).map(|p| p.title).unwrap_or("");
        let main = v_flex()
            .id("reader-main")
            .size_full()
            .overflow_y_scroll()
            .px_8()
            .py_4()
            .child(self.breadcrumb(cx))
            .child(
                h_flex()
                    .justify_between()
                    .child(div().text_2xl().font_weight(FontWeight::BOLD).child(title))
                    .child(
                        Button::new("edit").label(if editing { "Done  ⌘E" } else { "Edit  ⌘E" }).small().when(!editing, |b| b.primary())
                            .on_click(cx.listener(|t, _, _, cx| { t.show_preview = !t.show_preview; cx.notify(); })),
                    ),
            )
            .child(self.tags_el(cx))
            .children(self.banner_el(cx))
            .children(self.compare_el(cx))
            .child(
                div().mt_2().h(px(520.)).border_1().border_color(cx.theme().border).rounded(cx.theme().radius)
                    .child(if editing { self.editor_el(cx).into_any_element() } else { self.preview_el(cx).into_any_element() }),
            )
            .child(Self::section("Linked from", cx))
            .child(self.backlinks_el(cx))
            .child(Self::section("Child Pages", cx))
            .child(
                v_flex().children(
                    self.pages.borrow().iter()
                        .filter(|p| p.path.starts_with(&format!("{}/", self.current)))
                        .map(|p| div().text_sm().child(format!("{}  ·  {}", p.title, p.path))),
                ),
            );

        v_flex()
            .size_full()
            .child(top)
            .child(
                h_resizable("reader")
                    .child(resizable_panel().size(px(240.)).child(
                        v_flex().size_full().bg(cx.theme().sidebar).p_1()
                            .child(Self::section(&format!("{space} Space"), cx))
                            .child(div().flex_1().child(self.tree_view(&self.page_tree.clone(), cx))),
                    ))
                    .child(resizable_panel().child(main)),
            )
            .into_any_element()
    }

    // ---- Variant C: Focus
    fn render_focus(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let bl = self.backlinks(self.current).len();
        let tags = self.page(self.current).map(|p| p.tags.join(" #")).unwrap_or_default();
        let overlay_preview = !self.show_preview; // Focus starts in source
        v_flex()
            .size_full()
            .child(h_flex().justify_center().py_2().child(self.breadcrumb(cx)))
            .children(self.banner_el(cx))
            .children(self.compare_el(cx))
            .child(
                h_flex().flex_1().justify_center().child(
                    div().w(px(760.)).h_full().child(if overlay_preview {
                        self.preview_el(cx).into_any_element()
                    } else {
                        self.editor_el(cx).into_any_element()
                    }),
                ),
            )
            .child(
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_4()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{bl} Backlinks"))
                    .child(if tags.is_empty() { "no Tags".to_string() } else { format!("#{tags}") })
                    .child(div().flex_1())
                    .child(if overlay_preview { "preview  ⌘E" } else { "source  ⌘E" })
                    .child("⌘P open   ⌘K commands   ⌘S save"),
            )
            .into_any_element()
    }

    fn palette_el(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mode = self.palette?;
        let q = self.palette_input.read(cx).value().to_lowercase();
        let view = cx.entity();
        let body: AnyElement = match (mode, self.palette_op) {
            (PaletteMode::QuickOpen, _) => v_flex()
                .children(
                    self.pages.borrow().iter()
                        .filter(|p| p.path.contains(&q) || p.title.to_lowercase().contains(&q))
                        .map(|p| {
                            let path = p.path;
                            let view = view.clone();
                            ListItem::new(SharedString::from(format!("qo-{path}")))
                                .child(h_flex().gap_3().child(p.title).child(div().text_xs().text_color(cx.theme().muted_foreground).child(path)))
                                .on_click(move |_, window, cx| view.update(cx, |t, cx| { t.palette = None; t.open(path, window, cx) }))
                        }),
                )
                .into_any_element(),
            (PaletteMode::Commands, None) => v_flex()
                .id("ops")
                .max_h(px(420.))
                .overflow_y_scroll()
                .children(OPS.iter().enumerate().filter(|(_, (n, _, _))| n.contains(&q.replace(' ', "_"))).map(|(ix, (n, k, ins))| {
                    let view = view.clone();
                    ListItem::new(ix)
                        .child(
                            h_flex().gap_3()
                                .child(div().w(px(160.)).child(*n))
                                .child(div().w(px(90.)).text_xs().text_color(cx.theme().muted_foreground).child(*k))
                                .child(div().text_xs().text_color(cx.theme().muted_foreground).child(ins.join(", "))),
                        )
                        .on_click(move |_, window, cx| view.update(cx, |t, cx| t.pick_op(ix, window, cx)))
                }))
                .into_any_element(),
            (PaletteMode::Commands, Some(ix)) => {
                let (name, kind, _) = OPS[ix];
                v_flex()
                    .gap_2()
                    .child(h_flex().gap_2().child(div().font_weight(FontWeight::BOLD).child(name)).child(div().text_xs().child(kind)))
                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Form generated from the Input schema"))
                    .children(self.palette_fields.iter().map(|f| Input::new(f).small()))
                    .when(kind == "mutation", |d| {
                        d.child(
                            Button::new("dry").label(if self.dry_run { "☑ dry run (show Plan)" } else { "☐ dry run" }).small().ghost()
                                .on_click(cx.listener(|t, _, _, cx| { t.dry_run = !t.dry_run; cx.notify(); })),
                        )
                    })
                    .when(kind == "subscription", |d| d.child(div().text_xs().child("Opens a live event log pane in the real app.")))
                    .child(h_flex().gap_2()
                        .child(Button::new("run").label("Run").primary().small().on_click(cx.listener(|t, _, _, cx| t.run_op(cx))))
                        .child(Button::new("back").label("Back").ghost().small().on_click(cx.listener(|t, _, _, cx| { t.palette_op = None; t.plan = None; cx.notify(); }))))
                    .children(self.plan.clone().map(|p| {
                        div().p_2().bg(cx.theme().secondary).rounded(cx.theme().radius).font_family(cx.theme().mono_font_family.clone()).text_xs().child(p)
                    }))
                    .into_any_element()
            }
        };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(hsla(0., 0., 0., 0.25))
                .flex()
                .justify_center()
                .child(
                    v_flex()
                        .mt(px(80.))
                        .w(px(640.))
                        .h_auto()
                        .p_2()
                        .gap_2()
                        .bg(cx.theme().popover)
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded(cx.theme().radius_lg)
                        .shadow_lg()
                        .child(Input::new(&self.palette_input))
                        .child(body),
                )
                .into_any_element(),
        )
    }

    fn switcher_el(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div().absolute().bottom(px(12.)).left_0().w_full().flex().justify_center().child(
            h_flex()
                .gap_2()
                .px_3()
                .py_1()
                .rounded_full()
                .bg(hsla(0.6, 0.9, 0.96, 0.97))
                .border_2()
                .border_color(hsla(0.6, 0.9, 0.5, 1.))
                .text_color(hsla(0., 0., 0.05, 1.))
                .shadow_lg()
                .text_sm()
                .child(Button::new("prev").label("←").xsmall().ghost().on_click(cx.listener(|t, _, _, cx| { t.variant = (t.variant + 2) % 3; cx.notify(); })))
                .child(div().w(px(120.)).flex().justify_center().child(self.v().name()))
                .child(Button::new("next").label("→").xsmall().ghost().on_click(cx.listener(|t, _, _, cx| { t.variant = (t.variant + 1) % 3; cx.notify(); })))
                .child(div().w(px(1.)).h(px(16.)).bg(hsla(0., 0., 1., 0.3)))
                .child(Button::new("ext").label("simulate external edit").xsmall().ghost().on_click(cx.listener(|t, _, window, cx| t.simulate_external_edit(window, cx)))),
        )
    }
}

impl Render for Proto {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.v() != Variant::Reader {
            let items = page_tree_items(&self.pages.borrow());
            self.page_tree.update(cx, |st, cx| st.set_items(items, cx));
        }
        let body = match self.v() {
            Variant::Workbench => self.render_workbench(cx),
            Variant::Reader => self.render_reader(cx),
            Variant::Focus => self.render_focus(cx),
        };
        div()
            .id("proto")
            .key_context("Proto")
            .size_full()
            .relative()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|t, _: &TogglePalette, window, cx| t.open_palette(PaletteMode::Commands, window, cx)))
            .on_action(cx.listener(|t, _: &QuickOpen, window, cx| t.open_palette(PaletteMode::QuickOpen, window, cx)))
            .on_action(cx.listener(|t, _: &ClosePalette, _, cx| { t.palette = None; cx.notify(); }))
            .on_action(cx.listener(|t, _: &ToggleMode, _, cx| { t.show_preview = !t.show_preview; cx.notify(); }))
            .on_action(cx.listener(|t, _: &Save, _, cx| t.save(cx)))
            .on_action(cx.listener(|t, _: &NextVariant, _, cx| { t.variant = (t.variant + 1) % 3; cx.notify(); }))
            .on_action(cx.listener(|t, _: &PrevVariant, _, cx| { t.variant = (t.variant + 2) % 3; cx.notify(); }))
            .child(body)
            .children(self.palette_el(cx))
            .child(self.switcher_el(cx))
    }
}

fn main() {
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-k", TogglePalette, None),
            KeyBinding::new("cmd-p", QuickOpen, None),
            KeyBinding::new("escape", ClosePalette, Some("Proto")),
            KeyBinding::new("cmd-e", ToggleMode, None),
            KeyBinding::new("cmd-s", Save, None),
            KeyBinding::new("cmd-alt-right", NextVariant, None),
            KeyBinding::new("cmd-alt-left", PrevVariant, None),
        ]);
        let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
        cx.open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            |window, cx| {
                let view = cx.new(|cx| Proto::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("open window");
        cx.activate(true);
    });
}
