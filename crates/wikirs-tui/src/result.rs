//! An Operation's outcome as popup lines: a Plan as its edits (each splice as
//! `-old` / `+new`), any other result as JSON, then warnings; or the error.

use ratatui::{
    style::{Color, Style, Stylize},
    text::Line,
};
use serde_json::Value;
use wikirs_core::Error;

/// Lines for `{ result, warnings }` or an error.
#[must_use]
pub fn lines(outcome: &Result<Value, Error>) -> Vec<Line<'static>> {
    match outcome {
        Err(err) => {
            let mut lines = vec![
                Line::from(format!("error[{}]: {}", kind_name(err), err.message))
                    .red()
                    .bold(),
            ];
            if !err.details.is_null() {
                lines.extend(
                    json_lines(&err.details)
                        .into_iter()
                        .map(ratatui::prelude::Stylize::dim),
                );
            }
            lines
        }
        Ok(envelope) => {
            let result = &envelope["result"];
            let mut lines = match result.get("plan") {
                Some(plan) => plan_lines(result, plan),
                None => json_lines(result),
            };
            if let Some(warnings) = envelope["warnings"].as_array().filter(|w| !w.is_empty()) {
                lines.push(Line::default());
                for warning in warnings {
                    let message = warning["message"].as_str().unwrap_or_default();
                    lines.push(Line::from(format!("warning: {message}")).yellow());
                }
            }
            lines
        }
    }
}

fn kind_name(err: &Error) -> String {
    serde_json::to_value(err.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn plan_lines(result: &Value, plan: &Value) -> Vec<Line<'static>> {
    let applied = result["applied"].as_bool().unwrap_or(false);
    let mut lines = vec![if applied {
        Line::from("Applied:").green().bold()
    } else {
        Line::from("Dry run, nothing written (a: apply):")
            .yellow()
            .bold()
    }];
    let edits = plan["edits"].as_array().map_or(&[][..], Vec::as_slice);
    if edits.is_empty() {
        lines.push(Line::from("  no changes").dim());
    }
    for edit in edits {
        let s = |key: &str| edit[key].as_str().unwrap_or_default().to_string();
        match edit["op"].as_str().unwrap_or_default() {
            "create" => {
                let n = edit["content"].as_str().map_or(0, |c| c.lines().count());
                lines.push(Line::from(format!("  create {} ({n} lines)", s("path"))).green());
            }
            "create_binary" => lines.push(
                Line::from(format!("  create {} ({} bytes)", s("path"), edit["size"])).green(),
            ),
            "modify" => {
                lines.push(Line::from(format!("  modify {}", s("path"))).bold());
                for splice in edit["splices"].as_array().into_iter().flatten() {
                    for (sign, key, color) in [("-", "old", Color::Red), ("+", "new", Color::Green)]
                    {
                        let text = splice[key].as_str().unwrap_or_default();
                        if !text.is_empty() {
                            for part in text.split('\n') {
                                lines.push(Line::styled(
                                    format!("    {sign}{part}"),
                                    Style::new().fg(color),
                                ));
                            }
                        }
                    }
                }
            }
            "move" => lines.push(Line::from(format!("  move {} → {}", s("from"), s("to"))).cyan()),
            "delete" => lines.push(Line::from(format!("  delete {}", s("path"))).red()),
            other => lines.push(Line::from(format!("  {other}"))),
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
        lines.push(Line::default());
        lines.extend(
            json_lines(&Value::Object(extra))
                .into_iter()
                .map(ratatui::prelude::Stylize::dim),
        );
    }
    lines
}

fn json_lines(value: &Value) -> Vec<Line<'static>> {
    serde_json::to_string_pretty(value)
        .unwrap_or_default()
        .lines()
        .map(|l| Line::from(l.to_string()))
        .collect()
}
