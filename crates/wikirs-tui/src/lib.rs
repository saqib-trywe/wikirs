//! The TUI Interface (docs/spec/tui.md): ratatui over the core, calling typed
//! Operations in-process, with `watch` bridged into the event loop
//! (interfaces.md#gui-and-tui).
//!
//! [`App`] holds the state and behaviour, [`ui`] draws it, and [`run`] is the
//! terminal loop around them.

pub mod app;
pub mod form;
pub mod images;
pub mod render;
pub mod result;
pub mod ui;

use std::{io, process::Command, time::Duration};

use ratatui::crossterm::event::{self, Event, KeyEventKind};
use wikirs_core::Wiki;

pub use app::{App, Request};

/// Names the palette lists: every Operation (the parity test compares these
/// with the registry).
#[must_use]
pub fn palette_entries() -> Vec<String> {
    wikirs_ui::Session::palette_matches("")
        .iter()
        .map(|op| op.name.to_string())
        .collect()
}

/// Runs the TUI on `wiki` until the user quits, opening `page` first if given.
pub fn run(wiki: Wiki, page: Option<&str>) -> io::Result<()> {
    let mut app = App::new(wiki);
    // Without a watcher only this process's own changes show up (index_status says so).
    let _ = app.start_watcher();
    if let Some(page) = page {
        app.open(page);
    }
    let mut terminal = ratatui::init();
    app.images = Some(images::Images::detect());
    let outcome = (|| loop {
        terminal.draw(|f| ui::draw(f, &app))?;
        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.key(key);
        }
        app.pump();
        match app.take_request() {
            Some(Request::Editor(text)) => {
                ratatui::restore();
                let edited = edit_externally(&text);
                terminal = ratatui::init();
                app.editor_returned(edited);
            }
            Some(Request::OpenExternal(path)) => wikirs_ui::open_externally(&path),
            None => {}
        }
        if app.should_quit() {
            return Ok(());
        }
    })();
    ratatui::restore();
    outcome
}

/// Edits `text` as a temp file in `$VISUAL`, else `$EDITOR`, else `vi`
/// (`notepad` on Windows), and returns the text it was saved with.
pub fn edit_externally(text: &str) -> io::Result<String> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| if cfg!(windows) { "notepad" } else { "vi" }.to_string());
    edit_with(&editor, text)
}

/// Edits `text` in `editor`, a command with optional arguments (`code -w`).
pub fn edit_with(editor: &str, text: &str) -> io::Result<String> {
    let file = tempfile::Builder::new()
        .prefix("wikirs-")
        .suffix(".md")
        .tempfile()?;
    std::fs::write(file.path(), text)?;
    let mut words = editor.split_whitespace();
    let program = words
        .next()
        .ok_or_else(|| io::Error::other("the editor command is empty"))?;
    let status = Command::new(program)
        .args(words)
        .arg(file.path())
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!("`{editor}` exited with {status}")));
    }
    std::fs::read_to_string(file.path())
}
