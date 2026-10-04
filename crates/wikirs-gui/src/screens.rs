//! Screens that were palette-only (gui.md#custom-ui-beyond-the-palette): the
//! Wiki's settings, `check`'s diagnostics, and the open Page's Attachments.
//! Changes go through the palette's forms, so a dry run's Plan shows first.

use gpui_kit::{
    component::{
        ActiveTheme as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        list::ListItem,
        v_flex,
    },
    *,
};
use serde_json::{Value, json};

use crate::{overlay::Overlay, workbench::Workbench};

/// One setting as `get_config` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub key: String,
    pub value: Value,
    /// Where the value comes from: `default`, `wiki` or `machine`.
    pub source: String,
    pub scope: String,
    pub description: String,
}

/// One `check` diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: String,
    pub page: String,
    pub message: String,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

impl Workbench {
    /// Settings: every setting, its value and where it comes from.
    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        let settings = self
            .session
            .run_json("get_config", json!({}))
            .map(|out| {
                out["result"]["settings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|s| Setting {
                        key: text(&s["key"]),
                        value: s["value"].clone(),
                        source: text(&s["source"]),
                        scope: text(&s["scope"]),
                        description: text(&s["description"]),
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.overlay = Some(Overlay::Settings(settings));
        cx.notify();
    }

    /// Check: every diagnostic in the Wiki.
    pub fn open_check(&mut self, cx: &mut Context<Self>) {
        let diagnostics = self
            .session
            .run_json("check", json!({}))
            .map(|out| {
                out["result"]["diagnostics"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|d| Diagnostic {
                        kind: text(&d["kind"]),
                        page: text(&d["page"]),
                        message: text(&d["message"]),
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.overlay = Some(Overlay::Check(diagnostics));
        cx.notify();
    }

    /// The open Page's own Attachments (paths from the Wiki root).
    #[must_use]
    pub fn attachments(&self) -> Vec<String> {
        let Some(page) = &self.session.page else {
            return Vec::new();
        };
        wikirs_core::find("list_attachments")
            .and_then(|op| {
                op.call(self.session.wiki(), json!({ "page": page.path }))
                    .ok()
            })
            .map(|out| {
                out["result"]["attachments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|a| text(&a["path"]))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn settings_view(settings: &[Setting], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let view = cx.entity();
        v_flex()
            .gap_1()
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Settings"))
            .children(settings.iter().enumerate().map(|(i, s)| {
                let (view, s) = (view.clone(), s.clone());
                let value = match &s.value {
                    Value::String(v) => v.clone(),
                    v => v.to_string(),
                };
                h_flex()
                    .gap_3()
                    .py_1()
                    .child(
                        v_flex()
                            .flex_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div().font_weight(FontWeight::MEDIUM).child(s.key.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(format!("{} · from {}", s.scope, s.source)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(s.description.clone()),
                            ),
                    )
                    .child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .text_sm()
                            .child(value),
                    )
                    .child(
                        Button::new(("edit-setting", i))
                            .label("Edit")
                            .ghost()
                            .xsmall()
                            .on_click(move |_, window, cx| {
                                let prefill =
                                    json!({ "key": s.key, "value": s.value, "scope": s.scope });
                                view.update(cx, |this, cx| {
                                    this.open_form("set_config", &prefill, window, cx);
                                });
                            }),
                    )
            }))
            .into_any_element()
    }

    pub fn check_view(diagnostics: &[Diagnostic], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let view = cx.entity();
        let mut kinds: Vec<&str> = diagnostics.iter().map(|d| d.kind.as_str()).collect();
        kinds.dedup();
        kinds.sort_unstable();
        kinds.dedup();
        let mut list = v_flex().gap_1().child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!("Check: {} diagnostic(s)", diagnostics.len())),
        );
        if diagnostics.is_empty() {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Nothing to fix."),
            );
        }
        for kind in kinds {
            list = list.child(
                div()
                    .mt_2()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(kind.replace('_', " ").to_uppercase()),
            );
            for (i, d) in diagnostics
                .iter()
                .enumerate()
                .filter(|(_, d)| d.kind == kind)
            {
                let (view, page) = (view.clone(), d.page.clone());
                list = list.child(
                    ListItem::new(("diagnostic", i))
                        .child(
                            h_flex()
                                .gap_3()
                                .child(div().text_color(theme.blue).child(d.page.clone()))
                                .child(div().text_sm().child(d.message.clone())),
                        )
                        .on_click(move |_, window, cx| {
                            view.update(cx, |this, cx| {
                                this.overlay = None;
                                this.open(&page, window, cx);
                                this.focus_view(window, cx);
                            });
                        }),
                );
            }
        }
        list.into_any_element()
    }

    /// The right panel's Attachments section: open, add, move, delete.
    pub fn attachments_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let view = cx.entity();
        let Some(page) = self.session.page.as_ref().map(|p| p.path.clone()) else {
            return div().into_any_element();
        };
        let root = self.session.wiki().root().to_path_buf();
        let files = self.attachments();
        let add = {
            let view = view.clone();
            Button::new("add-attachment")
                .label("Add…")
                .ghost()
                .xsmall()
                .on_click(move |_, window, cx| {
                    let prefill = json!({ "page": page, "source": { "local_path": "" } });
                    view.update(cx, |this, cx| {
                        this.open_form("add_attachment", &prefill, window, cx);
                    });
                })
        };
        let mut section = v_flex().gap_0p5().child(add);
        if files.is_empty() {
            section = section.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("None"),
            );
        }
        for (i, path) in files.into_iter().enumerate() {
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            let file = path.split('/').fold(root.clone(), |p, seg| p.join(seg));
            let (move_view, delete_view) = (view.clone(), view.clone());
            let (move_path, delete_path) = (path.clone(), path.clone());
            section = section.child(
                h_flex()
                    .gap_1()
                    .child(
                        div()
                            .id(("attachment", i))
                            .flex_1()
                            .text_sm()
                            .text_color(theme.blue)
                            .cursor_pointer()
                            .child(name)
                            .on_click(move |_, _, _| wikirs_ui::open_externally(&file)),
                    )
                    .child(
                        Button::new(("move-attachment", i))
                            .label("Move")
                            .ghost()
                            .xsmall()
                            .on_click(move |_, window, cx| {
                                let prefill = json!({ "from": move_path, "to": move_path });
                                move_view.update(cx, |this, cx| {
                                    this.open_form("move_attachment", &prefill, window, cx);
                                });
                            }),
                    )
                    .child(
                        Button::new(("delete-attachment", i))
                            .label("Delete")
                            .ghost()
                            .xsmall()
                            .on_click(move |_, window, cx| {
                                let prefill = json!({ "path": delete_path });
                                delete_view.update(cx, |this, cx| {
                                    this.open_form("delete_attachment", &prefill, window, cx);
                                });
                            }),
                    ),
            );
        }
        section.into_any_element()
    }
}

/// The breadcrumb bar's buttons for the screens.
pub fn screen_buttons(cx: &mut Context<Workbench>) -> impl IntoElement {
    h_flex()
        .gap_1()
        .child(
            Button::new("open-check")
                .label("Check")
                .ghost()
                .xsmall()
                .on_click(cx.listener(|this, _, _, cx| this.open_check(cx))),
        )
        .child(
            Button::new("open-settings")
                .label("Settings")
                .ghost()
                .xsmall()
                .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
        )
}
