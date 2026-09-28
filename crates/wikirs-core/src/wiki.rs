//! The Wiki handle, Page Paths, and finding a Wiki's root (docs/spec/wiki-selection.md).

use std::path::{Path, PathBuf};

use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct Wiki {
    root: PathBuf,
}

impl Wiki {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let given = root.as_ref();
        let root = given
            .canonicalize()
            .ok()
            .filter(|p| p.is_dir())
            .ok_or_else(|| Error::not_found("wiki", &given.display().to_string()))?;
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `.md` file for a (validated) Page Path.
    #[must_use]
    pub fn page_file(&self, page: &PagePath) -> PathBuf {
        self.root.join(format!("{}.md", page.as_str()))
    }
}

/// Finds the root for CLI-style Interfaces: `--wiki`, then `WIKIRS_WIKI`, then
/// walking up from `cwd` to the nearest folder containing `.wikirs/`.
///
/// Walking skeleton: named Wikis and `default_wiki` from the user config come later.
pub fn resolve_root(
    flag: Option<&str>,
    env: Option<&str>,
    cwd: &Path,
    walk_up: bool,
) -> Result<PathBuf> {
    if let Some(given) = flag.or(env) {
        return Ok(expand_home(given));
    }
    if walk_up && let Some(found) = cwd.ancestors().find(|dir| dir.join(".wikirs").is_dir()) {
        return Ok(found.to_path_buf());
    }
    let mut err = Error::not_found("wiki", &cwd.display().to_string());
    err.message = "no Wiki found: run 'wikirs init' here, pass --wiki, or set default_wiki".into();
    Err(err)
}

fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// A validated Page Path: relative to the Wiki root, no `.md`, no hidden segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagePath(String);

impl PagePath {
    pub fn parse(raw: &str) -> Result<Self> {
        let bad = |reason| Err(Error::invalid_path(raw, reason));
        if raw.is_empty() {
            return bad("empty");
        }
        if raw.contains('\\') || raw.contains('\0') {
            return bad("malformed");
        }
        if std::path::Path::new(raw)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            return bad("page paths have no .md extension");
        }
        for segment in raw.split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return bad("malformed");
            }
            if segment.starts_with('.') {
                return bad("hidden segment");
            }
        }
        Ok(Self(raw.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last segment, used as the Title when there is no H1.
    #[must_use]
    pub fn file_stem(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }
}

/// Rejects a new path whose file or any folder differs from an existing entry
/// only by case (Page identity decision, item 10).
pub fn check_case_conflict(wiki: &Wiki, page: &PagePath) -> Result<()> {
    let segments: Vec<&str> = page.as_str().split('/').collect();
    let mut dir = wiki.root().to_path_buf();
    let mut prefix = String::new();
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        let wanted: Vec<String> = if last {
            vec![format!("{segment}.md"), (*segment).to_string()]
        } else {
            vec![(*segment).to_string()]
        };
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                for want in &wanted {
                    if name != *want && name.to_lowercase() == want.to_lowercase() {
                        let existing = format!("{prefix}{}", name.trim_end_matches(".md"));
                        return Err(Error::case_conflict(page.as_str(), &existing));
                    }
                }
            }
        }
        prefix.push_str(segment);
        prefix.push('/');
        dir = dir.join(segment);
    }
    Ok(())
}

/// Filename slug from a Title: Unicode letters and digits kept and lowercased,
/// everything else becomes `-` (On-disk layout decision, item 8).
#[must_use]
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_matches_the_spec_example() {
        assert_eq!(slugify("Async Notes, Part 2"), "async-notes-part-2");
        assert_eq!(slugify("  Größe & Ärger "), "größe-ärger");
    }

    #[test]
    fn page_paths_reject_malformed_input() {
        for bad in ["", "a//b", "/a", "a/", "../a", "a/.hidden", "a.md", "a\\b"] {
            assert!(PagePath::parse(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(PagePath::parse("eng/rust/async-notes").is_ok());
    }
}
