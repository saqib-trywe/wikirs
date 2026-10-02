//! [`crate::layout`]'s views as gpui elements (markdown.md#presentation, GUI
//! column). Clicking a Link's text calls back with the Link's index; every
//! clickable Link comes from the core.

use std::rc::Rc;

use gpui_kit::{
    component::{ActiveTheme as _, h_flex, v_flex},
    prelude::FluentBuilder as _,
    *,
};
use wikirs_core::document::{AlertKind, Align};

use crate::layout::{Mark, Rich, View};

/// Called with a Link's index when its text is clicked.
pub type OnLink = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// Builds elements; `ids` keeps every interactive text's id unique.
pub struct PageRenderer {
    on_link: OnLink,
    /// The Wiki root, which Attachment paths are relative to.
    root: std::path::PathBuf,
    next_id: usize,
}

impl PageRenderer {
    pub fn new(on_link: OnLink, root: std::path::PathBuf) -> Self {
        Self {
            on_link,
            root,
            next_id: 0,
        }
    }

    pub fn views(&mut self, views: &[View], cx: &App) -> Vec<AnyElement> {
        views.iter().map(|v| self.view(v, cx)).collect()
    }

    pub fn view(&mut self, view: &View, cx: &App) -> AnyElement {
        let theme = cx.theme();
        match view {
            View::Heading { level, text, .. } => {
                let size = match level {
                    1 => rems(1.75),
                    2 => rems(1.4),
                    3 => rems(1.2),
                    _ => rems(1.05),
                };
                div()
                    .mt_3()
                    .mb_1()
                    .text_size(size)
                    .font_weight(FontWeight::BOLD)
                    .child(self.rich(text, cx))
                    .into_any_element()
            }
            View::Paragraph(text) => div().my_1().child(self.rich(text, cx)).into_any_element(),
            View::Quote(children) => v_flex()
                .my_1()
                .pl_3()
                .border_l_2()
                .border_color(theme.border)
                .text_color(theme.muted_foreground)
                .children(self.views(children, cx))
                .into_any_element(),
            View::Alert { kind, children } => self.alert(*kind, children, cx),
            View::List(items) => v_flex()
                .my_1()
                .children(items.iter().map(|(marker, blocks)| {
                    h_flex()
                        .items_start()
                        .gap_2()
                        .child(div().min_w(px(18.)).child(marker.clone()))
                        .child(v_flex().flex_1().children(self.views(blocks, cx)))
                }))
                .into_any_element(),
            View::Code { lang, code } => v_flex()
                .my_2()
                .p_2()
                .rounded_md()
                .bg(theme.secondary)
                .font_family(theme.mono_font_family.clone())
                .text_sm()
                .when_some(lang.clone(), |d, lang| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(lang),
                    )
                })
                .child(code.clone())
                .into_any_element(),
            View::Table { align, head, rows } => self.table(align, head, rows, cx),
            View::Rule => div().my_3().h(px(1.)).bg(theme.border).into_any_element(),
            View::Image { link, url, file } => {
                let shown: AnyElement = match file {
                    Some(file) => img(file
                        .split('/')
                        .fold(self.root.clone(), |p, seg| p.join(seg)))
                    .max_w_full()
                    .into_any_element(),
                    None => div()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(format!("[image: {url}] (missing)"))
                        .into_any_element(),
                };
                self.next_id += 1;
                let on_link = self.on_link.clone();
                div()
                    .id(("image", self.next_id))
                    .my_2()
                    .when_some(*link, |d, link| {
                        d.cursor_pointer()
                            .on_click(move |_, window, cx| on_link(link, window, cx))
                    })
                    .child(shown)
                    .into_any_element()
            }
            View::Html(html) => div()
                .my_1()
                .font_family(theme.mono_font_family.clone())
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(html.clone())
                .into_any_element(),
        }
    }

    fn alert(&mut self, kind: AlertKind, children: &[View], cx: &App) -> AnyElement {
        let theme = cx.theme();
        let color = match kind {
            AlertKind::Note => theme.info,
            AlertKind::Tip => theme.success,
            AlertKind::Important => theme.primary,
            AlertKind::Warning => theme.warning,
            AlertKind::Caution => theme.danger,
        };
        v_flex()
            .my_2()
            .p_2()
            .border_l_4()
            .border_color(color)
            .bg(theme.secondary)
            .rounded_md()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color)
                    .child(kind.label()),
            )
            .children(self.views(children, cx))
            .into_any_element()
    }

    fn table(
        &mut self,
        align: &[Align],
        head: &[Rich],
        rows: &[Vec<Rich>],
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let columns = std::iter::once(head.len())
            .chain(rows.iter().map(Vec::len))
            .max()
            .unwrap_or(0);
        let border = theme.border;
        let row_el = |cells: &[Rich], header: bool, this: &mut Self| {
            h_flex().children((0..columns).map(|c| {
                let cell = cells.get(c).cloned().unwrap_or_default();
                div()
                    .flex_1()
                    .px_2()
                    .py_1()
                    .border_1()
                    .border_color(border)
                    .when(header, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .map(|d| match align.get(c) {
                        Some(Align::Right) => d.flex().justify_end(),
                        Some(Align::Center) => d.flex().justify_center(),
                        _ => d,
                    })
                    .child(this.rich(&cell, cx))
            }))
        };
        let head_el = row_el(head, true, self);
        let mut body = Vec::new();
        for row in rows {
            body.push(row_el(row, false, self));
        }
        v_flex()
            .my_2()
            .child(head_el)
            .children(body)
            .into_any_element()
    }

    /// Styled, clickable text.
    pub fn rich(&mut self, rich: &Rich, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let highlights: Vec<_> = rich
            .marks
            .iter()
            .map(|(range, mark)| {
                let style = match mark {
                    Mark::Strong => HighlightStyle {
                        font_weight: Some(FontWeight::BOLD),
                        ..Default::default()
                    },
                    Mark::Emphasis => HighlightStyle {
                        font_style: Some(FontStyle::Italic),
                        ..Default::default()
                    },
                    Mark::Strike => HighlightStyle {
                        strikethrough: Some(StrikethroughStyle {
                            thickness: px(1.),
                            color: None,
                        }),
                        ..Default::default()
                    },
                    Mark::Code => HighlightStyle {
                        background_color: Some(theme.secondary),
                        color: Some(theme.warning),
                        ..Default::default()
                    },
                    Mark::Link | Mark::BrokenLink | Mark::External => HighlightStyle {
                        color: Some(match mark {
                            Mark::BrokenLink => theme.danger,
                            _ => theme.link,
                        }),
                        underline: Some(UnderlineStyle {
                            thickness: px(1.),
                            color: None,
                            wavy: *mark == Mark::BrokenLink,
                        }),
                        ..Default::default()
                    },
                    Mark::Tag => HighlightStyle {
                        color: Some(theme.primary),
                        ..Default::default()
                    },
                    Mark::Math => HighlightStyle {
                        color: Some(theme.success),
                        font_style: Some(FontStyle::Italic),
                        ..Default::default()
                    },
                    Mark::Muted => HighlightStyle {
                        fade_out: Some(0.4),
                        ..Default::default()
                    },
                };
                (range.clone(), style)
            })
            .collect();
        let text = StyledText::new(rich.text.clone()).with_highlights(highlights);
        if rich.links.is_empty() {
            return text.into_any_element();
        }
        self.next_id += 1;
        let indices: Vec<usize> = rich.links.iter().map(|(_, i)| *i).collect();
        let ranges = rich.links.iter().map(|(r, _)| r.clone()).collect();
        let on_link = self.on_link.clone();
        let id = self.next_id;
        div()
            .id(("rich-block", id))
            .debug_selector(move || format!("rich-{id}"))
            .child(InteractiveText::new(("rich", id), text).on_click(
                ranges,
                move |clicked, window, cx| {
                    on_link(indices[clicked], window, cx);
                },
            ))
            .into_any_element()
    }
}
