//! wikirs core: the Wiki handle, the Operation registry, and every Operation.
//!
//! Adapters (CLI, MCP, HTTP, TUI, GUI) are generated from [`ops::registry`] and
//! stay thin (ADR 0004).

pub mod error;
pub mod frontmatter;
pub mod index;
pub mod links;
pub mod markdown;
pub mod mutations;
pub mod ops;
pub mod plan;
mod registry;
pub mod rewrite;
mod wiki;

pub use error::{ChangedFile, Error, ErrorKind, Result};
pub use registry::{Kind, OpInfo, Operation, catalogue, find, run_enveloped};
pub use wiki::{PagePath, Wiki, resolve_root};

#[cfg(feature = "clap")]
pub use ops::Command;
pub use ops::registry;
