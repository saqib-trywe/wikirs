//! The core's document model laid out for the GUI (markdown.md#presentation,
//! GUI column), with no gpui in it: blocks, and runs of text with marked ranges
//! and the ranges each Link covers. [`crate::page`] turns this into elements.

use std::{fmt::Write, ops::Range};

use wikirs_core::{
    document::{AlertKind, Align, Block, Document, Inline},
    links::Resolved,
};

/// Styled text: `marks` and `links` are byte ranges of `text`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rich {
    pub text: String,
    pub marks: Vec<(Range<usize>, Mark)>,
    /// `(range, Link index)`: clicking the range follows that Link.
    pub links: Vec<(Range<usize>, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Strong,
    Emphasis,
    Strike,
    Code,
    Link,
    BrokenLink,
    External,
    Tag,
    Math,
    /// Footnote references, raw HTML and image placeholders.
    Muted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    Heading {
        level: u8,
        anchor: String,
        text: Rich,
    },
    Paragraph(Rich),
    Quote(Vec<View>),
    Alert {
        kind: AlertKind,
        children: Vec<View>,
    },
    /// Each item: its marker (`•`, `3.`, `☐`, `☑`) and blocks.
    List(Vec<(String, Vec<View>)>),
    Code {
        lang: Option<String>,
        code: String,
    },
    Table {
        align: Vec<Align>,
        head: Vec<Rich>,
        rows: Vec<Vec<Rich>>,
    },
    Rule,
    Html(String),
}

/// The Page's blocks, then its footnotes (`[^label]` and their blocks).
#[must_use]
pub fn layout(
    doc: &Document,
    resolved: &[Option<Resolved>],
) -> (Vec<View>, Vec<(String, Vec<View>)>) {
    let l = Layout { resolved };
    let blocks = doc.blocks.iter().map(|b| l.block(b)).collect();
    let notes = doc
        .footnotes
        .iter()
        .map(|n| {
            (
                n.label.clone(),
                n.blocks.iter().map(|b| l.block(b)).collect(),
            )
        })
        .collect();
    (blocks, notes)
}

/// Whether a resolution means the Link is broken (or couldn't be resolved).
#[must_use]
pub fn is_broken(resolved: Option<&Resolved>) -> bool {
    resolved.is_none_or(|r| r.status == wikirs_core::links::LinkStatus::Broken)
}

struct Layout<'a> {
    resolved: &'a [Option<Resolved>],
}

impl Layout<'_> {
    fn block(&self, block: &Block) -> View {
        match block {
            Block::Heading {
                level,
                anchor,
                inlines,
                ..
            } => View::Heading {
                level: *level,
                anchor: anchor.clone(),
                text: self.rich(inlines),
            },
            Block::Paragraph { inlines } => View::Paragraph(self.rich(inlines)),
            Block::Quote { blocks } => View::Quote(blocks.iter().map(|b| self.block(b)).collect()),
            Block::Alert { kind, blocks } => View::Alert {
                kind: *kind,
                children: blocks.iter().map(|b| self.block(b)).collect(),
            },
            Block::List { start, items } => View::List(
                items
                    .iter()
                    .enumerate()
                    .map(|(n, item)| {
                        let marker = match (item.task, start) {
                            (Some(true), _) => "☑".to_string(),
                            (Some(false), _) => "☐".to_string(),
                            (None, Some(first)) => format!("{}.", first + n as u64),
                            (None, None) => "•".to_string(),
                        };
                        (marker, item.blocks.iter().map(|b| self.block(b)).collect())
                    })
                    .collect(),
            ),
            Block::Code { lang, code } => View::Code {
                lang: lang.clone(),
                code: code.trim_end_matches('\n').to_string(),
            },
            Block::Table { align, head, rows } => View::Table {
                align: align.clone(),
                head: head.iter().map(|c| self.rich(c)).collect(),
                rows: rows
                    .iter()
                    .map(|r| r.iter().map(|c| self.rich(c)).collect())
                    .collect(),
            },
            Block::Rule => View::Rule,
            Block::Html { html } => View::Html(html.trim_end_matches('\n').to_string()),
        }
    }

    fn rich(&self, inlines: &[Inline]) -> Rich {
        let mut rich = Rich::default();
        self.inlines(inlines, &mut rich);
        rich
    }

    fn inlines(&self, inlines: &[Inline], out: &mut Rich) {
        for inline in inlines {
            match inline {
                Inline::Text { text } => out.text.push_str(text),
                Inline::Code { code } => mark(out, Mark::Code, |o| o.text.push_str(code)),
                Inline::Emphasis { inlines } => {
                    mark(out, Mark::Emphasis, |o| self.inlines(inlines, o));
                }
                Inline::Strong { inlines } => mark(out, Mark::Strong, |o| self.inlines(inlines, o)),
                Inline::Strikethrough { inlines } => {
                    mark(out, Mark::Strike, |o| self.inlines(inlines, o));
                }
                Inline::Link { link, inlines } => {
                    let start = out.text.len();
                    mark(out, self.link_mark(*link), |o| self.inlines(inlines, o));
                    out.links.push((start..out.text.len(), *link));
                }
                Inline::External { inlines, .. } => {
                    mark(out, Mark::External, |o| self.inlines(inlines, o));
                }
                Inline::Image { link, url, .. } => {
                    // Inline images come later (gui.md); for now a placeholder
                    // that opens the file like any Attachment Link.
                    let start = out.text.len();
                    let style = link.map_or(Mark::Muted, |l| self.link_mark(l));
                    mark(out, style, |o| {
                        let _ = write!(o.text, "[image: {url}]");
                    });
                    if let Some(link) = link {
                        out.links.push((start..out.text.len(), *link));
                    }
                }
                Inline::Tag { tag } => mark(out, Mark::Tag, |o| {
                    o.text.push('#');
                    o.text.push_str(tag);
                }),
                Inline::Math { source, display } => mark(out, Mark::Math, |o| {
                    let delim = if *display { "$$" } else { "$" };
                    o.text.push_str(delim);
                    o.text.push_str(source.trim());
                    o.text.push_str(delim);
                }),
                Inline::FootnoteRef { label } => {
                    mark(out, Mark::Muted, |o| {
                        let _ = write!(o.text, "[^{label}]");
                    });
                }
                Inline::Html { html } => mark(out, Mark::Muted, |o| o.text.push_str(html)),
                Inline::SoftBreak => out.text.push(' '),
                Inline::HardBreak => out.text.push('\n'),
            }
        }
    }

    fn link_mark(&self, link: usize) -> Mark {
        if is_broken(self.resolved.get(link).and_then(Option::as_ref)) {
            Mark::BrokenLink
        } else {
            Mark::Link
        }
    }
}

/// Runs `write`, marking whatever it appended.
fn mark(out: &mut Rich, mark: Mark, write: impl FnOnce(&mut Rich)) {
    let start = out.text.len();
    write(out);
    if out.text.len() > start {
        out.marks.push((start..out.text.len(), mark));
    }
}

#[cfg(test)]
mod tests {
    use wikirs_core::{
        document::document,
        links::{LinkStatus, TargetKind},
    };

    use super::*;

    fn ok(target: &str) -> Resolved {
        Resolved {
            target: target.into(),
            target_kind: TargetKind::Page,
            heading: None,
            status: LinkStatus::Ok,
        }
    }

    #[test]
    fn links_marks_and_ranges() {
        let doc = document("See **[[a|the A]]**, `x`, #t and [[gone]] or <https://x.y>.\n");
        let broken = Some(Resolved {
            status: LinkStatus::Broken,
            ..ok("gone")
        });
        let (views, _) = layout(&doc, &[Some(ok("a")), broken]);
        let [View::Paragraph(rich)] = views.as_slice() else {
            panic!("{views:?}")
        };
        assert_eq!(rich.text, "See the A, x, #t and gone or https://x.y.");
        let at = |r: &Range<usize>| &rich.text[r.clone()];
        let marks: Vec<_> = rich.marks.iter().map(|(r, m)| (at(r), *m)).collect();
        assert_eq!(
            marks,
            [
                ("the A", Mark::Link),
                ("the A", Mark::Strong),
                ("x", Mark::Code),
                ("#t", Mark::Tag),
                ("gone", Mark::BrokenLink),
                ("https://x.y", Mark::External),
            ]
        );
        let links: Vec<_> = rich.links.iter().map(|(r, i)| (at(r), *i)).collect();
        assert_eq!(links, [("the A", 0), ("gone", 1)]);
        // Nothing to mark, nothing marked.
        let (views, _) = layout(&document("[](q.md) x"), &[None]);
        let [View::Paragraph(rich)] = views.as_slice() else {
            panic!()
        };
        assert!(rich.marks.is_empty(), "{rich:?}");
    }

    #[test]
    fn blocks() {
        let doc = document(
            "# T\n\n> [!TIP]\n> hi\n\n- [x] done\n- [ ] todo\n\n3. c\n4. d\n\n| a | b |\n|--:|---|\n| 1 | ![[p.png]] |\n\n```rs\nfn x() {}\n```\n\n---\n\nnote[^n]\n\n[^n]: the note\n",
        );
        let (views, notes) = layout(&doc, &[None]);
        assert!(matches!(&views[0], View::Heading { level: 1, anchor, .. } if anchor == "t"));
        assert!(
            matches!(&views[1], View::Alert { kind: AlertKind::Tip, children } if children.len() == 1)
        );
        let View::List(items) = &views[2] else {
            panic!()
        };
        assert_eq!(
            items.iter().map(|i| i.0.as_str()).collect::<Vec<_>>(),
            ["☑", "☐"]
        );
        let View::List(items) = &views[3] else {
            panic!()
        };
        assert_eq!(
            items.iter().map(|i| i.0.as_str()).collect::<Vec<_>>(),
            ["3.", "4."]
        );
        let View::Table { align, rows, .. } = &views[4] else {
            panic!()
        };
        assert_eq!(align, &[Align::Right, Align::None]);
        assert_eq!(rows[0][1].text, "[image: p.png]");
        assert_eq!(rows[0][1].links, [(0..14, 0)]);
        assert_eq!(rows[0][1].marks, [(0..14, Mark::BrokenLink)]);
        assert_eq!(
            views[5],
            View::Code {
                lang: Some("rs".into()),
                code: "fn x() {}".into()
            }
        );
        assert_eq!(views[6], View::Rule);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].0, "n");
    }
}
