//! The core's document model drawn as terminal lines (markdown.md#presentation,
//! TUI column). Links are numbered `[n]` in document order, the number they're
//! followed by; whether one is broken comes from the core (`resolve_link`).

use ratatui::{
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
};
use wikirs_core::{
    document::{Align, Block, Document, Inline, Item},
    links::{LinkStatus, Resolved},
};

/// A rendered Page: its lines, and the line each heading starts on.
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub headings: Vec<(String, usize)>,
}

/// Draws `doc`. `resolved[i]` is Link `i`'s resolution (`None` if it couldn't
/// be resolved); `selected` highlights one Link.
#[must_use]
pub fn render(doc: &Document, resolved: &[Option<Resolved>], selected: Option<usize>) -> Rendered {
    let r = Renderer { resolved, selected };
    let mut out = Rendered {
        lines: Vec::new(),
        headings: Vec::new(),
    };
    r.blocks(&doc.blocks, &mut out);
    if !doc.footnotes.is_empty() {
        out.lines.push(Line::from("─".repeat(20)).dim());
        for note in &doc.footnotes {
            let mut lines = Rendered {
                lines: Vec::new(),
                headings: Vec::new(),
            };
            r.blocks(&note.blocks, &mut lines);
            let marker = format!("[^{}]: ", note.label);
            out.lines.extend(prefixed(
                lines.lines,
                Span::raw(marker.clone()).dim(),
                &indent(&marker),
            ));
        }
    }
    out
}

/// Whether Link `i` is broken (or unresolvable).
#[must_use]
pub fn is_broken(resolved: Option<&Resolved>) -> bool {
    resolved.is_none_or(|r| r.status == LinkStatus::Broken)
}

struct Renderer<'a> {
    resolved: &'a [Option<Resolved>],
    selected: Option<usize>,
}

impl Renderer<'_> {
    fn blocks(&self, blocks: &[Block], out: &mut Rendered) {
        for (i, block) in blocks.iter().enumerate() {
            // A blank line between blocks, as in the source.
            if i > 0 {
                out.lines.push(Line::default());
            }
            self.block(block, out);
        }
    }

    fn block(&self, block: &Block, out: &mut Rendered) {
        match block {
            Block::Heading {
                level,
                anchor,
                inlines,
                ..
            } => {
                let style = match level {
                    1 => Style::new()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                    2 => Style::new().fg(Color::Cyan).bold(),
                    _ => Style::new().bold(),
                };
                out.headings.push((anchor.clone(), out.lines.len()));
                for line in self.inlines(inlines, style) {
                    out.lines.push(line);
                }
            }
            Block::Paragraph { inlines } => out.lines.extend(self.inlines(inlines, Style::new())),
            Block::Quote { blocks } => {
                let inner = self.nested(blocks, out);
                out.lines.extend(prefixed(
                    inner,
                    Span::raw("│ ").dim(),
                    &Span::raw("│ ").dim(),
                ));
            }
            Block::Alert { kind, blocks } => {
                let color = match kind.label() {
                    "NOTE" => Color::Blue,
                    "TIP" => Color::Green,
                    "IMPORTANT" => Color::Magenta,
                    "WARNING" => Color::Yellow,
                    _ => Color::Red,
                };
                let bar = Span::styled("┃ ", Style::new().fg(color));
                out.lines.push(Line::from(vec![
                    bar.clone(),
                    Span::styled(kind.label(), Style::new().fg(color).bold()),
                ]));
                let inner = self.nested(blocks, out);
                out.lines.extend(inner.into_iter().map(|mut line| {
                    line.spans.insert(0, bar.clone());
                    line
                }));
            }
            Block::List { start, items } => {
                for (n, item) in items.iter().enumerate() {
                    let marker = match start {
                        Some(first) => format!("{}. ", first + n as u64),
                        None => "• ".to_string(),
                    };
                    self.item(item, &marker, out);
                }
            }
            Block::Code { lang, code } => {
                if let Some(lang) = lang {
                    out.lines
                        .push(Line::from(format!("  {lang}")).dim().italic());
                }
                for line in code.trim_end_matches('\n').split('\n') {
                    out.lines.push(Line::from(vec![
                        Span::raw("  ▏").dim(),
                        Span::styled(line.to_string(), Style::new().fg(Color::Yellow)),
                    ]));
                }
            }
            Block::Table { align, head, rows } => self.table(align, head, rows, out),
            Block::Rule => out.lines.push(Line::from("─".repeat(40)).dim()),
            Block::Html { html } => {
                for line in html.trim_end_matches('\n').split('\n') {
                    out.lines.push(Line::from(line.to_string()).dim());
                }
            }
        }
    }

    /// Nested blocks, with their headings kept (at the outer line numbers).
    fn nested(&self, blocks: &[Block], out: &mut Rendered) -> Vec<Line<'static>> {
        let mut inner = Rendered {
            lines: Vec::new(),
            headings: Vec::new(),
        };
        self.blocks(blocks, &mut inner);
        let base = out.lines.len();
        out.headings
            .extend(inner.headings.into_iter().map(|(a, l)| (a, base + l)));
        inner.lines
    }

    fn item(&self, item: &Item, marker: &str, out: &mut Rendered) {
        let marker = match item.task {
            Some(true) => format!("{marker}[x] "),
            Some(false) => format!("{marker}[ ] "),
            None => marker.to_string(),
        };
        // Items in a list sit tight: no blank lines between their blocks.
        let mut inner = Rendered {
            lines: Vec::new(),
            headings: Vec::new(),
        };
        for block in &item.blocks {
            self.block(block, &mut inner);
        }
        let base = out.lines.len();
        out.headings
            .extend(inner.headings.into_iter().map(|(a, l)| (a, base + l)));
        out.lines.extend(prefixed(
            inner.lines,
            Span::raw(marker.clone()),
            &indent(&marker),
        ));
    }

    fn table(
        &self,
        align: &[Align],
        head: &[Vec<Inline>],
        rows: &[Vec<Vec<Inline>>],
        out: &mut Rendered,
    ) {
        let cell = |inlines: &[Inline], style: Style| -> Line<'static> {
            let mut spans = Vec::new();
            for line in self.inlines(inlines, style) {
                spans.extend(line.spans);
            }
            Line::from(spans)
        };
        let head: Vec<Line> = head.iter().map(|c| cell(c, Style::new().bold())).collect();
        let rows: Vec<Vec<Line>> = rows
            .iter()
            .map(|r| r.iter().map(|c| cell(c, Style::new())).collect())
            .collect();
        let columns = std::iter::once(head.len())
            .chain(rows.iter().map(Vec::len))
            .max()
            .unwrap_or(0);
        let widths: Vec<usize> = (0..columns)
            .map(|c| {
                std::iter::once(&head)
                    .chain(&rows)
                    .filter_map(|r| r.get(c).map(Line::width))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let border = |l: &str, m: &str, r: &str| {
            let inner: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            Line::from(format!("{l}{}{r}", inner.join(m))).dim()
        };
        let row_line = |cells: &[Line<'static>]| {
            let mut spans = vec![Span::raw("│ ").dim()];
            for (c, width) in widths.iter().enumerate() {
                let content = cells.get(c).cloned().unwrap_or_default();
                let pad = width - content.width();
                let (before, after) = match align.get(c) {
                    Some(Align::Right) => (pad, 0),
                    Some(Align::Center) => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                spans.push(Span::raw(" ".repeat(before)));
                spans.extend(content.spans);
                spans.push(Span::raw(" ".repeat(after)));
                spans.push(Span::raw(" │ ").dim());
            }
            if let Some(last) = spans.last_mut() {
                *last = Span::raw(" │").dim();
            }
            Line::from(spans)
        };
        out.lines.push(border("┌", "┬", "┐"));
        out.lines.push(row_line(&head));
        out.lines.push(border("├", "┼", "┤"));
        for row in &rows {
            out.lines.push(row_line(row));
        }
        out.lines.push(border("└", "┴", "┘"));
    }

    /// Inlines as lines: a hard break starts a new line, a soft one is a space.
    fn inlines(&self, inlines: &[Inline], style: Style) -> Vec<Line<'static>> {
        let mut lines = vec![Line::default()];
        self.spans(inlines, style, &mut lines);
        lines
    }

    fn spans(&self, inlines: &[Inline], style: Style, lines: &mut Vec<Line<'static>>) {
        let push = |lines: &mut Vec<Line<'static>>, span: Span<'static>| {
            if let Some(line) = lines.last_mut() {
                line.spans.push(span);
            }
        };
        for inline in inlines {
            match inline {
                Inline::Text { text } => push(lines, Span::styled(text.clone(), style)),
                Inline::Code { code } => {
                    push(lines, Span::styled(code.clone(), style.fg(Color::Yellow)));
                }
                Inline::Emphasis { inlines } => self.spans(inlines, style.italic(), lines),
                Inline::Strong { inlines } => self.spans(inlines, style.bold(), lines),
                Inline::Strikethrough { inlines } => {
                    self.spans(inlines, style.crossed_out(), lines);
                }
                Inline::Link { link, inlines } => {
                    push(lines, Self::number(*link));
                    self.spans(inlines, self.link_style(*link, style), lines);
                }
                Inline::External { inlines, .. } => {
                    self.spans(inlines, style.fg(Color::Blue), lines);
                }
                Inline::Image { link, url, .. } => {
                    if let Some(link) = link {
                        push(lines, Self::number(*link));
                    }
                    let style = match link {
                        Some(link) => self.link_style(*link, style),
                        None => style,
                    };
                    push(
                        lines,
                        Span::styled(format!("[image: {url}]"), style.italic().dim()),
                    );
                }
                Inline::Tag { tag } => {
                    push(
                        lines,
                        Span::styled(format!("#{tag}"), style.fg(Color::Magenta)),
                    );
                }
                Inline::Math { source, display } => {
                    let delim = if *display { "$$" } else { "$" };
                    push(
                        lines,
                        Span::styled(
                            format!("{delim}{}{delim}", source.trim()),
                            style.fg(Color::Green).italic(),
                        ),
                    );
                }
                Inline::FootnoteRef { label } => {
                    push(lines, Span::styled(format!("[^{label}]"), style.dim()));
                }
                Inline::Html { html } => push(lines, Span::styled(html.clone(), style.dim())),
                Inline::SoftBreak => push(lines, Span::styled(" ", style)),
                Inline::HardBreak => lines.push(Line::default()),
            }
        }
    }

    fn number(link: usize) -> Span<'static> {
        Span::styled(
            format!("[{}]", link + 1),
            Style::new().fg(Color::Yellow).bold(),
        )
    }

    fn link_style(&self, link: usize, base: Style) -> Style {
        let broken = is_broken(self.resolved.get(link).and_then(Option::as_ref));
        let style = base
            .fg(if broken { Color::Red } else { Color::Blue })
            .underlined();
        if self.selected == Some(link) {
            style.reversed()
        } else {
            style
        }
    }
}

/// `lines` with `first` before the first line and `rest` before the others.
fn prefixed(
    lines: Vec<Line<'static>>,
    first: Span<'static>,
    rest: &Span<'static>,
) -> Vec<Line<'static>> {
    if lines.is_empty() {
        return vec![Line::from(first)];
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(i, mut line)| {
            line.spans
                .insert(0, if i == 0 { first.clone() } else { rest.clone() });
            line
        })
        .collect()
}

/// Spaces as wide as `marker`, to align the lines after a list marker.
fn indent(marker: &str) -> Span<'static> {
    Span::raw(" ".repeat(Span::raw(marker.to_string()).width()))
}
