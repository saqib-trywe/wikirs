//! What the GUI and TUI share, with no UI code (docs/spec/workspace.md):
//! - [`form`]: the palette's form model, generated from Input schemas;
//! - [`session`]: the open Page and how it reacts to saves and changes on disk
//!   (process-model.md#open-page-changed-on-disk-gui-and-tui).
//!
//! Each UI draws these its own way and keeps only its view state.

pub mod form;
pub mod session;

pub use form::{Choice, Control, Field, FieldError, Form, Item, Unsupported, forms};
pub use session::{Followed, Notice, OpenPage, Session, TreeRow};
