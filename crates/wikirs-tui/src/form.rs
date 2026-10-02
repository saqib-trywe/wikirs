//! The palette's form (interfaces.md#gui-and-tui): a `wikirs-forms` Form drawn
//! as one row per editable value. Groups and records get a header row with
//! their fields indented under it; a list is one comma-separated row.

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::Stylize,
    text::{Line, Span},
};
use serde_json::Value;
use wikirs_forms::{Control, Field, FieldError, Form};

/// A form being filled in.
pub struct FormView {
    pub form: Form,
    pub selected: usize,
    pub errors: Vec<FieldError>,
}

/// What a key did to the form.
pub enum FormAction {
    None,
    /// Back to the Operation list.
    Close,
    /// Run with this Input.
    Submit(Value),
}

#[derive(Clone, Copy)]
enum Step {
    Field(usize),
    Record(usize),
}

struct Row {
    path: Vec<Step>,
    label: String,
    error_path: String,
    depth: usize,
}

impl FormView {
    #[must_use]
    pub fn new(form: Form) -> Self {
        Self {
            form,
            selected: 0,
            errors: Vec::new(),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> FormAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let rows = rows(&self.form.fields);
        let path = rows.get(self.selected).map(|r| r.path.clone());
        match key.code {
            KeyCode::Esc => return FormAction::Close,
            KeyCode::Enter => match self.form.input() {
                Ok(input) => return FormAction::Submit(input),
                Err(errors) => self.errors = errors,
            },
            KeyCode::Char('d') if ctrl => {
                if let Some(on) = self.form.dry_run.as_mut() {
                    *on = !*on;
                }
            }
            KeyCode::Char('n') if ctrl => {
                // Adds a record to the Records the cursor is in.
                if let Some(path) = path {
                    let records = path
                        .iter()
                        .position(|s| matches!(s, Step::Record(_)))
                        .map_or(path.as_slice(), |i| &path[..i]);
                    if let Some(control) = control_mut(&mut self.form.fields, records) {
                        control.push();
                    }
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                self.selected = (self.selected + 1).min(rows.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::BackTab => self.selected = self.selected.saturating_sub(1),
            code => {
                if let Some(control) = path.and_then(|p| control_mut(&mut self.form.fields, &p)) {
                    edit(control, code);
                }
            }
        }
        FormAction::None
    }

    /// The form as lines: header, one row per value, the dry-run toggle, errors.
    #[must_use]
    pub fn lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![
            Line::from(vec![
                Span::raw(self.form.op).bold(),
                Span::raw(format!("  {:?}", self.form.kind).to_lowercase()).dim(),
            ]),
            Line::from(self.form.description).dim(),
            Line::default(),
        ];
        let rows = rows(&self.form.fields);
        let width = rows
            .iter()
            .map(|r| r.label.len() + 2 * r.depth)
            .max()
            .unwrap_or(0)
            .max(7);
        for (i, row) in rows.iter().enumerate() {
            let Some((field, control)) = field_at(&self.form.fields, &row.path) else {
                continue;
            };
            let active = i == self.selected;
            let label = format!(
                "{}{}{}",
                "  ".repeat(row.depth),
                row.label,
                if field.required { "*" } else { "" }
            );
            let mut value = display(control);
            if active && editable(control) {
                value.push('█');
            }
            let error = self.errors.iter().find(|e| e.path == row.error_path);
            let mut spans = vec![
                Span::raw(format!("{label:>width$}: ", width = width + 1)).dim(),
                Span::raw(value),
            ];
            if let Some(error) = error {
                spans.push(Span::raw(format!("  ✗ {}", error.message)).red());
            }
            let line = Line::from(spans);
            lines.push(if active { line.bold().reversed() } else { line });
        }
        if let Some(on) = self.form.dry_run {
            lines.push(Line::from(format!(
                "{:>width$}  [{}] (ctrl-d)",
                "dry run",
                if on { "x" } else { " " },
                width = width + 1
            )));
        }
        for error in self
            .errors
            .iter()
            .filter(|e| !rows.iter().any(|r| r.error_path == e.path))
        {
            lines.push(Line::from(format!("✗ {error}")).red());
        }
        lines.push(Line::default());
        lines.push(
            Line::from(
                "Tab/↑↓ field · Space/←→ toggle or choose · ctrl-n add item · Enter run · Esc back",
            )
            .dim(),
        );
        lines
    }

    /// How many rows the form draws (for sizing its popup).
    #[must_use]
    pub fn height(&self) -> usize {
        self.lines().len()
    }
}

fn rows(fields: &[Field]) -> Vec<Row> {
    let mut out = Vec::new();
    collect_rows(fields, &[], "", 0, &mut out);
    out
}

fn collect_rows(fields: &[Field], path: &[Step], prefix: &str, depth: usize, out: &mut Vec<Row>) {
    for (i, field) in fields.iter().enumerate() {
        let mut here = path.to_vec();
        here.push(Step::Field(i));
        let error_path = if prefix.is_empty() {
            field.name.clone()
        } else {
            format!("{prefix}.{}", field.name)
        };
        out.push(Row {
            path: here.clone(),
            label: field.label(),
            error_path: error_path.clone(),
            depth,
        });
        match &field.control {
            Control::Group(inner) => collect_rows(inner, &here, &error_path, depth + 1, out),
            Control::Records { records, .. } => {
                for (r, record) in records.iter().enumerate() {
                    let mut at = here.clone();
                    at.push(Step::Record(r));
                    collect_rows(record, &at, &format!("{error_path}[{r}]"), depth + 1, out);
                }
            }
            _ => {}
        }
    }
}

fn field_at<'a>(fields: &'a [Field], path: &[Step]) -> Option<(&'a Field, &'a Control)> {
    let (Step::Field(i), rest) = path.split_first()? else {
        return None;
    };
    let field = fields.get(*i)?;
    match rest {
        [] => Some((field, &field.control)),
        [Step::Record(r), rest @ ..] => match &field.control {
            Control::Records { records, .. } => field_at(records.get(*r)?, rest),
            _ => None,
        },
        rest => match &field.control {
            Control::Group(inner) => field_at(inner, rest),
            _ => None,
        },
    }
}

fn control_mut<'a>(fields: &'a mut [Field], path: &[Step]) -> Option<&'a mut Control> {
    let (Step::Field(i), rest) = path.split_first()? else {
        return None;
    };
    let field = fields.get_mut(*i)?;
    match rest {
        [] => Some(&mut field.control),
        [Step::Record(r), rest @ ..] => match &mut field.control {
            Control::Records { records, .. } => control_mut(records.get_mut(*r)?, rest),
            _ => None,
        },
        rest => match &mut field.control {
            Control::Group(inner) => control_mut(inner, rest),
            _ => None,
        },
    }
}

fn editable(control: &Control) -> bool {
    matches!(
        control,
        Control::Text(_) | Control::Integer { .. } | Control::Json { .. } | Control::List { .. }
    )
}

fn display(control: &Control) -> String {
    match control {
        Control::Text(text) | Control::Integer { text, .. } | Control::Json { text, .. } => {
            text.clone()
        }
        Control::Bool { on, .. } => if *on { "[x]" } else { "[ ]" }.to_string(),
        Control::Choice { choices, selected } => match selected {
            Some(i) => format!("‹ {} ›", choices[*i].value),
            None => format!(
                "‹ none › ({})",
                choices
                    .iter()
                    .map(|c| c.value.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
        },
        Control::List { items, .. } => items.join(","),
        Control::Group(_) => String::new(),
        Control::Records { records, .. } => format!("{} (ctrl-n adds)", records.len()),
    }
}

fn edit(control: &mut Control, code: KeyCode) {
    match control {
        Control::Text(text) | Control::Integer { text, .. } | Control::Json { text, .. } => {
            match code {
                KeyCode::Char(c) => text.push(c),
                KeyCode::Backspace => {
                    text.pop();
                }
                _ => {}
            }
        }
        Control::List { items, .. } => {
            let mut text = items.join(",");
            match code {
                KeyCode::Char(c) => text.push(c),
                KeyCode::Backspace => {
                    text.pop();
                }
                _ => return,
            }
            *items = if text.is_empty() {
                Vec::new()
            } else {
                text.split(',').map(str::to_string).collect()
            };
        }
        Control::Bool { on, .. } => {
            if matches!(code, KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right) {
                *on = !*on;
            }
        }
        Control::Choice { choices, selected } => {
            let n = choices.len();
            *selected = match (code, *selected) {
                (KeyCode::Right | KeyCode::Char(' '), None) => Some(0),
                (KeyCode::Right | KeyCode::Char(' '), Some(i)) => Some((i + 1) % n),
                (KeyCode::Left, None) => Some(n - 1),
                (KeyCode::Left, Some(i)) => Some((i + n - 1) % n),
                (KeyCode::Backspace, _) => None,
                (_, current) => current,
            };
        }
        Control::Group(_) | Control::Records { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn view(op: &str) -> FormView {
        FormView::new(Form::for_op(op).unwrap().unwrap())
    }

    fn press(view: &mut FormView, keys: &[KeyEvent]) -> Option<Value> {
        let mut submitted = None;
        for key in keys {
            if let FormAction::Submit(input) = view.key(*key) {
                submitted = Some(input);
            }
        }
        submitted
    }

    fn chars(text: &str) -> Vec<KeyEvent> {
        text.chars()
            .map(|c| KeyEvent::from(KeyCode::Char(c)))
            .collect()
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn records_lists_choices_and_toggles() {
        // edit_page: page*, edits* (records), base_version.
        let mut form = view("edit_page");
        let down = KeyEvent::from(KeyCode::Down);
        let up = KeyEvent::from(KeyCode::Up);
        let mut keys = chars("a");
        keys.extend([down, ctrl('n'), down]);
        keys.extend(chars("x"));
        // ctrl-n inside a record adds a sibling record, not a nested one.
        // ctrl-d three times: off → on → off → on.
        keys.extend([ctrl('n'), up, up, ctrl('d'), ctrl('d'), ctrl('d')]);
        keys.extend(chars("p"));
        keys.push(KeyEvent::from(KeyCode::Enter));
        // Rows: page, edits, edits[0].old, edits[0].new, edits[1].old, … so two
        // Ups from edits[0].old land on `page`.
        assert_eq!(
            press(&mut form, &keys).unwrap(),
            json!({
                "page": "ap",
                "edits": [{"old": "x", "new": ""}, {"old": "", "new": ""}],
                "dry_run": true
            })
        );
    }

    #[test]
    fn choices_bools_and_lists() {
        // check: kinds (list of choices), scope (group).
        let mut form = view("check");
        let input = press(
            &mut form,
            &[
                chars("broken_link,nope"),
                vec![KeyEvent::from(KeyCode::Enter)],
            ]
            .concat(),
        );
        assert!(input.is_none());
        assert_eq!(form.errors[0].path, "kinds");
        let lines: Vec<String> = form.lines().iter().map(ToString::to_string).collect();
        assert!(
            lines.iter().any(|l| l.contains("✗ `nope` isn't one of")),
            "{lines:?}"
        );
        let mut keys = vec![KeyEvent::from(KeyCode::Backspace); 5];
        keys.push(KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            press(&mut form, &keys).unwrap(),
            json!({"kinds": ["broken_link"]})
        );

        // set_config: key*, value*, scope* (a choice).
        let mut form = view("set_config");
        let down = KeyEvent::from(KeyCode::Down);
        let mut keys = chars("k");
        keys.extend([down]);
        keys.extend(chars("1"));
        keys.extend([
            down,
            KeyEvent::from(KeyCode::Right),
            KeyEvent::from(KeyCode::Right),
        ]);
        keys.extend([KeyEvent::from(KeyCode::Left), KeyEvent::from(KeyCode::Left)]);
        keys.push(KeyEvent::from(KeyCode::Enter));
        // From none: Right → wiki → machine, Left → wiki → machine (wrapping).
        assert_eq!(
            press(&mut form, &keys).unwrap(),
            json!({"key": "k", "value": 1, "scope": "machine", "dry_run": false})
        );

        // delete_page: page*, recursive (a bool).
        let mut form = view("delete_page");
        let mut keys = chars("a");
        keys.extend([
            KeyEvent::from(KeyCode::Down),
            KeyEvent::from(KeyCode::Char(' ')),
        ]);
        keys.push(KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            press(&mut form, &keys).unwrap(),
            json!({"page": "a", "recursive": true, "dry_run": false})
        );
        assert_eq!(press(&mut form, &[KeyEvent::from(KeyCode::Esc)]), None);
    }
}
