//! The closed set of Operation error kinds (docs/spec/errors.md).

use std::fmt;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotFound,
    AlreadyExists,
    CaseConflict,
    InvalidPath,
    InvalidInput,
    Conflict,
    NoMatch,
    AmbiguousMatch,
    Io,
    Internal,
}

/// An Operation failure. `kind` and `details` are the stable contract;
/// `message` is prose for humans.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub details: Value,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    #[must_use]
    pub fn new(kind: ErrorKind, message: impl Into<String>, details: Value) -> Self {
        Self {
            kind,
            message: message.into(),
            details,
        }
    }

    #[must_use]
    pub fn not_found(what: &str, path: &str) -> Self {
        Self::new(
            ErrorKind::NotFound,
            format!("{what} `{path}` does not exist"),
            json!({ "what": what, "path": path }),
        )
    }

    #[must_use]
    pub fn already_exists(path: &str) -> Self {
        Self::new(
            ErrorKind::AlreadyExists,
            format!("`{path}` already exists"),
            json!({ "path": path }),
        )
    }

    #[must_use]
    pub fn case_conflict(path: &str, existing: &str) -> Self {
        Self::new(
            ErrorKind::CaseConflict,
            format!("`{path}` differs from existing `{existing}` only by case"),
            json!({ "path": path, "existing": existing }),
        )
    }

    #[must_use]
    pub fn invalid_path(path: &str, reason: &str) -> Self {
        Self::new(
            ErrorKind::InvalidPath,
            format!("invalid path `{path}`: {reason}"),
            json!({ "path": path, "reason": reason }),
        )
    }

    #[must_use]
    pub fn invalid_input(field: Option<&str>, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self::new(
            ErrorKind::InvalidInput,
            reason.clone(),
            json!({ "field": field, "reason": reason }),
        )
    }

    #[must_use]
    pub fn io(path: Option<&str>, err: &std::io::Error) -> Self {
        Self::new(
            ErrorKind::Io,
            err.to_string(),
            json!({ "path": path, "os_error": err.to_string() }),
        )
    }

    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message, json!({}))
    }

    /// The wire form: `{ "error": { kind, message, details } }`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({ "error": self })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = serde_json::to_value(self.kind).ok();
        let kind = kind.as_ref().and_then(Value::as_str).unwrap_or("error");
        write!(f, "error[{kind}]: {}", self.message)
    }
}

impl std::error::Error for Error {}
