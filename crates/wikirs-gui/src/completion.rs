//! `[[` autocomplete in the source editor (gui.md#custom-ui-beyond-the-palette):
//! typing `[[ru` offers Pages whose path or Title contains `ru`, and accepting
//! one inserts `path]]`.

use std::{cell::RefCell, rc::Rc};

use gpui_kit::{
    App, Task, Window,
    component::input::{CompletionProvider, Rope, RopeExt as _},
};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Range as LspRange, TextEdit,
};

/// The Wiki's Pages as `(path, title)`, kept current by the Workbench.
pub type Pages = Rc<RefCell<Vec<(String, String)>>>;

pub struct LinkCompletion {
    pub pages: Pages,
}

/// Inside an unclosed `[[` on the cursor's line: where the query starts (a
/// byte offset in `before`) and the query.
#[must_use]
pub fn link_query(before: &str) -> Option<(usize, &str)> {
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let open = before[line_start..].rfind("[[")? + line_start + 2;
    let query = &before[open..];
    (!query.contains("]]") && !query.contains('|')).then_some((open, query))
}

/// The Pages a query matches, by path or Title, case-insensitively.
#[must_use]
pub fn matching<'a>(pages: &'a [(String, String)], query: &str) -> Vec<&'a (String, String)> {
    let q = query.to_lowercase();
    pages
        .iter()
        .filter(|(path, title)| {
            path.to_lowercase().contains(&q) || title.to_lowercase().contains(&q)
        })
        .collect()
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
        let all = text.to_string();
        let before = &all[..offset.min(all.len())];
        let Some((start, query)) = link_query(before) else {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        };
        let range = LspRange {
            start: text.offset_to_position(start),
            end: text.offset_to_position(offset),
        };
        let items = matching(&self.pages.borrow(), query)
            .into_iter()
            .map(|(path, title)| CompletionItem {
                label: path.clone(),
                detail: Some(title.clone()),
                kind: Some(CompletionItemKind::FILE),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: format!("{path}]]"),
                })),
                ..Default::default()
            })
            .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _offset: usize, _new_text: &str, _cx: &mut App) -> bool {
        // `completions` itself offers nothing outside `[[`.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_inside_an_open_wikilink() {
        assert_eq!(link_query("see [[eng/ru"), Some((6, "eng/ru")));
        assert_eq!(link_query("a\nsee [["), Some((8, "")));
        assert_eq!(link_query("see [[done]] and "), None);
        assert_eq!(link_query("[[x|alias"), None);
        assert_eq!(link_query("[[x\nnext line"), None);
        assert_eq!(link_query("no link"), None);
    }

    #[test]
    fn matches_paths_and_titles() {
        let pages = vec![
            ("eng/rust".to_string(), "Rust".to_string()),
            ("inbox".to_string(), "Inbox".to_string()),
        ];
        let paths = |q| {
            matching(&pages, q)
                .into_iter()
                .map(|p| p.0.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(paths("RU"), ["eng/rust"]);
        assert_eq!(paths("inb"), ["inbox"]);
        assert_eq!(paths(""), ["eng/rust", "inbox"]);
    }
}
