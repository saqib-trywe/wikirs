//! The Workbench's overlays (gui.md#custom-ui-beyond-the-palette): the ⌘K
//! palette and its generated forms (which are also the move, rename and delete
//! dialogs, showing the dry-run Plan before Apply), ⌘P quick open and search,
//! and the Pages carrying a Tag.

use gpui_kit::{
    component::{
        ActiveTheme as _, Sizable as _,
        button::{Button, ButtonVariants},
        h_flex,
        input::Input,
        list::ListItem,
        v_flex,
    },
    prelude::FluentBuilder as _,
    *,
};
use serde_json::{Value, json};
use wikirs_core::{Kind, Operation as _, watch::Watch};
use wikirs_ui::{
    Form, Session,
    outcome::{self, Kind as LineKind, OutcomeLine},
};

use crate::{
    form::{Change, FormPanel, outcome_view},
    workbench::Workbench,
};

actions!(wikirs, [OpenPalette, QuickOpen, CloseOverlay]);

pub enum Overlay {
    Palette,
    QuickOpen {
        all: Vec<(String, String)>,
    },
    TagPages {
        tag: String,
        pages: Vec<(String, String)>,
    },
    Form(Box<FormPanel>),
}

impl Workbench {
    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Palette);
        self.reset_query("Operation…", window, cx);
    }

    pub fn open_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::QuickOpen {
            all: self.session.all_pages(),
        });
        self.reset_query("Open a Page by path or Title, or search…", window, cx);
    }

    /// Lists the Pages carrying `tag` (`list_pages{tag}`).
    pub fn show_tag(&mut self, tag: &str, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::TagPages {
            tag: tag.to_string(),
            pages: self.session.pages_with_tag(tag),
        });
        cx.notify();
    }

    pub fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = None;
        self.focus_view(window, cx);
        cx.notify();
    }

    fn reset_query(
        &mut self,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.query.update(cx, |query, cx| {
            query.set_value("", window, cx);
            query.set_placeholder(placeholder, window, cx);
            query.focus(window, cx);
        });
        cx.notify();
    }

    fn query_text(&self, cx: &App) -> String {
        self.query.read(cx).value().to_string()
    }

    /// Opens `op`'s form, filled from `prefill`. A mutation starts as a dry run,
    /// so its Plan shows before anything is written.
    pub fn open_form(
        &mut self,
        op: &str,
        prefill: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Ok(mut form)) = Form::for_op(op) else {
            return;
        };
        form.fill(prefill);
        if form.dry_run.is_some() {
            form.dry_run = Some(true);
        }
        self.overlay = Some(Overlay::Form(Box::new(FormPanel::new(form, window, cx))));
        cx.notify();
    }

    /// The palette's pick: its form, filled with the open Page.
    fn pick(&mut self, op: &str, window: &mut Window, cx: &mut Context<Self>) {
        let prefill = self.session.page.as_ref().map_or(Value::Null, |p| {
            let p = &p.path;
            json!({ "page": p, "from": p, "target": p, "from_page": p })
        });
        self.open_form(op, &prefill, window, cx);
    }

    /// Runs the form (Run), or its dry run's Plan (Apply).
    pub fn run_form(&mut self, apply: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Form(mut panel)) = self.overlay.take() else {
            return;
        };
        let input = if apply {
            panel.apply.take()
        } else {
            panel.input(cx)
        };
        if let Some(input) = input {
            let op = panel.form.op;
            if op == Watch::NAME {
                panel.outcome = Some(self.event_lines());
            } else {
                let mutation = panel.form.kind == Kind::Mutation;
                let result = self.session.run_json(op, input.clone());
                panel.apply = (mutation && outcome::is_dry_run(&result)).then(|| {
                    let mut input = input;
                    input["dry_run"] = json!(false);
                    input
                });
                panel.outcome = Some(outcome::lines(&result));
            }
        }
        self.overlay = Some(Overlay::Form(panel));
        self.changed(window, cx);
    }

    fn event_lines(&self) -> Vec<OutcomeLine> {
        if self.session.event_log.is_empty() {
            return vec![OutcomeLine {
                kind: LineKind::Muted,
                text: "No changes yet: the GUI is watching".into(),
            }];
        }
        self.session
            .event_log
            .iter()
            .map(|e| OutcomeLine {
                kind: LineKind::Plain,
                text: e.clone(),
            })
            .collect()
    }

    /// Quick open's rows: Pages whose path or Title matches, then search hits.
    #[must_use]
    pub fn quick_open_items(&self, cx: &App) -> Vec<(String, String, String)> {
        let Some(Overlay::QuickOpen { all }) = &self.overlay else {
            return Vec::new();
        };
        let query = self.query_text(cx);
        let q = query.to_lowercase();
        let mut items: Vec<(String, String, String)> = all
            .iter()
            .filter(|(path, title)| {
                path.to_lowercase().contains(&q) || title.to_lowercase().contains(&q)
            })
            .map(|(path, title)| (path.clone(), title.clone(), path.clone()))
            .collect();
        if q.trim().chars().count() >= 2 {
            for hit in self.session.search(&query) {
                if !items.iter().any(|(path, _, _)| *path == hit.0) {
                    items.push(hit);
                }
            }
        }
        items
    }

    /// Enter in the query box: open the first match.
    pub fn query_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.overlay {
            Some(Overlay::Palette) => {
                if let Some(op) = Session::palette_matches(&self.query_text(cx)).first() {
                    let name = op.name;
                    self.pick(name, window, cx);
                }
            }
            Some(Overlay::QuickOpen { .. }) => {
                if let Some((path, _, _)) = self.quick_open_items(cx).into_iter().next() {
                    self.overlay = None;
                    self.open(&path, window, cx);
                    self.focus_view(window, cx);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ rendering

    pub fn overlay_view(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = cx.entity();
        let theme = cx.theme().clone();
        let body: AnyElement = match self.overlay.as_ref()? {
            Overlay::Palette => {
                let query = self.query_text(cx);
                v_flex()
                    .child(Input::new(&self.query))
                    .child(
                        v_flex()
                            .id("ops")
                            .max_h(px(440.))
                            .overflow_y_scroll()
                            .children(Session::palette_matches(&query).into_iter().map(|op| {
                                let view = view.clone();
                                let name = op.name;
                                ListItem::new(SharedString::from(format!("op-{name}")))
                                    .child(
                                        h_flex()
                                            .gap_3()
                                            .child(div().w(px(150.)).child(name))
                                            .child(
                                                div()
                                                    .w(px(90.))
                                                    .text_xs()
                                                    .text_color(theme.muted_foreground)
                                                    .child(format!("{:?}", op.kind).to_lowercase()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.muted_foreground)
                                                    .child(op.description),
                                            ),
                                    )
                                    .on_click(move |_, window, cx| {
                                        view.update(cx, |this, cx| this.pick(name, window, cx));
                                    })
                            })),
                    )
                    .into_any_element()
            }
            Overlay::QuickOpen { .. } => {
                let items = self.quick_open_items(cx);
                v_flex()
                    .child(Input::new(&self.query))
                    .child(Self::page_list(items, &theme, cx))
                    .into_any_element()
            }
            Overlay::TagPages { tag, pages } => {
                let items = pages
                    .iter()
                    .map(|(path, title)| (path.clone(), title.clone(), path.clone()))
                    .collect();
                v_flex()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("Pages tagged #{tag}")),
                    )
                    .child(Self::page_list(items, &theme, cx))
                    .into_any_element()
            }
            Overlay::Form(panel) => Self::form_view(panel, &view, &theme, cx),
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
                        .id("overlay")
                        .mt(px(70.))
                        .w(px(720.))
                        .max_h(px(720.))
                        .overflow_y_scroll()
                        .p_3()
                        .gap_2()
                        .bg(theme.popover)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(theme.radius_lg)
                        .shadow_lg()
                        .child(body),
                )
                .into_any_element(),
        )
    }

    fn form_view(
        panel: &FormPanel,
        view: &Entity<Self>,
        theme: &gpui_kit::component::theme::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = {
            let view = view.clone();
            move |change: Change, window: &mut Window, cx: &mut App| {
                view.update(cx, |this, cx| {
                    if let Some(Overlay::Form(panel)) = &mut this.overlay {
                        change.apply(panel, window, cx);
                    }
                    cx.notify();
                });
            }
        };
        let form = &panel.form;
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().font_weight(FontWeight::BOLD).child(form.op))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{:?}", form.kind).to_lowercase()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(form.description),
            )
            .children(panel.rows(on, cx))
            .child(
                h_flex()
                    .gap_2()
                    .when_some(form.dry_run, |d, dry| {
                        d.child(
                            Button::new("dry-run")
                                .label(if dry {
                                    "☑ dry run (show the Plan)"
                                } else {
                                    "☐ dry run"
                                })
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(Overlay::Form(panel)) = &mut this.overlay
                                        && let Some(dry) = panel.form.dry_run.as_mut()
                                    {
                                        *dry = !*dry;
                                    }
                                    cx.notify();
                                })),
                        )
                    })
                    .child(Button::new("run").label("Run").primary().small().on_click(
                        cx.listener(|this, _, window, cx| this.run_form(false, window, cx)),
                    ))
                    .when(panel.apply.is_some(), |d| {
                        d.child(
                            Button::new("apply")
                                .label("Apply")
                                .primary()
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.run_form(true, window, cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("close")
                            .label("Close")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_overlay(window, cx);
                            })),
                    ),
            )
            .children(panel.outcome.as_ref().map(|lines| outcome_view(lines, cx)))
            .into_any_element()
    }

    fn page_list(
        items: Vec<(String, String, String)>,
        theme: &gpui_kit::component::theme::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = cx.entity();
        v_flex()
            .id("pages")
            .max_h(px(440.))
            .overflow_y_scroll()
            .children(items.into_iter().map(|(path, title, detail)| {
                let view = view.clone();
                ListItem::new(SharedString::from(format!("page-{path}")))
                    .child(
                        h_flex().gap_3().child(title).child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(detail),
                        ),
                    )
                    .on_click(move |_, window, cx| {
                        view.update(cx, |this, cx| {
                            this.overlay = None;
                            this.open(&path, window, cx);
                            this.focus_view(window, cx);
                        });
                    })
            }))
            .into_any_element()
    }
}
