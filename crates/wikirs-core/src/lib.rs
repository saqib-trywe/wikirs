//! wikirs core: the Wiki handle, the Operation registry, and every Operation.
//!
//! Adapters (CLI, MCP, HTTP, TUI, GUI) are generated from [`ops::registry`] and
//! stay thin (ADR 0004).

mod error;
pub mod ops;
pub mod plan;
mod registry;
mod wiki;

pub use error::{Error, ErrorKind, Result};
pub use registry::{Kind, OpInfo, Operation, catalogue, find, run_enveloped};
pub use wiki::{PagePath, Wiki, resolve_root};

#[cfg(feature = "clap")]
pub use ops::Command;
pub use ops::registry;
