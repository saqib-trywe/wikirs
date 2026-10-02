//! The Workbench window (docs/spec/gui.md#layout-workbench-prototype-variant-a-as-the-base):
//! Pages/Tags sidebar | breadcrumb, banner and the Page | Backlinks, Links,
//! Outline and Tags. A Page opens rendered; ⌘E shows source and preview side
//! by side; ⌘S saves.
//!
//! The open Page's behaviour is the [`Session`] the TUI shares; this holds only
//! the GUI's view state.

use std::{rc::Rc, time::Duration};

use gpui_kit::{
    component::{
        ActiveTheme as _, Sizable as _,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Editor, EditorState, InputEvent},
        list::ListItem,
        resizable::{h_resizable, resizable_panel},
        tree::{TreeItem, TreeState, tree},
        v_flex,
    },
    prelude::FluentBuilder as _,
    *,
};
use wikirs_core::{
    Operation as _, Wiki,
    ops::{TagNode, TagTree, TagTreeInput},
};
use wikirs_ui::{Followed, Notice, Session};

use crate::{
    layout::{self, View},
    page::PageRenderer,
};

actions!(wikirs, [Save, ToggleSource]);

/// How often watch events are taken in.
const POLL: Duration = Duration::from_millis(250);

pub struct Workbench {
    pub session: Session,
    editor: Entity<EditorState>,
    page_tree: Entity<TreeState>,
    tag_tree: Entity<TreeState>,
    tags_tab: bool,
    /// ⌘E: source and preview side by side, instead of rendered only.
    pub source: bool,
    /// Changed on disk: mine and disk side by side.
    pub compare: bool,
    scroll: ScrollHandle,
    /// Holds the keyboard focus when no input does, so ⌘S and ⌘E always work.
    focus: FocusHandle,
    /// A file to open in the system viewer (tests read it instead).
    pub opened_externally: Option<std::path::PathBuf>,
    _subscriptions: Vec<Subscription>,
    _poll: Option<Task<()>>,
}

impl Workbench {
    /// A Workbench on `wiki`. With `watch`, other writers' changes arrive too
    /// (tests leave it off and send events themselves).
    pub fn new(wiki: Wiki, watch: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut session = Session::new(wiki);
        if watch {
            // Without a watcher only this process's own changes show up.
            let _ = session.start_watcher();
        }
        let text = session
            .page
            .as_ref()
            .map(|p| p.buffer.clone())
            .unwrap_or_default();
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("markdown")
                .line_number(false)
                .soft_wrap(true)
                .default_value(text)
        });
        let page_tree = cx.new(|cx| TreeState::new(cx));
        let tag_tree = cx.new(|cx| TreeState::new(cx));
        let subscriptions = vec![
            cx.subscribe(&editor, |this, editor, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = editor.read(cx).value().to_string();
                    this.session.set_buffer(text);
                    cx.notify();
                }
            }),
        ];
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let alive = this.update_in(cx, |this, window, cx| {
                    if this.session.pump() {
                        this.changed(window, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut this = Self {
            session,
            editor,
            page_tree,
            tag_tree,
            tags_tab: false,
            source: false,
            compare: false,
            scroll: ScrollHandle::new(),
            focus,
            opened_externally: None,
            _subscriptions: subscriptions,
            _poll: watch.then_some(poll),
        };
        this.refresh_trees(cx);
        this
    }

    // -------------------------------------------------------------- actions

    /// Opens a Page.
    pub fn open(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.open(path) {
            self.scroll.scroll_to_item(0);
            self.compare = false;
        }
        self.changed(window, cx);
    }

    /// Follows Link `i` of the open Page.
    pub fn follow(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.session.follow(i) {
            Followed::Opened { heading } => {
                self.changed(window, cx);
                // Applied at the next layout, which is the new Page's.
                let block = heading.and_then(|h| self.block_of(&h)).unwrap_or(0);
                self.scroll.scroll_to_top_of_item(block);
            }
            Followed::OpenExternal(file) => {
                wikirs_ui::open_externally(&file);
                self.opened_externally = Some(file);
            }
            Followed::Stayed => self.changed(window, cx),
        }
    }

    /// How far the Page is scrolled down, in pixels.
    #[must_use]
    pub fn scrolled(&self) -> Pixels {
        -self.scroll.offset().y
    }

    /// The top-level block of the open Page that is, or holds, `heading`.
    fn block_of(&self, heading: &str) -> Option<usize> {
        let page = self.session.page.as_ref()?;
        let anchor = wikirs_core::markdown::anchor(heading);
        layout::layout(&page.doc, &page.resolved)
            .0
            .iter()
            .position(|view| anchors_in(view).contains(&anchor))
    }

    /// Puts the keyboard focus in the source editor.
    pub fn focus_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.focus(window, cx));
    }

    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.save();
        self.changed(window, cx);
    }

    /// Creates the Page a Broken Link or Placeholder pointed at.
    pub fn create(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.session.create(path);
        self.changed(window, cx);
    }

    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.reload_from_disk();
        self.changed(window, cx);
    }

    pub fn keep_mine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.keep_mine();
        self.changed(window, cx);
    }

    /// The Session changed: bring the editor, trees and anchors into line.
    pub fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let buffer = self.session.page.as_ref().map(|p| p.buffer.clone());
        if let Some(buffer) = buffer
            && self.editor.read(cx).value().as_ref() != buffer
        {
            self.editor
                .update(cx, |editor, cx| editor.set_value(buffer, window, cx));
        }
        if !matches!(self.session.notice, Some(Notice::ChangedOnDisk { .. })) {
            self.compare = false;
        }
        self.refresh_trees(cx);
        cx.notify();
    }

    fn refresh_trees(&mut self, cx: &mut Context<Self>) {
        let pages = page_items(&self.session.tree);
        self.page_tree
            .update(cx, |tree, cx| tree.set_items(pages, cx));
        let tags = TagTree::run(
            self.session.wiki(),
            TagTreeInput {
                scope: wikirs_core::index::Scope::default(),
            },
        )
        .map(|out| tag_items(&out.tags))
        .unwrap_or_default();
        self.tag_tree
            .update(cx, |tree, cx| tree.set_items(tags, cx));
    }

    // ------------------------------------------------------------ rendering

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity();
        let state = if self.tags_tab {
            self.tag_tree.clone()
        } else {
            self.page_tree.clone()
        };
        let current = self.session.page.as_ref().map(|p| p.path.clone());
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .p_1()
                    .gap_1()
                    .child(
                        Button::new("tab-pages")
                            .label("Pages")
                            .small()
                            .when(!self.tags_tab, ButtonVariants::primary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tags_tab = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("tab-tags")
                            .label("Tags")
                            .small()
                            .when(self.tags_tab, ButtonVariants::primary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tags_tab = true;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .child(tree(&state, move |ix, entry, selected, _, cx| {
                        let item = entry.item().clone();
                        let id = item.id.to_string();
                        let placeholder = id.starts_with("placeholder:");
                        let path = id.trim_start_matches("placeholder:").to_string();
                        let is_current = current.as_deref() == Some(path.as_str());
                        ListItem::new(ix)
                            .w_full()
                            .py_0p5()
                            .pl(px(14.) * entry.depth() + px(8.))
                            .selected(selected || is_current)
                            .child(
                                div()
                                    .text_sm()
                                    .when(placeholder, |d| {
                                        d.italic().text_color(cx.theme().muted_foreground)
                                    })
                                    .child(item.label.clone()),
                            )
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    if id.starts_with("tag:") {
                                        return;
                                    }
                                    view.update(cx, |this, cx| {
                                        if placeholder {
                                            this.session.notice =
                                                Some(Notice::Create(path.clone()));
                                            cx.notify();
                                        } else {
                                            this.open(&path, window, cx);
                                        }
                                    });
                                }
                            })
                    })),
            )
            .into_any_element()
    }

    fn breadcrumb(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let page = self.session.page.as_ref();
        h_flex()
            .px_3()
            .py_1()
            .justify_between()
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(self.session.breadcrumb().join("  /  "))
                    .when(page.is_some_and(wikirs_ui::OpenPage::dirty), |d| {
                        d.child(div().text_color(theme.danger).child("●  unsaved"))
                    })
                    .when(page.is_some_and(|p| p.deleted), |d| {
                        d.child(div().text_color(theme.danger).child("deleted on disk"))
                    }),
            )
            .child(
                Button::new("mode")
                    .label(if self.source {
                        "Rendered  ⌘E"
                    } else {
                        "Source  ⌘E"
                    })
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.source = !this.source;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notice = self.session.notice.clone()?;
        let theme = cx.theme();
        let dismiss = Button::new("dismiss")
            .label("×")
            .ghost()
            .xsmall()
            .on_click(cx.listener(|this, _, _, cx| {
                this.session.notice = None;
                cx.notify();
            }));
        let bar = |bg, fg| {
            h_flex()
                .px_3()
                .py_1()
                .gap_2()
                .bg(bg)
                .text_color(fg)
                .text_sm()
        };
        Some(match notice {
            Notice::Info(message) => bar(theme.secondary, theme.foreground)
                .child(div().flex_1().child(message))
                .child(dismiss)
                .into_any_element(),
            Notice::Error(message) => bar(theme.danger, theme.danger_foreground)
                .child(div().flex_1().child(message))
                .child(dismiss)
                .into_any_element(),
            Notice::Create(path) => bar(theme.warning, theme.warning_foreground)
                .child(div().flex_1().child(format!("No Page `{path}`.")))
                .child(
                    Button::new("create")
                        .label("Create it")
                        .small()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.create(&path, window, cx);
                        })),
                )
                .child(dismiss)
                .into_any_element(),
            Notice::ChangedOnDisk { .. } => bar(theme.danger, theme.danger_foreground)
                .child(
                    div()
                        .flex_1()
                        .child("This Page changed on disk while you have unsaved edits."),
                )
                .child(
                    Button::new("reload")
                        .label("Reload")
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
                )
                .child(
                    Button::new("keep")
                        .label("Keep mine")
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| this.keep_mine(window, cx))),
                )
                .child(
                    Button::new("compare")
                        .label("Compare")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.compare = !this.compare;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        })
    }

    fn compare_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Notice::ChangedOnDisk { disk, .. }) = &self.session.notice else {
            return None;
        };
        if !self.compare {
            return None;
        }
        let theme = cx.theme();
        let mine = self
            .session
            .page
            .as_ref()
            .map(|p| p.buffer.clone())
            .unwrap_or_default();
        let column = |title: &'static str, text: String| {
            v_flex()
                .flex_1()
                .p_2()
                .border_1()
                .border_color(theme.border)
                .child(section(title, cx))
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .text_xs()
                        .child(text),
                )
        };
        Some(
            h_flex()
                .h(px(240.))
                .gap_2()
                .p_2()
                .child(column("Mine (unsaved)", mine))
                .child(column("On disk", disk.clone()))
                .into_any_element(),
        )
    }

    /// The rendered Page, scrollable, with clickable Links.
    fn rendered(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(page) = &self.session.page else {
            return div()
                .p_4()
                .child("No Pages yet: create one from the palette (⌘K).")
                .into_any_element();
        };
        let (views, notes) = layout::layout(&page.doc, &page.resolved);
        let view = cx.entity();
        let mut renderer = PageRenderer::new(Rc::new(move |link, window, cx| {
            view.update(cx, |this, cx| this.follow(link, window, cx));
        }));
        let mut blocks = renderer.views(&views, cx);
        if !notes.is_empty() {
            blocks.push(
                div()
                    .my_3()
                    .h(px(1.))
                    .bg(cx.theme().border)
                    .into_any_element(),
            );
            for (label, note) in &notes {
                blocks.push(
                    h_flex()
                        .items_start()
                        .gap_2()
                        .text_sm()
                        .child(format!("[^{label}]"))
                        .child(v_flex().flex_1().children(renderer.views(note, cx)))
                        .into_any_element(),
                );
            }
        }
        div()
            .id("page")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px_6()
            .py_4()
            .children(blocks)
            .into_any_element()
    }

    fn centre(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let body = if self.source {
            let editor = div()
                .size_full()
                .font_family(cx.theme().mono_font_family.clone())
                .child(Editor::new(&self.editor).h_full().p_2().border_0());
            h_resizable("split")
                .child(resizable_panel().child(editor))
                .child(resizable_panel().child(self.rendered(cx)))
                .into_any_element()
        } else {
            self.rendered(cx)
        };
        v_flex()
            .size_full()
            .child(self.breadcrumb(cx))
            .children(self.banner(cx))
            .children(self.compare_view(cx))
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    fn right(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let view = cx.entity();
        let mut panel = v_flex()
            .id("right")
            .size_full()
            .p_3()
            .overflow_y_scroll()
            .child(section("Backlinks", cx));
        let Some(page) = &self.session.page else {
            return panel.into_any_element();
        };
        if page.backlinks.is_empty() {
            panel = panel.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("None"),
            );
        }
        for from in &page.backlinks {
            let (view, from) = (view.clone(), from.clone());
            panel = panel.child(
                div()
                    .id(SharedString::from(format!("backlink-{from}")))
                    .text_sm()
                    .text_color(theme.link)
                    .cursor_pointer()
                    .child(self.session.title_of(&from))
                    .on_click(move |_, window, cx| {
                        view.update(cx, |this, cx| this.open(&from, window, cx));
                    }),
            );
        }
        panel = panel.child(section("Links", cx));
        for (i, link) in page.doc.links.iter().enumerate() {
            let resolved = page.resolved.get(i).and_then(Option::as_ref);
            let broken = layout::is_broken(resolved);
            let target = resolved.map_or(link.target.clone(), |r| r.target.clone());
            let view = view.clone();
            panel = panel.child(
                div()
                    .id(("link", i))
                    .debug_selector(move || format!("link-{i}"))
                    .text_sm()
                    .cursor_pointer()
                    .text_color(if broken { theme.danger } else { theme.link })
                    .child(if broken {
                        format!("{target}  (broken)")
                    } else {
                        target
                    })
                    .on_click(move |_, window, cx| {
                        view.update(cx, |this, cx| this.follow(i, window, cx));
                    }),
            );
        }
        panel = panel.child(section("Outline", cx));
        for (level, text) in &page.outline {
            panel = panel.child(
                div()
                    .text_sm()
                    .pl(px(10. * f32::from(level.saturating_sub(1))))
                    .child(text.clone()),
            );
        }
        panel
            .child(section("Tags", cx))
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .children(page.tags.iter().map(|t| {
                        div()
                            .px_2()
                            .rounded_full()
                            .bg(theme.secondary)
                            .text_xs()
                            .child(format!("#{t}"))
                    })),
            )
            .into_any_element()
    }
}

impl Render for Workbench {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self.sidebar(cx);
        let centre = self.centre(cx);
        let right = self.right(cx);
        div()
            .id("workbench")
            .key_context("Workbench")
            .track_focus(&self.focus)
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSource, _, cx| {
                this.source = !this.source;
                cx.notify();
            }))
            .child(
                h_resizable("workbench")
                    .child(resizable_panel().size(px(240.)).child(sidebar))
                    .child(resizable_panel().child(centre))
                    .child(resizable_panel().size(px(260.)).child(right)),
            )
    }
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

/// Heading anchors in a view, nested ones included.
fn anchors_in(view: &View) -> Vec<String> {
    match view {
        View::Heading { anchor, .. } => vec![anchor.clone()],
        View::Quote(children) | View::Alert { children, .. } => {
            children.iter().flat_map(anchors_in).collect()
        }
        View::List(items) => items
            .iter()
            .flat_map(|(_, blocks)| blocks.iter().flat_map(anchors_in))
            .collect(),
        _ => Vec::new(),
    }
}

/// The Page tree as tree items; a Placeholder's id is `placeholder:<path>`.
fn page_items(rows: &[wikirs_ui::TreeRow]) -> Vec<TreeItem> {
    fn build(rows: &[wikirs_ui::TreeRow], at: &mut usize, depth: usize) -> Vec<TreeItem> {
        let mut items = Vec::new();
        while let Some(row) = rows.get(*at) {
            if row.depth < depth {
                break;
            }
            *at += 1;
            let id = if row.placeholder {
                format!("placeholder:{}", row.path)
            } else {
                row.path.clone()
            };
            let children = build(rows, at, depth + 1);
            items.push(
                TreeItem::new(id, row.title.clone())
                    .expanded(true)
                    .children(children),
            );
        }
        items
    }
    build(rows, &mut 0, 0)
}

/// The Tag tree with direct and inclusive counts.
fn tag_items(nodes: &[TagNode]) -> Vec<TreeItem> {
    nodes
        .iter()
        .map(|n| {
            let leaf = n.tag.rsplit('/').next().unwrap_or(&n.tag);
            TreeItem::new(
                format!("tag:{}", n.tag),
                format!("#{leaf}   {} / {}", n.direct, n.inclusive),
            )
            .expanded(true)
            .children(tag_items(&n.children))
        })
        .collect()
}
