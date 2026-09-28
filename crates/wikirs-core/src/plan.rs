//! Plans: the file edits a mutation will make, computed first and applied as one unit.
//!
//! Walking skeleton: only `create` edits exist. The write lock, hash checks and
//! roll-forward journal (ADR 0006) arrive in a later slice.

use std::{fs, io::Write, path::Path};

use schemars::JsonSchema;
use serde::Serialize;

use crate::{Error, Result, Wiki};

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Plan {
    pub edits: Vec<Edit>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Edit {
    /// Create a new file. `path` is relative to the Wiki root.
    Create { path: String, content: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Warning {
    pub kind: String,
    pub message: String,
}

impl Plan {
    pub fn apply(&self, wiki: &Wiki) -> Result<()> {
        for edit in &self.edits {
            match edit {
                Edit::Create { path, content } => {
                    let target = wiki.root().join(path);
                    if target.exists() {
                        return Err(Error::already_exists(path));
                    }
                    atomic_write(&target, content.as_bytes())
                        .map_err(|e| Error::io(Some(path), &e))?;
                }
            }
        }
        Ok(())
    }
}

/// Temp file in the same folder, fsync, rename over the target (process-model.md).
fn atomic_write(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = target.parent().expect("target is inside the Wiki");
    fs::create_dir_all(dir)?;
    let name = target
        .file_name()
        .expect("target has a file name")
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.wikirs-tmp"));
    let mut file = fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, target)
}
