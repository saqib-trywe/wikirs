//! Drawing (docs/spec/tui.md#layout-columns-prototype-variant-a): Pages tree |
//! the rendered Page | Backlinks, Links, Outline and Tags; a top bar with the
//! breadcrumb, a banner row when needed, and key hints or the `:` line.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use wikirs_ui::{Notice, OpenPage, Session};

use crate::{
    app::{App, Focus, Mode, PickKind, Picker, ResultPopup},
    form::FormView,
    render,
};

const HINTS: &str = "Tab links  1-9 follow  o open  / search  b backlinks  t tasks  e edit  E $EDITOR  R move  : cmd  ^k ops  q quit";

pub fn draw(f: &mut Frame, app: &App) {
    let [top, banner, main, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::from(app.session.notice.is_some())),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    f.render_widget(top_bar(app), top);
    if let Some(b) = &app.session.notice {
        f.render_widget(banner_line(b), banner);
    }
    let [left, centre, right] = Layout::horizontal([
        Constraint::Length(28),
        Constraint::Fill(1),
        Constraint::Length(34),
    ])
    .areas(main);
    tree(f, app, left);
    page(f, app, centre);
    side(f, app, right);
    f.render_widget(status_line(app), status);
    popups(f, app);
}

fn top_bar(app: &App) -> Line<'static> {
    let mut spans = vec![Span::raw(" wikirs ").bold().reversed(), Span::raw("  ")];
    if let Some(page) = &app.session.page {
        let mut acc = String::new();
        let mut parts = Vec::new();
        for seg in page.path.split('/') {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(seg);
            parts.push(
                app.session
                    .tree
                    .iter()
                    .find(|r| r.path == acc)
                    .map_or_else(|| seg.to_string(), |r| r.title.clone()),
            );
        }
        spans.push(Span::raw(parts.join(" / ")).dim());
        if page.dirty() {
            spans.push(Span::raw("  ● unsaved").red());
        }
        if page.deleted {
            spans.push(Span::raw("  ✗ deleted").red());
        }
    }
    Line::from(spans)
}

fn banner_line(banner: &Notice) -> Line<'static> {
    match banner {
        Notice::Info(m) => Line::from(format!(" {m}  (x dismiss)")).black().on_cyan(),
        Notice::Error(m) => Line::from(format!(" {m}  (x dismiss)")).white().on_red(),
        Notice::Create(path) => Line::from(format!(" No Page `{path}`: c creates it  (x dismiss)"))
            .black()
            .on_yellow(),
        Notice::ChangedOnDisk { .. } => Line::from(
            " CHANGED ON DISK: (Esc first if editing)  [r] Reload   [m] Keep mine   [d] Compare ",
        )
        .white()
        .on_red()
        .bold(),
    }
}

fn focused(on: bool) -> Style {
    if on {
        Style::new().cyan()
    } else {
        Style::new()
    }
}

fn tree(f: &mut Frame, app: &App, area: Rect) {
    let current = app.session.page.as_ref().map(|p| p.path.as_str());
    let items: Vec<ListItem> = app
        .session
        .tree
        .iter()
        .map(|row| {
            let item = ListItem::new(format!("{}{}", "  ".repeat(row.depth), row.title));
            if row.placeholder {
                item.dim().italic()
            } else if Some(row.path.as_str()) == current {
                item.bold()
            } else {
                item
            }
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.tree_sel));
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::bordered()
                    .title(" Pages ")
                    .border_style(focused(app.focus == Focus::Tree)),
            )
            .highlight_style(Style::new().reversed()),
        area,
        &mut state,
    );
}

fn page(f: &mut Frame, app: &App, area: Rect) {
    let Some(page) = &app.session.page else {
        f.render_widget(
            Paragraph::new("No Pages yet: `:create_page <path>` or ctrl-k")
                .block(Block::bordered()),
            area,
        );
        return;
    };
    let title = format!(" {} ", page.title);
    if let Mode::Edit(area_widget) = &app.mode {
        let mut editor = (**area_widget).clone();
        editor.set_block(
            Block::bordered()
                .title(format!("{title}— EDIT (ctrl-s save, Esc done) "))
                .border_style(Style::new().yellow()),
        );
        f.render_widget(&editor, area);
        return;
    }
    let selected = (app.focus == Focus::Links).then_some(app.link_sel);
    let rendered = render::render(&page.doc, &page.resolved, selected, app.images.is_some());
    let images = rendered.images.clone();
    let lines = rendered.lines;
    f.render_widget(
        Paragraph::new(Text::from(lines.clone()))
            .wrap(Wrap { trim: false })
            .scroll((app.scroll, 0))
            .block(Block::bordered().title(title)),
        area,
    );
    if let Some(store) = &app.images {
        let inner = area.inner(ratatui::layout::Margin::new(1, 1));
        let root = app.session.wiki().root().to_path_buf();
        for (line, target) in images {
            // Where the kept lines land once everything above them is wrapped.
            let above = Paragraph::new(Text::from(lines[..line].to_vec()))
                .wrap(Wrap { trim: false })
                .line_count(inner.width);
            let Some(top) = above.checked_sub(usize::from(app.scroll)) else {
                continue; // scrolled past
            };
            let rows = render::IMAGE_ROWS;
            // Only whole pictures: a cut-off one can't be drawn faithfully.
            if top + rows > usize::from(inner.height) {
                continue;
            }
            let rect = Rect {
                x: inner.x,
                y: inner.y + u16::try_from(top).unwrap_or(u16::MAX),
                width: inner.width,
                height: u16::try_from(rows).unwrap_or(u16::MAX),
            };
            let file = target.split('/').fold(root.clone(), |p, seg| p.join(seg));
            store.draw(f, rect, &file);
        }
    }
}

fn side(f: &mut Frame, app: &App, area: Rect) {
    let mut lines = Vec::new();
    if let Some(page) = &app.session.page {
        side_lines(app, page, &mut lines);
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().border_style(focused(app.focus == Focus::Links))),
        area,
    );
}

fn side_lines(app: &App, page: &OpenPage, lines: &mut Vec<Line<'static>>) {
    let heading = |text: &str| Line::from(text.to_string()).dim().bold();
    lines.push(heading("BACKLINKS"));
    if page.backlinks.is_empty() {
        lines.push(Line::from("  none").dim());
    }
    for from in &page.backlinks {
        let title = app
            .session
            .tree
            .iter()
            .find(|r| &r.path == from)
            .map_or(from.as_str(), |r| r.title.as_str());
        lines.push(Line::from(format!("  {title}")).blue());
    }
    lines.push(Line::default());
    lines.push(heading("LINKS"));
    for (i, link) in page.doc.links.iter().enumerate() {
        let resolved = page.resolved.get(i).and_then(Option::as_ref);
        let broken = render::is_broken(resolved);
        let target = resolved.map_or(link.target.as_str(), |r| r.target.as_str());
        let mut line = Line::from(vec![
            Span::raw(format!("  [{}] ", i + 1)).yellow(),
            Span::raw(format!("{}{target}", if broken { "✗ " } else { "" })),
        ]);
        line = if broken { line.red() } else { line.blue() };
        if app.focus == Focus::Links && i == app.link_sel {
            line = line.reversed();
        }
        lines.push(line);
    }
    lines.push(Line::default());
    lines.push(heading("OUTLINE"));
    for (level, text) in &page.outline {
        lines.push(Line::from(format!(
            "{}{text}",
            "  ".repeat(usize::from(*level))
        )));
    }
    lines.push(Line::default());
    lines.push(heading("TAGS"));
    let tags: Vec<String> = page.tags.iter().map(|t| format!("#{t}")).collect();
    lines.push(Line::from(format!("  {}", tags.join(" "))).magenta());
}

fn status_line(app: &App) -> Line<'static> {
    match &app.mode {
        Mode::Command { line, hint } => {
            let mut spans = vec![Span::raw(format!(":{line}█")).bold()];
            if let Some(hint) = hint {
                spans.push(Span::raw(format!("   {hint}")).dim());
            }
            Line::from(spans)
        }
        Mode::Edit(_) => Line::from(" -- EDIT --   ctrl-s save   Esc back to the view").yellow(),
        _ => {
            let digits = if app.digits.is_empty() {
                String::new()
            } else {
                format!(" [{}…] ", app.digits)
            };
            Line::from(vec![
                Span::raw(digits).yellow().bold(),
                Span::raw(format!(" {HINTS}")).dim(),
            ])
        }
    }
}

/// A cleared box `w`×`h`, centred near the top.
fn popup_area(f: &mut Frame, w: u16, h: u16) -> Rect {
    let full = f.area();
    let area = Rect {
        x: full.width.saturating_sub(w) / 2,
        y: 2.min(full.height),
        width: w.min(full.width),
        height: h.min(full.height.saturating_sub(2)),
    };
    f.render_widget(Clear, area);
    area
}

fn popups(f: &mut Frame, app: &App) {
    match &app.mode {
        Mode::Palette { query, sel } => palette(f, query, *sel),
        Mode::Form(form) => form_popup(f, form),
        Mode::Pick(picker) => pick(f, picker),
        Mode::Result(popup) => result_popup(f, popup),
        _ => {}
    }
    if let (Some(Notice::ChangedOnDisk { disk, .. }), Some(page), true) =
        (&app.session.notice, &app.session.page, app.compare)
    {
        let area = popup_area(f, 120, 24);
        let [mine, theirs] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(area);
        f.render_widget(
            Paragraph::new(page.buffer.clone()).block(
                Block::bordered()
                    .title(" Mine (unsaved) ")
                    .border_style(Style::new().fg(Color::Yellow)),
            ),
            mine,
        );
        f.render_widget(
            Paragraph::new(disk.clone()).block(Block::bordered().title(" On disk (d closes) ")),
            theirs,
        );
    }
}

fn palette(f: &mut Frame, query: &str, sel: usize) {
    let area = popup_area(f, 90, 24);
    let ops = Session::palette_matches(query);
    let items: Vec<ListItem> = ops
        .iter()
        .map(|op| {
            ListItem::new(Line::from(vec![
                Span::raw(format!("{:<18}", op.name)),
                Span::raw(format!("{:<13}", format!("{:?}", op.kind).to_lowercase())).dim(),
                Span::raw(op.description).dim(),
            ]))
        })
        .collect();
    let [q, list] = Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(area);
    f.render_widget(
        Paragraph::new(format!("{query}█")).block(Block::bordered().title(" Operations ")),
        q,
    );
    let mut state = ListState::default().with_selected(Some(sel));
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered())
            .highlight_style(Style::new().reversed()),
        list,
        &mut state,
    );
}

fn form_popup(f: &mut Frame, form: &FormView) {
    let lines = form.lines();
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = popup_area(f, 90, height);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" ctrl-k ")),
        area,
    );
}

fn pick(f: &mut Frame, picker: &Picker) {
    let title = match picker.kind {
        PickKind::Open => " Open a Page (list_pages) ",
        PickKind::Search => " Search ",
        PickKind::Backlinks => " Backlinks (Enter opens) ",
        PickKind::Tasks => " Tasks (Enter ticks, Esc closes) ",
    };
    let area = popup_area(f, 80, 18);
    let items: Vec<ListItem> = picker
        .items
        .iter()
        .map(|(_, label, detail)| {
            ListItem::new(Line::from(vec![
                Span::raw(format!("{label:<24} ")),
                Span::raw(detail.clone()).dim(),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(picker.sel));
    let list = List::new(items).highlight_style(Style::new().reversed());
    if matches!(picker.kind, PickKind::Backlinks | PickKind::Tasks) {
        f.render_stateful_widget(list.block(Block::bordered().title(title)), area, &mut state);
    } else {
        let [q, rest] = Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(area);
        f.render_widget(
            Paragraph::new(format!("{}█", picker.query)).block(Block::bordered().title(title)),
            q,
        );
        f.render_stateful_widget(list.block(Block::bordered()), rest, &mut state);
    }
}

fn result_popup(f: &mut Frame, popup: &ResultPopup) {
    let height = u16::try_from(popup.lines.len() + 2)
        .unwrap_or(u16::MAX)
        .max(5);
    let area = popup_area(f, 90, height);
    let keys = if popup.apply.is_some() {
        "a apply · Esc"
    } else {
        "Esc"
    };
    f.render_widget(
        Paragraph::new(popup.lines.clone())
            .scroll((popup.scroll, 0))
            .block(Block::bordered().title(format!(" {} ({keys}) ", popup.title))),
        area,
    );
}
