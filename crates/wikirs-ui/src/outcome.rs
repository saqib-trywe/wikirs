//! An Operation's outcome as lines for a result popup, with no UI code: a Plan
//! as its edits (each splice as `-old` / `+new`), any other result as JSON,
//! then warnings; or the error. Each UI styles the lines by their [`Kind`].

use serde_json::Value;
use wikirs_core::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An error's first line.
    Error,
    /// "Applied" / "Dry run" heading of a Plan.
    Applied,
    DryRun,
    /// A file the Plan creates, removes, moves or edits.
    Create,
    Delete,
    Move,
    Modify,
    /// Splice text: removed and added.
    Removed,
    Added,
    Warning,
    /// JSON and other details.
    Plain,
    Muted,
}

/// One line of an outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeLine {
    pub kind: Kind,
    pub text: String,
}

fn line(kind: Kind, text: impl Into<String>) -> OutcomeLine {
    OutcomeLine {
        kind,
        text: text.into(),
    }
}

/// Lines for `{ result, warnings }` or an error.
#[must_use]
pub fn lines(outcome: &Result<Value, Error>) -> Vec<OutcomeLine> {
    match outcome {
        Err(err) => {
            let mut lines = vec![line(
                Kind::Error,
                format!("error[{}]: {}", kind_name(err), err.message),
            )];
            if !err.details.is_null() {
                lines.extend(json_lines(&err.details, Kind::Muted));
            }
            lines
        }
        Ok(envelope) => {
            let result = &envelope["result"];
            let mut lines = match result.get("plan") {
                Some(plan) => plan_lines(result, plan),
                None => json_lines(result, Kind::Plain),
            };
            if let Some(warnings) = envelope["warnings"].as_array().filter(|w| !w.is_empty()) {
                lines.push(line(Kind::Plain, ""));
                for warning in warnings {
                    let message = warning["message"].as_str().unwrap_or_default();
                    lines.push(line(Kind::Warning, format!("warning: {message}")));
                }
            }
            lines
        }
    }
}

/// Whether an outcome is a dry run that could be applied.
#[must_use]
pub fn is_dry_run(outcome: &Result<Value, Error>) -> bool {
    outcome.as_ref().is_ok_and(|envelope| {
        envelope["result"].get("plan").is_some() && envelope["result"]["applied"] == false
    })
}

fn kind_name(err: &Error) -> String {
    serde_json::to_value(err.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn plan_lines(result: &Value, plan: &Value) -> Vec<OutcomeLine> {
    let applied = result["applied"].as_bool().unwrap_or(false);
    let mut lines = vec![if applied {
        line(Kind::Applied, "Applied:")
    } else {
        line(Kind::DryRun, "Dry run, nothing written.")
    }];
    let edits = plan["edits"].as_array().map_or(&[][..], Vec::as_slice);
    if edits.is_empty() {
        lines.push(line(Kind::Muted, "  no changes"));
    }
    for edit in edits {
        let s = |key: &str| edit[key].as_str().unwrap_or_default().to_string();
        match edit["op"].as_str().unwrap_or_default() {
            "create" => {
                let n = edit["content"].as_str().map_or(0, |c| c.lines().count());
                lines.push(line(
                    Kind::Create,
                    format!("  create {} ({n} lines)", s("path")),
                ));
            }
            "create_binary" => lines.push(line(
                Kind::Create,
                format!("  create {} ({} bytes)", s("path"), edit["size"]),
            )),
            "modify" => {
                lines.push(line(Kind::Modify, format!("  modify {}", s("path"))));
                for splice in edit["splices"].as_array().into_iter().flatten() {
                    for (sign, key, kind) in
                        [("-", "old", Kind::Removed), ("+", "new", Kind::Added)]
                    {
                        let text = splice[key].as_str().unwrap_or_default();
                        if !text.is_empty() {
                            for part in text.split('\n') {
                                lines.push(line(kind, format!("    {sign}{part}")));
                            }
                        }
                    }
                }
            }
            "move" => lines.push(line(
                Kind::Move,
                format!("  move {} → {}", s("from"), s("to")),
            )),
            "delete" => lines.push(line(Kind::Delete, format!("  delete {}", s("path")))),
            other => lines.push(line(Kind::Plain, format!("  {other}"))),
        }
    }
    // What the Operation reports besides its Plan, e.g. `links_rewritten`.
    let extra: serde_json::Map<String, Value> = result
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, v)| k.as_str() != "plan" && k.as_str() != "applied" && !v.is_null())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !extra.is_empty() {
        lines.push(line(Kind::Plain, ""));
        lines.extend(json_lines(&Value::Object(extra), Kind::Muted));
    }
    lines
}

fn json_lines(value: &Value, kind: Kind) -> Vec<OutcomeLine> {
    serde_json::to_string_pretty(value)
        .unwrap_or_default()
        .lines()
        .map(|l| line(kind, l))
        .collect()
}

/// A line diff from `old` to `new`, for comparing a buffer with the disk:
/// `-` removed, `+` added, unchanged lines indented.
#[must_use]
pub fn diff_lines(old: &str, new: &str) -> Vec<OutcomeLine> {
    similar::TextDiff::from_lines(old, new)
        .iter_all_changes()
        .map(|change| {
            let text = change.value().trim_end_matches('\n');
            match change.tag() {
                similar::ChangeTag::Delete => line(Kind::Removed, format!("-{text}")),
                similar::ChangeTag::Insert => line(Kind::Added, format!("+{text}")),
                similar::ChangeTag::Equal => line(Kind::Plain, format!(" {text}")),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_plan_with_its_splices_and_warnings() {
        let outcome = Ok(json!({
            "result": {
                "applied": false,
                "plan": {"edits": [
                    {"op": "modify", "path": "a.md", "splices": [{"old": "[[x]]", "new": "[[y]]"}]},
                    {"op": "move", "from": "x.md", "to": "y.md"},
                    {"op": "create", "path": "n.md", "content": "1\n2\n"},
                    {"op": "delete", "path": "d.md"},
                ]},
                "links_rewritten": 1,
            },
            "warnings": [{"kind": "k", "message": "careful"}],
        }));
        assert!(is_dry_run(&outcome));
        let got: Vec<_> = lines(&outcome)
            .into_iter()
            .map(|l| (l.kind, l.text))
            .collect();
        let want = [
            (Kind::DryRun, "Dry run, nothing written."),
            (Kind::Modify, "  modify a.md"),
            (Kind::Removed, "    -[[x]]"),
            (Kind::Added, "    +[[y]]"),
            (Kind::Move, "  move x.md → y.md"),
            (Kind::Create, "  create n.md (2 lines)"),
            (Kind::Delete, "  delete d.md"),
            (Kind::Plain, ""),
            (Kind::Muted, "{"),
            (Kind::Muted, "  \"links_rewritten\": 1"),
            (Kind::Muted, "}"),
            (Kind::Plain, ""),
            (Kind::Warning, "warning: careful"),
        ];
        let want: Vec<_> = want.iter().map(|(k, t)| (*k, (*t).to_string())).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn errors_queries_and_applied_plans() {
        let err = Err(Error::not_found("page", "x"));
        let got = lines(&err);
        assert_eq!(got[0].kind, Kind::Error);
        assert_eq!(got[0].text, "error[not_found]: page `x` does not exist");
        assert!(!is_dry_run(&err));
        let query = Ok(json!({"result": {"n": 1}, "warnings": []}));
        assert_eq!(lines(&query).len(), 3);
        assert!(!is_dry_run(&query));
        let applied =
            Ok(json!({"result": {"applied": true, "plan": {"edits": []}}, "warnings": []}));
        assert!(!is_dry_run(&applied));
        assert_eq!(lines(&applied)[0].kind, Kind::Applied);
        assert_eq!(lines(&applied)[1].text, "  no changes");
    }

    #[test]
    fn diffs_by_line() {
        let got: Vec<_> = diff_lines("a\nb\nc\n", "a\nB\nc\n")
            .into_iter()
            .map(|l| (l.kind, l.text))
            .collect();
        let want = [
            (Kind::Plain, " a"),
            (Kind::Removed, "-b"),
            (Kind::Added, "+B"),
            (Kind::Plain, " c"),
        ];
        assert_eq!(got, want.map(|(k, t)| (k, t.to_string())));
    }
}
