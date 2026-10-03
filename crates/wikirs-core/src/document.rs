//! The document model (ADR 0007, docs/spec/markdown.md#presentation): one
//! Page as blocks and inlines, for the TUI and GUI to render.
//!
//! It walks the same pulldown-cmark events with the same rules as
//! [`markdown::parse`], so its Links, Tags and headings are exactly the ones the
//! Index sees. Renderers draw this and never re-parse.

use std::ops::Range;

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, LinkType, Parser, Tag, TagEnd,
};
use serde::Serialize;

use crate::markdown::{self, RawLink, anchor, inline_tags, level_num, raw_link};

/// One Page, ready to render.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct Document {
    pub blocks: Vec<Block>,
    /// Every Link in source order: the list [`markdown::parse`] gives the Index.
    /// [`Inline::Link`] and [`Inline::Image`] point into it by position.
    #[serde(skip)]
    pub links: Vec<RawLink>,
    /// Footnote definitions in source order, drawn after the blocks.
    pub footnotes: Vec<Footnote>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Block {
    Heading {
        level: u8,
        /// The anchor a `#heading` Link names (as in `outline`).
        anchor: String,
        inlines: Vec<Inline>,
        /// Byte range in the Page.
        range: Range<usize>,
    },
    Paragraph {
        inlines: Vec<Inline>,
    },
    Quote {
        blocks: Vec<Block>,
    },
    /// `> [!NOTE]` and friends.
    Alert {
        kind: AlertKind,
        blocks: Vec<Block>,
    },
    List {
        /// The first number of an ordered list; `None` for bullets.
        start: Option<u64>,
        items: Vec<Item>,
    },
    /// Fenced or indented code, mermaid included.
    Code {
        lang: Option<String>,
        code: String,
    },
    Table {
        align: Vec<Align>,
        head: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    Rule,
    /// Raw HTML, shown as literal text and never rendered.
    Html {
        html: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    /// `Some(checked)` for a task list item.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Footnote {
    pub label: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    /// The label a renderer shows, e.g. `NOTE`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            AlertKind::Note => "NOTE",
            AlertKind::Tip => "TIP",
            AlertKind::Important => "IMPORTANT",
            AlertKind::Warning => "WARNING",
            AlertKind::Caution => "CAUTION",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Inline {
    Text {
        text: String,
    },
    Code {
        code: String,
    },
    Emphasis {
        inlines: Vec<Inline>,
    },
    Strong {
        inlines: Vec<Inline>,
    },
    Strikethrough {
        inlines: Vec<Inline>,
    },
    /// A Link: `link` is its position in [`Document::links`].
    Link {
        link: usize,
        inlines: Vec<Inline>,
    },
    /// A URL, mail address or in-page anchor: not a Link to the Wiki.
    External {
        url: String,
        inlines: Vec<Inline>,
    },
    /// `![[file]]` or `![alt](file)`. `link` is set when it's a Link (an
    /// Attachment or Page embed), and unset for an external image.
    Image {
        link: Option<usize>,
        url: String,
        alt: Vec<Inline>,
    },
    /// An Inline Tag, as written (without `#`).
    Tag {
        tag: String,
    },
    Math {
        source: String,
        display: bool,
    },
    FootnoteRef {
        label: String,
    },
    /// Inline raw HTML, shown as literal text.
    Html {
        html: String,
    },
    SoftBreak,
    HardBreak,
}

/// Builds the document model of one Page's markdown.
#[must_use]
pub fn document(text: &str) -> Document {
    let mut b = Builder {
        stack: vec![Frame::Blocks {
            kind: Container::Root,
            blocks: Vec::new(),
        }],
        links: Vec::new(),
        footnotes: Vec::new(),
        in_url_link: 0,
    };
    for (event, range) in Parser::new_ext(text, markdown::options()).into_offset_iter() {
        b.event(text, event, range);
    }
    b.close_implicit();
    let Some(Frame::Blocks { blocks, .. }) = b.stack.pop() else {
        unreachable!("the root frame is never popped by an event");
    };
    Document {
        blocks,
        links: b.links,
        footnotes: b.footnotes,
    }
}

impl Document {
    /// Every Inline Tag (the Index's inline Tags): the blocks' in source order,
    /// then the footnotes'.
    #[must_use]
    pub fn inline_tags(&self) -> Vec<&str> {
        let mut out = Vec::new();
        let blocks = self
            .blocks
            .iter()
            .chain(self.footnotes.iter().flat_map(|f| &f.blocks));
        for block in blocks {
            block.visit_inlines(&mut |inline| {
                if let Inline::Tag { tag } = inline {
                    out.push(tag.as_str());
                }
            });
        }
        out
    }
}

impl Block {
    /// Calls `f` on every inline in this block, depth first, in source order.
    pub fn visit_inlines<'a>(&'a self, f: &mut impl FnMut(&'a Inline)) {
        match self {
            Block::Heading { inlines, .. } | Block::Paragraph { inlines } => {
                visit_all(inlines, f);
            }
            Block::Quote { blocks } | Block::Alert { blocks, .. } => {
                for block in blocks {
                    block.visit_inlines(f);
                }
            }
            Block::List { items, .. } => {
                for block in items.iter().flat_map(|i| &i.blocks) {
                    block.visit_inlines(f);
                }
            }
            Block::Table { head, rows, .. } => {
                for cell in head.iter().chain(rows.iter().flatten()) {
                    visit_all(cell, f);
                }
            }
            Block::Code { .. } | Block::Rule | Block::Html { .. } => {}
        }
    }
}

fn visit_all<'a>(inlines: &'a [Inline], f: &mut impl FnMut(&'a Inline)) {
    for inline in inlines {
        f(inline);
        match inline {
            Inline::Emphasis { inlines }
            | Inline::Strong { inlines }
            | Inline::Strikethrough { inlines }
            | Inline::Link { inlines, .. }
            | Inline::External { inlines, .. }
            | Inline::Image { alt: inlines, .. } => visit_all(inlines, f),
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ builder

/// What a block frame becomes when it closes.
enum Container {
    Root,
    Quote(Option<BlockQuoteKind>),
    Item { task: Option<bool> },
    Footnote(String),
}

/// What an inline frame becomes when it closes.
enum Span {
    Paragraph,
    /// Inlines directly in a list item (a tight list): closed by the next block.
    Implicit,
    Heading {
        level: u8,
        range: Range<usize>,
    },
    Emphasis,
    Strong,
    Strikethrough,
    Link(usize),
    External(String),
    Image {
        link: Option<usize>,
        url: String,
    },
    Cell,
}

enum Frame {
    Blocks {
        kind: Container,
        blocks: Vec<Block>,
    },
    Inlines {
        kind: Span,
        inlines: Vec<Inline>,
    },
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Table {
        align: Vec<Align>,
        head: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
        /// The cells of the row being read (the head has no row tag).
        row: Vec<Vec<Inline>>,
    },
    Code {
        lang: Option<String>,
        code: String,
    },
    Html(String),
    Metadata,
}

struct Builder {
    stack: Vec<Frame>,
    links: Vec<RawLink>,
    footnotes: Vec<Footnote>,
    /// Inside an autolink or email link, as in `markdown::parse`.
    in_url_link: usize,
}

impl Builder {
    fn event(&mut self, text: &str, event: Event<'_>, range: Range<usize>) {
        match event {
            Event::Start(tag) => self.start(text, tag, range),
            Event::End(end) => self.end(end),
            Event::Text(t) => match self.stack.last_mut() {
                Some(Frame::Code { code, .. }) => code.push_str(&t),
                Some(Frame::Html(html)) => html.push_str(&t),
                Some(Frame::Metadata) => {}
                _ if self.in_url_link > 0 => self.push_text(&t),
                _ => self.push_tagged_text(&t),
            },
            Event::Code(code) => self.push_inline(Inline::Code {
                code: code.into_string(),
            }),
            Event::InlineMath(source) => self.push_inline(Inline::Math {
                source: source.into_string(),
                display: false,
            }),
            Event::DisplayMath(source) => self.push_inline(Inline::Math {
                source: source.into_string(),
                display: true,
            }),
            Event::Html(html) => {
                if let Some(Frame::Html(block)) = self.stack.last_mut() {
                    block.push_str(&html);
                } else {
                    self.push_block(Block::Html {
                        html: html.into_string(),
                    });
                }
            }
            Event::InlineHtml(html) => self.push_inline(Inline::Html {
                html: html.into_string(),
            }),
            Event::FootnoteReference(label) => self.push_inline(Inline::FootnoteRef {
                label: label.into_string(),
            }),
            Event::SoftBreak => self.push_inline(Inline::SoftBreak),
            Event::HardBreak => self.push_inline(Inline::HardBreak),
            Event::Rule => self.push_block(Block::Rule),
            Event::TaskListMarker(checked) => {
                let item = self.stack.iter_mut().rev().find_map(|f| match f {
                    Frame::Blocks {
                        kind: Container::Item { task },
                        ..
                    } => Some(task),
                    _ => None,
                });
                if let Some(task) = item {
                    *task = Some(checked);
                }
            }
        }
    }

    fn start(&mut self, text: &str, tag: Tag<'_>, range: Range<usize>) {
        let frame = match tag {
            Tag::Paragraph => inlines(Span::Paragraph),
            Tag::Heading { level, .. } => inlines(Span::Heading {
                level: level_num(level),
                range,
            }),
            Tag::BlockQuote(kind) => blocks(Container::Quote(kind)),
            Tag::CodeBlock(kind) => Frame::Code {
                lang: match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_string)
                    }
                    CodeBlockKind::Indented => None,
                },
                code: String::new(),
            },
            Tag::HtmlBlock => Frame::Html(String::new()),
            Tag::List(start) => Frame::List {
                start,
                items: Vec::new(),
            },
            Tag::Item => blocks(Container::Item { task: None }),
            Tag::FootnoteDefinition(label) => blocks(Container::Footnote(label.into_string())),
            Tag::Table(align) => Frame::Table {
                align: align.into_iter().map(convert_align).collect(),
                head: Vec::new(),
                rows: Vec::new(),
                row: Vec::new(),
            },
            Tag::TableCell => inlines(Span::Cell),
            Tag::Emphasis => inlines(Span::Emphasis),
            Tag::Strong => inlines(Span::Strong),
            Tag::Strikethrough => inlines(Span::Strikethrough),
            Tag::Link {
                link_type,
                dest_url,
                id,
                ..
            } => {
                if matches!(link_type, LinkType::Autolink | LinkType::Email) {
                    self.in_url_link += 1;
                    inlines(Span::External(dest_url.into_string()))
                } else {
                    let embed = text[range.clone()].starts_with('!');
                    match raw_link(text, range, &dest_url, &id, link_type, embed) {
                        Some(link) => inlines(Span::Link(self.add_link(link))),
                        None => inlines(Span::External(dest_url.into_string())),
                    }
                }
            }
            Tag::Image {
                link_type,
                dest_url,
                id,
                ..
            } => {
                let embed = text[range.clone()].starts_with('!');
                let link = raw_link(text, range, &dest_url, &id, link_type, embed)
                    .map(|link| self.add_link(link));
                inlines(Span::Image {
                    link,
                    url: dest_url.into_string(),
                })
            }
            Tag::MetadataBlock(_) => Frame::Metadata,
            // Not enabled: their content still shows, in plain blocks.
            Tag::TableHead
            | Tag::TableRow
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript => return,
        };
        if is_block(&frame) {
            self.close_implicit();
        }
        self.stack.push(frame);
    }

    fn end(&mut self, end: TagEnd) {
        match end {
            TagEnd::TableHead => {
                if let Some(Frame::Table { head, row, .. }) = self.stack.last_mut() {
                    *head = std::mem::take(row);
                }
                return;
            }
            TagEnd::TableRow => {
                if let Some(Frame::Table { rows, row, .. }) = self.stack.last_mut() {
                    rows.push(std::mem::take(row));
                }
                return;
            }
            TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript => return,
            TagEnd::Link if self.in_url_link > 0 => self.in_url_link -= 1,
            _ => {}
        }
        self.close_implicit();
        let Some(frame) = self.stack.pop() else {
            return;
        };
        match frame {
            Frame::Blocks { kind, blocks } => match kind {
                Container::Root => self.stack.push(Frame::Blocks { kind, blocks }),
                Container::Quote(None) => self.push_block(Block::Quote { blocks }),
                Container::Quote(Some(kind)) => self.push_block(Block::Alert {
                    kind: convert_alert(kind),
                    blocks,
                }),
                Container::Item { task } => {
                    if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                        items.push(Item { task, blocks });
                    }
                }
                Container::Footnote(label) => self.footnotes.push(Footnote { label, blocks }),
            },
            Frame::Inlines { kind, inlines } => self.close_span(kind, inlines),
            Frame::List { start, items } => self.push_block(Block::List { start, items }),
            Frame::Table {
                align, head, rows, ..
            } => self.push_block(Block::Table { align, head, rows }),
            Frame::Code { lang, code } => self.push_block(Block::Code { lang, code }),
            Frame::Html(html) => self.push_block(Block::Html { html }),
            Frame::Metadata => {}
        }
    }

    fn close_span(&mut self, kind: Span, inlines: Vec<Inline>) {
        match kind {
            Span::Paragraph | Span::Implicit => self.push_block(Block::Paragraph { inlines }),
            Span::Heading { level, range } => {
                let text = plain_text(&inlines);
                self.push_block(Block::Heading {
                    level,
                    anchor: anchor(&text),
                    inlines,
                    range,
                });
            }
            Span::Emphasis => self.push_inline(Inline::Emphasis { inlines }),
            Span::Strong => self.push_inline(Inline::Strong { inlines }),
            Span::Strikethrough => self.push_inline(Inline::Strikethrough { inlines }),
            Span::Link(link) => self.push_inline(Inline::Link { link, inlines }),
            Span::External(url) => self.push_inline(Inline::External { url, inlines }),
            Span::Image { link, url } => self.push_inline(Inline::Image {
                link,
                url,
                alt: inlines,
            }),
            Span::Cell => {
                if let Some(Frame::Table { row, .. }) = self.stack.last_mut() {
                    row.push(inlines);
                }
            }
        }
    }

    fn add_link(&mut self, link: RawLink) -> usize {
        self.links.push(link);
        self.links.len() - 1
    }

    /// Closes a tight list item's inlines before a block starts or the item ends.
    fn close_implicit(&mut self) {
        if let Some(Frame::Inlines {
            kind: Span::Implicit,
            ..
        }) = self.stack.last()
            && let Some(Frame::Inlines { inlines, .. }) = self.stack.pop()
        {
            self.push_block(Block::Paragraph { inlines });
        }
    }

    fn push_block(&mut self, block: Block) {
        if let Some(Frame::Blocks { blocks, .. }) = self.stack.last_mut() {
            blocks.push(block);
        }
    }

    fn push_inline(&mut self, inline: Inline) {
        if matches!(self.stack.last(), Some(Frame::Blocks { .. })) {
            self.stack.push(inlines(Span::Implicit));
        }
        if let Some(Frame::Inlines { inlines, .. }) = self.stack.last_mut() {
            // Entities and escapes split text events; keep runs whole.
            if let (Some(Inline::Text { text: last }), Inline::Text { text }) =
                (inlines.last_mut(), &inline)
            {
                last.push_str(text);
            } else {
                inlines.push(inline);
            }
        }
    }

    fn push_text(&mut self, text: &str) {
        if !text.is_empty() {
            self.push_inline(Inline::Text {
                text: text.to_string(),
            });
        }
    }

    /// Text with its Inline Tags split out, by the Index's own scanner.
    fn push_tagged_text(&mut self, text: &str) {
        let mut at = 0;
        for (start, tag) in inline_tags(text) {
            self.push_text(&text[at..start]);
            self.push_inline(Inline::Tag {
                tag: tag.to_string(),
            });
            at = start + 1 + tag.len();
        }
        self.push_text(&text[at..]);
    }
}

fn blocks(kind: Container) -> Frame {
    Frame::Blocks {
        kind,
        blocks: Vec::new(),
    }
}

fn inlines(kind: Span) -> Frame {
    Frame::Inlines {
        kind,
        inlines: Vec::new(),
    }
}

/// Whether a frame holds blocks, so it ends a tight item's inlines. Inline
/// frames (and cells) don't.
fn is_block(frame: &Frame) -> bool {
    !matches!(
        frame,
        Frame::Inlines {
            kind: Span::Emphasis
                | Span::Strong
                | Span::Strikethrough
                | Span::Link(_)
                | Span::External(_)
                | Span::Image { .. }
                | Span::Cell,
            ..
        }
    )
}

/// The text of some inlines, as `markdown::parse` collects a heading's.
fn plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    visit_all(inlines, &mut |inline| match inline {
        Inline::Text { text } => out.push_str(text),
        Inline::Code { code } => out.push_str(code),
        Inline::Tag { tag } => {
            out.push('#');
            out.push_str(tag);
        }
        _ => {}
    });
    out
}

fn convert_align(align: Alignment) -> Align {
    match align {
        Alignment::None => Align::None,
        Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
    }
}

fn convert_alert(kind: BlockQuoteKind) -> AlertKind {
    match kind {
        BlockQuoteKind::Note => AlertKind::Note,
        BlockQuoteKind::Tip => AlertKind::Tip,
        BlockQuoteKind::Important => AlertKind::Important,
        BlockQuoteKind::Warning => AlertKind::Warning,
        BlockQuoteKind::Caution => AlertKind::Caution,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::markdown::{TagSource, parse};

    const KITCHEN_SINK: &str = "---\ntitle: Sink\ntags: [demo]\n---\n\
# Sink #demo\n\n\
Text with *em*, **strong**, ~~gone~~, `code`, $x^2$ and a #tag/sub.\n\
See [[eng/rust|Rust]], [notes](../notes.md#top), <https://x.com/#no> and [web](https://y.com).\n\n\
![[diagram.png]] ![alt #t](https://img/x.png)\n\n\
- [ ] todo\n- [x] done\n  1. nested\n\n\
> [!WARNING]\n> Careful[^1].\n\n\
> plain quote\n\n\
| a | b |\n|:--|--:|\n| 1 | [[x]] |\n\n\
```rust\nlet #no = 1;\n```\n\n\
$$\nE = mc^2\n$$\n\n\
<div>raw</div>\n\n\
Inline <b>html</b>.\n\n\
---\n\n\
[^1]: The note #fn.\n";

    #[test]
    fn kitchen_sink() {
        let doc = document(KITCHEN_SINK);
        insta::assert_json_snapshot!(doc);
        let links: Vec<_> = doc.links.iter().map(|l| l.raw.as_str()).collect();
        assert_eq!(
            links,
            [
                "[[eng/rust|Rust]]",
                "[notes](../notes.md#top)",
                "![[diagram.png]]",
                "[[x]]"
            ]
        );
    }

    #[test]
    fn tight_and_loose_items_hold_paragraphs() {
        let tight = document("- a\n- b\n  - c\n");
        let loose = document("- a\n\n- b\n");
        let Block::List { items, .. } = &tight.blocks[0] else {
            panic!("{tight:?}")
        };
        assert!(matches!(
            items[1].blocks[..],
            [Block::Paragraph { .. }, Block::List { .. }]
        ));
        let Block::List { items, .. } = &loose.blocks[0] else {
            panic!("{loose:?}")
        };
        assert!(matches!(items[0].blocks[..], [Block::Paragraph { .. }]));
    }

    #[test]
    fn entities_and_escapes_keep_text_whole() {
        let doc = document("a &amp; b \\* c");
        assert_eq!(
            doc.blocks,
            [Block::Paragraph {
                inlines: vec![Inline::Text {
                    text: "a & b * c".into()
                }]
            }]
        );
    }

    // ------------------------------------------------- agreement with the Index

    const PIECES: &[&str] = &[
        "word ",
        "#tag ",
        "#a/b ",
        "#2026 ",
        "x#no ",
        "[[p]] ",
        "[[p|Alias #t]] ",
        "[[p#H]] ",
        "![[f.png]] ",
        "[l](q.md) ",
        "[l](#here) ",
        "[w](https://w.x/#u) ",
        "<https://a.b/#c> ",
        "<irc:#chan> ",
        "![i #t](i.png) ",
        "*",
        "**",
        "~~",
        "`c #t` ",
        "$m #t$ ",
        "&amp;",
        "\\#esc ",
        "<i>#h</i> ",
        "[^n] ",
        "\n",
        "\n\n",
        "\n# ",
        "\n## ",
        "\n- ",
        "\n  - ",
        "\n1. ",
        "\n- [ ] ",
        "\n> ",
        "\n> [!NOTE]\n> ",
        "\n```\n#code\n```\n",
        "\n| a | #b |\n|---|---|\n| [[c]] | d |\n",
        "\n[^n]: foot #f ",
        "\n---\n",
        "\n<div>\n#h\n</div>\n",
    ];

    fn markdown() -> impl Strategy<Value = String> {
        let front = prop_oneof![Just(""), Just("---\ntags: [x]\ntitle: T\n---\n")];
        (
            front,
            prop::collection::vec(prop::sample::select(PIECES), 0..40),
        )
            .prop_map(|(front, pieces)| format!("{front}{}", pieces.concat()))
    }

    proptest! {
        /// The document's Links, Tags and headings are the Index's (ADR 0007).
        #[test]
        fn agrees_with_the_index(text in markdown()) {
            let parsed = parse(&text);
            let doc = document(&text);
            prop_assert_eq!(&doc.links, &parsed.links);
            let inline: Vec<_> = parsed
                .tags
                .iter()
                .filter(|t| t.source == TagSource::Inline)
                .map(|t| t.tag.as_str())
                .collect();
            // Footnotes come last in the document, wherever they're defined.
            let (mut got, mut want) = (doc.inline_tags(), inline);
            got.sort_unstable();
            want.sort_unstable();
            prop_assert_eq!(got, want);
            let headings: Vec<_> = parsed
                .headings
                .iter()
                .map(|h| (h.level, h.anchor.clone(), h.range.clone()))
                .collect();
            let mut doc_headings = Vec::new();
            collect_headings(&doc.blocks, &mut doc_headings);
            for f in &doc.footnotes {
                collect_headings(&f.blocks, &mut doc_headings);
            }
            doc_headings.sort_by_key(|h| h.2.start);
            let mut headings = headings;
            headings.sort_by_key(|h| h.2.start);
            prop_assert_eq!(doc_headings, headings);
        }
    }

    fn collect_headings(blocks: &[Block], out: &mut Vec<(u8, String, Range<usize>)>) {
        for block in blocks {
            match block {
                Block::Heading {
                    level,
                    anchor,
                    range,
                    ..
                } => out.push((*level, anchor.clone(), range.clone())),
                Block::Quote { blocks } | Block::Alert { blocks, .. } => {
                    collect_headings(blocks, out);
                }
                Block::List { items, .. } => {
                    for item in items {
                        collect_headings(&item.blocks, out);
                    }
                }
                _ => {}
            }
        }
    }
}
