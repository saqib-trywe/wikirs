//! Resolving Links against the Wiki (Page identity decision; operations.md).
//!
//! The Index stores each Link's target *as written, normalised to a path from
//! the Wiki root*; whether that path exists is decided at query time, so
//! creating a Page instantly fixes every Link that pointed at it.

use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::Serialize;

use crate::{
    Error, Result,
    markdown::{RawLink, anchor},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkStatus {
    Ok,
    Broken,
    HeadingMissing,
    CaseFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Page,
    Attachment,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Resolved {
    /// Page Path (no `.md`) or Attachment path.
    pub target: String,
    pub target_kind: TargetKind,
    pub heading: Option<String>,
    pub status: LinkStatus,
}

/// The Link's target as a path from the Wiki root (no `#heading`), or `None`
/// if a relative Link climbs out of the Wiki.
#[must_use]
pub fn normalize_dest(src_file: &str, link: &RawLink) -> Option<String> {
    let target = link.target.trim();
    if link.wiki || target.starts_with('/') {
        return normalize(target.trim_start_matches('/'));
    }
    let dir = src_file.rsplit_once('/').map_or("", |(d, _)| d);
    normalize(&if dir.is_empty() {
        target.to_string()
    } else {
        format!("{dir}/{target}")
    })
}

/// Collapses `.` and `..` segments; `None` if `..` climbs above the root.
fn normalize(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            s => out.push(s),
        }
    }
    (!out.is_empty()).then(|| out.join("/"))
}

/// Candidate files a stored target can mean, most likely first:
/// `(file path, kind)`.
fn candidates(dest: &str, wiki: bool) -> Vec<(String, TargetKind)> {
    if let Some(page) = dest.strip_suffix(".md") {
        return vec![(format!("{page}.md"), TargetKind::Page)];
    }
    if wiki {
        // `[[eng/rust]]` names a Page; `[[eng/diagram.png]]` an Attachment.
        // The likelier one goes first: it's what a broken Link reports.
        let page = (format!("{dest}.md"), TargetKind::Page);
        let attachment = (dest.to_string(), TargetKind::Attachment);
        let file_name = dest.rsplit('/').next().unwrap_or(dest);
        if file_name.contains('.') {
            vec![attachment, page]
        } else {
            vec![page, attachment]
        }
    } else {
        vec![(dest.to_string(), TargetKind::Attachment)]
    }
}

fn kind_str(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Page => "page",
        TargetKind::Attachment => "attachment",
    }
}

fn display(file: &str, kind: TargetKind) -> String {
    match kind {
        TargetKind::Page => file.strip_suffix(".md").unwrap_or(file).to_string(),
        TargetKind::Attachment => file.to_string(),
    }
}

fn db_err(e: &rusqlite::Error) -> Error {
    Error::internal(format!("index: {e}"))
}

/// Resolves a stored target against the files currently in the Index.
pub fn resolve(
    conn: &Connection,
    dest: Option<&str>,
    wiki: bool,
    heading: Option<&str>,
) -> Result<Resolved> {
    let heading = heading.map(str::to_string);
    let Some(dest) = dest else {
        return Ok(Resolved {
            target: String::new(),
            target_kind: TargetKind::Page,
            heading,
            status: LinkStatus::Broken,
        });
    };
    let cands = candidates(dest, wiki);
    let mut found = None;
    for (file, kind) in &cands {
        let exists: Option<String> = conn
            .query_row(
                "SELECT path FROM files WHERE path = ?1 AND kind = ?2",
                params![file, kind_str(*kind)],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| db_err(&e))?;
        if let Some(path) = exists {
            found = Some((path, *kind, false));
            break;
        }
    }
    if found.is_none() {
        let mut matches = Vec::new();
        for (file, kind) in &cands {
            let mut stmt = conn
                .prepare("SELECT path FROM files WHERE lower(path) = lower(?1) AND kind = ?2")
                .map_err(|e| db_err(&e))?;
            let rows: Vec<String> = stmt
                .query_map(params![file, kind_str(*kind)], |r| r.get(0))
                .map_err(|e| db_err(&e))?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| db_err(&e))?;
            matches.extend(rows.into_iter().map(|p| (p, *kind)));
        }
        if let [(path, kind)] = matches.as_slice() {
            found = Some((path.clone(), *kind, true));
        }
    }
    let Some((file, kind, fallback)) = found else {
        let (file, kind) = &cands[0];
        return Ok(Resolved {
            target: display(file, *kind),
            target_kind: *kind,
            heading,
            status: LinkStatus::Broken,
        });
    };
    let heading_missing = match (&heading, kind) {
        (Some(h), TargetKind::Page) => conn
            .query_row(
                "SELECT 1 FROM headings WHERE page = ?1 AND anchor = ?2",
                params![file, anchor(h)],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| db_err(&e))?
            .is_none(),
        _ => false,
    };
    let status = if heading_missing {
        LinkStatus::HeadingMissing
    } else if fallback {
        LinkStatus::CaseFallback
    } else {
        LinkStatus::Ok
    };
    Ok(Resolved {
        target: display(&file, kind),
        target_kind: kind,
        heading,
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parse;

    fn dest(src: &str, text: &str) -> Option<String> {
        normalize_dest(src, &parse(text).links[0])
    }

    #[test]
    fn targets_normalise_from_the_right_base() {
        assert_eq!(
            dest("eng/rust.md", "[[eng/other]]").as_deref(),
            Some("eng/other")
        );
        assert_eq!(
            dest("eng/rust.md", "[x](async.md)").as_deref(),
            Some("eng/async.md")
        );
        assert_eq!(
            dest("eng/rust.md", "[x](../notes%20x.md)").as_deref(),
            Some("notes x.md")
        );
        assert_eq!(
            dest("eng/rust.md", "[x](/a/b.md)").as_deref(),
            Some("a/b.md")
        );
        assert_eq!(
            dest("eng/rust.md", "[x](../../out.md)"),
            None,
            "climbs out of the Wiki"
        );
        assert_eq!(dest("top.md", "![](pic.png)").as_deref(), Some("pic.png"));
    }
}
