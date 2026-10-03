//! What the GUI and TUI share, with no UI code (docs/spec/workspace.md):
//! - [`form`]: the palette's form model, generated from Input schemas;
//! - [`outcome`]: an Operation's result or Plan as lines for a popup;
//! - [`session`]: the open Page and how it reacts to saves and changes on disk
//!   (process-model.md#open-page-changed-on-disk-gui-and-tui).
//!
//! Each UI draws these its own way and keeps only its view state.

use std::{
    path::Path,
    process::{Command, Stdio},
};

pub mod form;
pub mod outcome;
pub mod session;

pub use form::{Choice, Control, Field, FieldError, Form, Item, Row, Step, Unsupported, forms};
pub use session::{Followed, Notice, OpenPage, Session, TreeRow};

/// A search snippet without its `**` match markers, and the ranges they marked.
#[must_use]
pub fn snippet_matches(snippet: &str) -> (String, Vec<std::ops::Range<usize>>) {
    let mut text = String::with_capacity(snippet.len());
    let mut ranges = Vec::new();
    let mut open: Option<usize> = None;
    let mut rest = snippet;
    while let Some(at) = rest.find("**") {
        text.push_str(&rest[..at]);
        match open.take() {
            Some(start) => ranges.push(start..text.len()),
            None => open = Some(text.len()),
        }
        rest = &rest[at + 2..];
    }
    text.push_str(rest);
    (text, ranges)
}

/// Opens a file in the system viewer, without waiting for it.
pub fn open_externally(path: &Path) {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        Command::new("xdg-open")
    };
    let _ = command
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::snippet_matches;

    #[test]
    fn snippet_markers_become_ranges() {
        let (text, ranges) = snippet_matches("a **Pin** b **pin**!");
        assert_eq!(text, "a Pin b pin!");
        assert_eq!(ranges, [2..5, 8..11]);
        // An unclosed marker is dropped, its text kept.
        assert_eq!(snippet_matches("x **y"), ("x y".to_string(), vec![]));
    }
}
