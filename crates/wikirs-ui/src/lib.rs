//! What the GUI and TUI share, with no UI code (docs/spec/workspace.md):
//! - [`form`]: the palette's form model, generated from Input schemas;
//! - [`session`]: the open Page and how it reacts to saves and changes on disk
//!   (process-model.md#open-page-changed-on-disk-gui-and-tui).
//!
//! Each UI draws these its own way and keeps only its view state.

use std::{
    path::Path,
    process::{Command, Stdio},
};

pub mod form;
pub mod session;

pub use form::{Choice, Control, Field, FieldError, Form, Item, Unsupported, forms};
pub use session::{Followed, Notice, OpenPage, Session, TreeRow};

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
