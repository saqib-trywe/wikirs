//! Human output: one renderer per Output type, chosen by the type's schema title,
//! with pretty JSON for the rest (interfaces.md#cli). Any result carrying a
//! Plan renders it: a unified diff on a dry run, a summary once applied.

use std::{fmt::Write, path::Path};

use base64::Engine;
use serde_json::Value;
use similar::TextDiff;
use wikirs_core::plan::{Edit, Plan, splice};

/// Renders `result` of the Operation `op` for a terminal. Bytes, because
/// `read_attachment` prints the Attachment itself.
#[must_use]
pub fn render(op: &str, result: &Value, root: &Path) -> Vec<u8> {
    if let Some(plan) = result.get("plan")
        && let Ok(plan) = serde_json::from_value::<Plan>(plan.clone())
    {
        return if result["applied"] == true {
            summary(&plan)
        } else {
            diff(&plan, root)
        }
        .into_bytes();
    }
    let title = wikirs_core::find(op).map(|info| info.output_schema()["title"].clone());
    let text = match title.as_ref().and_then(Value::as_str) {
        Some("GetPageOutput") => with_newline(str_of(&result["content"])),
        Some("ReadAttachmentOutput") => return attachment(result),
        Some("ListPagesOutput") => pages(result),
        Some("ChildrenOutput") => {
            let mut out = String::new();
            tree(&result["children"], 0, &mut out);
            out
        }
        Some("ListSpacesOutput") => lines(&result["spaces"], |s| {
            format!(
                "{}  {} pages  home: {}",
                str_of(&s["name"]),
                s["page_count"],
                str_or(&s["home_page"], "-")
            )
        }),
        Some("LinksOutput") => lines(&result["links"], |l| {
            let heading = l["heading"]
                .as_str()
                .map(|h| format!("#{h}"))
                .unwrap_or_default();
            format!(
                "{}  → {}{heading}  ({})",
                str_of(&l["raw"]),
                str_or(&l["target"], "?"),
                str_of(&l["status"])
            )
        }),
        Some("BacklinksOutput") => lines(&result["backlinks"], |b| {
            format!("{}  {}", str_of(&b["from"]), str_of(&b["raw"]))
        }),
        Some("Resolved") => {
            let heading = result["heading"]
                .as_str()
                .map(|h| format!("#{h}"))
                .unwrap_or_default();
            format!(
                "{}{heading}  ({} {})\n",
                str_or(&result["target"], "?"),
                str_or(&result["target_kind"], "-"),
                str_of(&result["status"])
            )
        }
        Some("OutlineOutput") => lines(&result["headings"], |h| {
            let level = usize::try_from(h["level"].as_u64().unwrap_or(1)).unwrap_or(1);
            format!(
                "{}{}",
                "  ".repeat(level.saturating_sub(1)),
                str_of(&h["text"])
            )
        }),
        Some("TagTreeOutput") => {
            let mut out = String::new();
            tags(&result["tags"], 0, &mut out);
            out
        }
        Some("CheckOutput") => lines(&result["diagnostics"], |d| diagnostic(d, root)),
        Some("SearchOutput") => search(result),
        Some("ListAttachmentsOutput") => lines(&result["attachments"], |a| {
            format!("{}  {} bytes", str_of(&a["path"]), a["size"])
        }),
        Some("GetConfigOutput") => lines(&result["settings"], |s| {
            format!(
                "{} = {}  ({})",
                str_of(&s["key"]),
                s["value"],
                str_of(&s["source"])
            )
        }),
        Some("IndexStatusOutput") => fields(result),
        _ => format!(
            "{}\n",
            serde_json::to_string_pretty(result).unwrap_or_default()
        ),
    };
    text.into_bytes()
}

/// A dry-run Plan as a unified diff, read against the files as they are now.
#[must_use]
pub fn diff(plan: &Plan, root: &Path) -> String {
    let mut out = String::new();
    for edit in &plan.edits {
        match edit {
            Edit::Create { path, content } => {
                out += &unified("", content, "/dev/null", &side("b", path));
            }
            Edit::CreateBinary { path, size, .. } => {
                let _ = writeln!(out, "new binary file {} ({size} bytes)", side("b", path));
            }
            Edit::Modify { path, splices, .. } => {
                let (a, b) = (side("a", path), side("b", path));
                match std::fs::read_to_string(root.join(path)) {
                    Ok(old) if splices.iter().all(|s| fits(&old, s.range)) => {
                        out += &unified(&old, &splice(&old, splices), &a, &b);
                    }
                    // The file moved on since planning: show the splices alone.
                    _ => {
                        let _ = writeln!(out, "--- {a}\n+++ {b}");
                        for s in splices {
                            let _ = writeln!(out, "@@ bytes {}..{} @@", s.range[0], s.range[1]);
                            out += &prefixed('-', &s.old);
                            out += &prefixed('+', &s.new);
                        }
                    }
                }
            }
            Edit::Move { from, to } => {
                let _ = writeln!(out, "rename from {from}\nrename to {to}");
            }
            Edit::Delete { path } => match std::fs::read_to_string(root.join(path)) {
                Ok(old) => out += &unified(&old, "", &side("a", path), "/dev/null"),
                Err(_) => {
                    let _ = writeln!(out, "deleted file {}", side("a", path));
                }
            },
        }
    }
    if plan.edits.is_empty() {
        out += "nothing to change\n";
    }
    out
}

/// An applied Plan, one line per edit.
#[must_use]
pub fn summary(plan: &Plan) -> String {
    let mut out = String::new();
    for edit in &plan.edits {
        let _ = match edit {
            Edit::Create { path, .. } | Edit::CreateBinary { path, .. } => {
                writeln!(out, "created {path}")
            }
            Edit::Modify { path, .. } => writeln!(out, "modified {path}"),
            Edit::Move { from, to } => writeln!(out, "moved {from} → {to}"),
            Edit::Delete { path } => writeln!(out, "deleted {path}"),
        };
    }
    if plan.edits.is_empty() {
        out += "nothing changed\n";
    }
    out
}

/// `a/eng.md`, as in git; a file outside the Wiki (machine settings) keeps its absolute path.
fn side(prefix: &str, path: &str) -> String {
    if Path::new(path).is_absolute() {
        path.to_string()
    } else {
        format!("{prefix}/{path}")
    }
}

fn fits(text: &str, [start, end]: [usize; 2]) -> bool {
    start <= end && text.get(start..end).is_some()
}

fn unified(old: &str, new: &str, a: &str, b: &str) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(a, b)
        .to_string()
}

fn prefixed(sign: char, text: &str) -> String {
    text.lines().fold(String::new(), |mut out, line| {
        let _ = writeln!(out, "{sign}{line}");
        out
    })
}

fn str_of(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

fn str_or<'v>(value: &'v Value, absent: &'v str) -> &'v str {
    value.as_str().unwrap_or(absent)
}

fn with_newline(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}

fn lines(items: &Value, line: impl Fn(&Value) -> String) -> String {
    items
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| line(item) + "\n")
        .collect()
}

fn pages(result: &Value) -> String {
    let rows = result["pages"].as_array().map_or(&[][..], Vec::as_slice);
    let width = rows
        .iter()
        .map(|p| str_of(&p["path"]).len())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for p in rows {
        let tags = p["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .fold(String::new(), |mut tags, t| {
                let _ = write!(tags, "  #{}", str_of(t));
                tags
            });
        let _ = writeln!(
            out,
            "{:width$}  {}{tags}",
            str_of(&p["path"]),
            str_of(&p["title"])
        );
    }
    if let Some(total) = result["total"].as_u64().filter(|&t| t > rows.len() as u64) {
        let _ = writeln!(out, "({} of {total})", rows.len());
    }
    out
}

fn tree(nodes: &Value, depth: usize, out: &mut String) {
    for node in nodes.as_array().into_iter().flatten() {
        let more = if node["has_children"] == true && !node["children"].is_array() {
            "/…"
        } else {
            ""
        };
        let placeholder = if node["kind"] == "placeholder" {
            "  (placeholder)"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "{}{}{more}  {}{placeholder}",
            "  ".repeat(depth),
            str_of(&node["path"]),
            str_of(&node["title"])
        );
        tree(&node["children"], depth + 1, out);
    }
}

fn tags(nodes: &Value, depth: usize, out: &mut String) {
    for node in nodes.as_array().into_iter().flatten() {
        let _ = writeln!(
            out,
            "{}#{}  {}",
            "  ".repeat(depth),
            str_of(&node["tag"]),
            node["inclusive"]
        );
        tags(&node["children"], depth + 1, out);
    }
}

/// `eng.md:3:5: broken_link: …`, located from the byte range when the file reads.
fn diagnostic(d: &Value, root: &Path) -> String {
    let page = str_of(&d["page"]);
    let at = d["range"][0]
        .as_u64()
        .and_then(|start| usize::try_from(start).ok())
        .and_then(|start| {
            let file = format!("{page}.md");
            let text = std::fs::read_to_string(root.join(&file)).ok()?;
            let before = text.get(..start)?;
            let line = before.matches('\n').count() + 1;
            let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
            Some(format!("{file}:{line}:{column}"))
        })
        .unwrap_or_else(|| page.to_string());
    format!("{at}: {}: {}", str_of(&d["kind"]), str_of(&d["message"]))
}

fn search(result: &Value) -> String {
    let mut out = String::new();
    for hit in result["hits"].as_array().into_iter().flatten() {
        let snippet = str_of(&hit["snippet"])
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(
            out,
            "{}  {}\n    {snippet}",
            str_of(&hit["path"]),
            str_of(&hit["title"])
        );
    }
    let shown = result["hits"].as_array().map_or(0, Vec::len) as u64;
    if let Some(total) = result["total"].as_u64().filter(|&t| t > shown) {
        let _ = writeln!(out, "({shown} of {total})");
    }
    out
}

fn attachment(result: &Value) -> Vec<u8> {
    if let Some(encoded) = result["base64"].as_str()
        && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded)
    {
        return bytes;
    }
    format!(
        "{}  {} bytes{}\n",
        str_of(&result["path"]),
        result["size"],
        result["local_path"]
            .as_str()
            .map(|p| format!("  {p}"))
            .unwrap_or_default()
    )
    .into_bytes()
}

/// `key: value` per field; lists one item per line under their key.
fn fields(result: &Value) -> String {
    let mut out = String::new();
    for (key, value) in result.as_object().into_iter().flatten() {
        match value {
            Value::Array(items) if items.is_empty() => {
                let _ = writeln!(out, "{key}: none");
            }
            Value::Array(items) => {
                let _ = writeln!(out, "{key}:");
                for item in items {
                    let item = item.as_str().map_or_else(|| item.to_string(), String::from);
                    let _ = writeln!(out, "  {item}");
                }
            }
            Value::String(s) => {
                let _ = writeln!(out, "{key}: {s}");
            }
            other => {
                let _ = writeln!(out, "{key}: {other}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use wikirs_core::plan::Splice;

    use super::*;

    #[test]
    fn a_dry_run_modify_is_a_unified_diff_of_the_spliced_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "one\ntwo\nthree\n").unwrap();
        let plan = Plan {
            edits: vec![
                Edit::Modify {
                    path: "a.md".into(),
                    base_version: String::new(),
                    splices: vec![Splice {
                        range: [4, 7],
                        old: "two".into(),
                        new: "2".into(),
                    }],
                },
                Edit::Move {
                    from: "a.md".into(),
                    to: "b.md".into(),
                },
            ],
            warnings: vec![],
        };
        assert_eq!(
            diff(&plan, dir.path()),
            "--- a/a.md\n+++ b/a.md\n@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\nrename from a.md\nrename to b.md\n"
        );
        assert_eq!(summary(&plan), "modified a.md\nmoved a.md → b.md\n");
    }

    #[test]
    fn splices_that_no_longer_fit_are_shown_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "x").unwrap();
        let plan = Plan {
            edits: vec![Edit::Modify {
                path: "a.md".into(),
                base_version: String::new(),
                splices: vec![Splice {
                    range: [4, 7],
                    old: "two".into(),
                    new: "2".into(),
                }],
            }],
            warnings: vec![],
        };
        assert_eq!(
            diff(&plan, dir.path()),
            "--- a/a.md\n+++ b/a.md\n@@ bytes 4..7 @@\n-two\n+2\n"
        );
    }

    #[test]
    fn creates_and_deletes_diff_against_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("gone.md"), "bye\n").unwrap();
        let plan = Plan {
            edits: vec![
                Edit::Create {
                    path: "new.md".into(),
                    content: "hi\n".into(),
                },
                Edit::Delete {
                    path: "gone.md".into(),
                },
            ],
            warnings: vec![],
        };
        assert_eq!(
            diff(&plan, dir.path()),
            "--- /dev/null\n+++ b/new.md\n@@ -0,0 +1 @@\n+hi\n--- a/gone.md\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n"
        );
        let empty = Plan {
            edits: vec![],
            warnings: vec![],
        };
        assert_eq!(diff(&empty, dir.path()), "nothing to change\n");
        assert_eq!(summary(&empty), "nothing changed\n");
    }

    #[test]
    fn files_outside_the_wiki_keep_their_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("machine.toml").display().to_string();
        let plan = Plan {
            edits: vec![Edit::Create {
                path: outside.clone(),
                content: "x = 1\n".into(),
            }],
            warnings: vec![],
        };
        assert_eq!(
            diff(&plan, Path::new("unused")),
            format!("--- /dev/null\n+++ {outside}\n@@ -0,0 +1 @@\n+x = 1\n")
        );
    }

    #[test]
    fn diagnostics_are_located_by_line_and_column() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("eng.md"), "# E\n\nsee [[x]]\n").unwrap();
        let d = serde_json::json!({
            "kind": "broken_link", "page": "eng", "message": "`x` does not exist", "range": [9, 14]
        });
        assert_eq!(
            diagnostic(&d, dir.path()),
            "eng.md:3:5: broken_link: `x` does not exist"
        );
        let gone =
            serde_json::json!({ "kind": "unrecovered_edit", "page": "a.md", "message": "m" });
        assert_eq!(diagnostic(&gone, dir.path()), "a.md: unrecovered_edit: m");
    }
}
