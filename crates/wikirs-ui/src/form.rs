//! The palette's form model (docs/spec/interfaces.md#gui-and-tui).
//!
//! A [`Form`] is built from an Operation's Input schema: one [`Field`] per
//! property, each with a [`Control`] holding what the user has entered.
//! [`Form::input`] turns the entries back into the JSON Input, or says which
//! fields are wrong. Mutations' `dry_run` becomes [`Form::dry_run`], a toggle
//! rather than a field. There's no UI code here: each UI draws the model its own way.

use std::fmt;

use serde_json::{Map, Value};
use wikirs_core::{Kind, OpInfo};

/// The palette form for one Operation.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub op: &'static str,
    pub description: &'static str,
    pub kind: Kind,
    /// Required fields first, then the rest, each in declaration order.
    pub fields: Vec<Field>,
    /// `Some` for Operations that take `dry_run`: whether to only show the Plan.
    pub dry_run: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// The property name in the Input, e.g. `base_version`.
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
    pub control: Control,
}

/// What the user edits, and what they've entered so far.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    /// A string. Left empty, an optional one is omitted.
    Text(String),
    /// A whole number, as typed.
    Integer { text: String, min: Option<i64> },
    /// Sent only when it differs from the schema's default.
    Bool { on: bool, default: bool },
    /// One of `choices`, or none yet.
    Choice {
        choices: Vec<Choice>,
        selected: Option<usize>,
    },
    /// A list of scalars, one entry per item.
    List { item: Item, items: Vec<String> },
    /// A list of objects, e.g. `edit_page`'s `edits`. `template` is a blank record.
    Records {
        template: Vec<Field>,
        records: Vec<Vec<Field>>,
    },
    /// A nested object, e.g. a `filter` or `scope`.
    Group(Vec<Field>),
    /// Any JSON value, as typed. Text that isn't JSON is taken as a string, as on
    /// the CLI. With `object`, it must be a JSON object.
    Json { text: String, object: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub value: String,
    pub description: Option<String>,
}

/// What each entry of a [`Control::List`] holds.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Text,
    Integer,
    Choice(Vec<Choice>),
}

/// A schema shape the form model can't represent. The parity test requires
/// there to be none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    pub op: String,
    pub field: String,
    pub schema: String,
}

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}`: field `{}` has an unsupported schema: {}",
            self.op, self.field, self.schema
        )
    }
}

impl std::error::Error for Unsupported {}

/// A field whose entry can't become Input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// Where it is, e.g. `limit`, `filter.tag` or `edits[0].old`.
    pub path: String,
    pub message: String,
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// One form per Operation in the registry, in catalogue order.
pub fn forms() -> Result<Vec<Form>, Unsupported> {
    wikirs_core::registry().iter().map(Form::new).collect()
}

impl Form {
    pub fn new(op: &OpInfo) -> Result<Self, Unsupported> {
        let schema = op.input_schema();
        let reader = Reader {
            op: op.name,
            defs: schema.get("$defs").cloned().unwrap_or(Value::Null),
        };
        let mut fields = reader.fields(&schema, "")?;
        let dry_run = fields
            .iter()
            .position(|f| f.name == "dry_run" && matches!(f.control, Control::Bool { .. }))
            .map(|i| match fields.remove(i).control {
                Control::Bool { on, .. } => on,
                _ => unreachable!("matched above"),
            });
        Ok(Form {
            op: op.name,
            description: op.description,
            kind: op.kind,
            fields,
            dry_run,
        })
    }

    /// The form for the Operation called `name`.
    #[must_use]
    pub fn for_op(name: &str) -> Option<Result<Self, Unsupported>> {
        wikirs_core::find(name).map(|op| Form::new(&op))
    }

    /// The top-level field called `name`.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    pub fn field_mut(&mut self, name: &str) -> Option<&mut Field> {
        self.fields.iter_mut().find(|f| f.name == name)
    }

    /// Fills entries from (part of) an Input, e.g. `{"page": "eng/rust"}` to
    /// open the form on the current Page. Unknown keys are ignored.
    pub fn fill(&mut self, input: &Value) {
        if let Some(on) = input.get("dry_run").and_then(Value::as_bool)
            && self.dry_run.is_some()
        {
            self.dry_run = Some(on);
        }
        fill_fields(&mut self.fields, input);
    }

    /// The Input the entries make, or every field that's wrong.
    pub fn input(&self) -> Result<Value, Vec<FieldError>> {
        let mut errors = Vec::new();
        let mut input = object_of(&self.fields, "", &mut errors);
        if let Some(on) = self.dry_run {
            input.insert("dry_run".into(), Value::Bool(on));
        }
        if errors.is_empty() {
            Ok(Value::Object(input))
        } else {
            Err(errors)
        }
    }
}

impl Field {
    /// The name as a label: `base_version` → `base version`.
    #[must_use]
    pub fn label(&self) -> String {
        self.name.replace('_', " ")
    }
}

impl Control {
    /// Adds a blank record to a [`Control::Records`], or an empty item to a
    /// [`Control::List`]. Does nothing for other controls.
    pub fn push(&mut self) {
        match self {
            Control::Records { template, records } => records.push(template.clone()),
            Control::List { items, .. } => items.push(String::new()),
            _ => {}
        }
    }
}

// ---------------------------------------------------------- schema → fields

struct Reader {
    op: &'static str,
    defs: Value,
}

impl Reader {
    fn fields(&self, schema: &Value, prefix: &str) -> Result<Vec<Field>, Unsupported> {
        let required: Vec<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut fields = Vec::new();
        for (name, prop) in schema
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let path = join(prefix, name);
            let description = prop
                .get("description")
                .or_else(|| self.resolve(prop).get("description"))
                .and_then(Value::as_str)
                .map(str::to_string);
            fields.push(Field {
                name: name.clone(),
                description,
                required: required.contains(&name.as_str()),
                control: self.control(prop, &path)?,
            });
        }
        // Both keep declaration order (serde_json's `preserve_order`).
        fields.sort_by_key(|f| {
            required
                .iter()
                .position(|r| *r == f.name)
                .unwrap_or(usize::MAX)
        });
        Ok(fields)
    }

    fn control(&self, prop: &Value, path: &str) -> Result<Control, Unsupported> {
        let schema = self.resolve(prop);
        if let Some(choices) = self.choices(schema) {
            return Ok(Control::Choice {
                choices,
                selected: None,
            });
        }
        let control = match base_type(schema) {
            None if schema.as_object().is_some_and(is_any) => Control::Json {
                text: String::new(),
                object: false,
            },
            Some("string") => Control::Text(String::new()),
            Some("integer") => Control::Integer {
                text: schema
                    .get("default")
                    .and_then(Value::as_i64)
                    .map(|d| d.to_string())
                    .unwrap_or_default(),
                min: schema.get("minimum").and_then(Value::as_i64),
            },
            Some("boolean") => {
                let default = schema
                    .get("default")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Control::Bool {
                    on: default,
                    default,
                }
            }
            Some("object") if schema.get("properties").is_some() => {
                Control::Group(self.fields(schema, path)?)
            }
            Some("object") if schema.get("additionalProperties") == Some(&Value::Bool(true)) => {
                Control::Json {
                    text: String::new(),
                    object: true,
                }
            }
            Some("array") => {
                let items = self.resolve(schema.get("items").unwrap_or(&Value::Null));
                if base_type(items) == Some("object") && items.get("properties").is_some() {
                    Control::Records {
                        template: self.fields(items, &format!("{path}[]"))?,
                        records: Vec::new(),
                    }
                } else {
                    let item = match (self.choices(items), base_type(items)) {
                        (Some(choices), _) => Item::Choice(choices),
                        (None, Some("string")) => Item::Text,
                        (None, Some("integer")) => Item::Integer,
                        _ => return Err(self.unsupported(path, items)),
                    };
                    Control::List {
                        item,
                        items: Vec::new(),
                    }
                }
            }
            _ => return Err(self.unsupported(path, schema)),
        };
        Ok(control)
    }

    /// A string `enum`, or a `oneOf` of string `const`s (with descriptions).
    fn choices(&self, schema: &Value) -> Option<Vec<Choice>> {
        if let Some(values) = schema.get("enum").and_then(Value::as_array) {
            return values
                .iter()
                .map(|v| {
                    v.as_str().map(|s| Choice {
                        value: s.to_string(),
                        description: None,
                    })
                })
                .collect();
        }
        schema
            .get("oneOf")
            .and_then(Value::as_array)?
            .iter()
            .map(|alt| {
                let alt = self.resolve(alt);
                Some(Choice {
                    value: alt.get("const")?.as_str()?.to_string(),
                    description: alt
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            })
            .collect()
    }

    /// Follows a `$ref` into `$defs`.
    fn resolve<'a>(&'a self, schema: &'a Value) -> &'a Value {
        match schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| r.strip_prefix("#/$defs/"))
        {
            Some(name) => self.defs.get(name).unwrap_or(&Value::Null),
            None => schema,
        }
    }

    fn unsupported(&self, path: &str, schema: &Value) -> Unsupported {
        Unsupported {
            op: self.op.to_string(),
            field: path.to_string(),
            schema: schema.to_string(),
        }
    }
}

/// `"string"`, or `["string", "null"]` (optional either way).
fn base_type(schema: &Value) -> Option<&str> {
    match schema.get("type")? {
        Value::String(t) => Some(t),
        Value::Array(types) => {
            let mut types = types
                .iter()
                .filter_map(Value::as_str)
                .filter(|t| *t != "null");
            let t = types.next()?;
            types.next().is_none().then_some(t)
        }
        _ => None,
    }
}

/// A schema that accepts any value: nothing but annotations.
fn is_any(schema: &Map<String, Value>) -> bool {
    schema
        .keys()
        .all(|k| matches!(k.as_str(), "description" | "title" | "default"))
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}.{name}")
    }
}

// ----------------------------------------------------------- fields → input

fn object_of(fields: &[Field], prefix: &str, errors: &mut Vec<FieldError>) -> Map<String, Value> {
    let mut out = Map::new();
    for field in fields {
        let path = join(prefix, &field.name);
        match value_of(field, &path) {
            Ok(Some(value)) => {
                out.insert(field.name.clone(), value);
            }
            Ok(None) if field.required => errors.push(FieldError {
                path,
                message: "required".into(),
            }),
            Ok(None) => {}
            Err(mut errs) => errors.append(&mut errs),
        }
    }
    out
}

/// The field's value, `None` if left empty.
fn value_of(field: &Field, path: &str) -> Result<Option<Value>, Vec<FieldError>> {
    let error = |message: String| {
        vec![FieldError {
            path: path.to_string(),
            message,
        }]
    };
    let value = match &field.control {
        // A required string may be empty: `edit_page` replaces with "".
        Control::Text(text) if text.is_empty() && !field.required => None,
        Control::Text(text) => Some(Value::String(text.clone())),
        Control::Integer { text, min } => match text.trim() {
            "" => None,
            text => Some(Value::from(integer(text, *min).map_err(error)?)),
        },
        Control::Bool { on, default } => {
            (on != default || field.required).then_some(Value::Bool(*on))
        }
        Control::Choice { choices, selected } => {
            selected.map(|i| Value::String(choices[i].value.clone()))
        }
        Control::List { item, items } => {
            let values = items
                .iter()
                .filter(|text| !text.trim().is_empty())
                .map(|text| list_item(item, text.trim()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(error)?;
            (!values.is_empty() || field.required).then_some(Value::Array(values))
        }
        Control::Records { records, .. } => {
            let mut errors = Vec::new();
            let values: Vec<Value> = records
                .iter()
                .enumerate()
                .map(|(i, record)| {
                    Value::Object(object_of(record, &format!("{path}[{i}]"), &mut errors))
                })
                .collect();
            if !errors.is_empty() {
                return Err(errors);
            }
            (!values.is_empty() || field.required).then_some(Value::Array(values))
        }
        Control::Group(fields) => {
            let mut errors = Vec::new();
            let object = object_of(fields, path, &mut errors);
            if !errors.is_empty() {
                return Err(errors);
            }
            (!object.is_empty() || field.required).then_some(Value::Object(object))
        }
        Control::Json { text, object } => match text.trim() {
            "" => None,
            text => {
                let value: Value = serde_json::from_str(text).unwrap_or_else(|_| text.into());
                if *object && !value.is_object() {
                    return Err(error("expected a JSON object, e.g. {\"key\": 1}".into()));
                }
                Some(value)
            }
        },
    };
    Ok(value)
}

fn integer(text: &str, min: Option<i64>) -> Result<i64, String> {
    let n: i64 = text
        .parse()
        .map_err(|_| format!("`{text}` isn't a whole number"))?;
    match min {
        Some(min) if n < min => Err(format!("must be at least {min}")),
        _ => Ok(n),
    }
}

fn list_item(item: &Item, text: &str) -> Result<Value, String> {
    match item {
        Item::Text => Ok(Value::String(text.to_string())),
        Item::Integer => integer(text, None).map(Value::from),
        Item::Choice(choices) => {
            if choices.iter().any(|c| c.value == text) {
                Ok(Value::String(text.to_string()))
            } else {
                let names: Vec<_> = choices.iter().map(|c| c.value.as_str()).collect();
                Err(format!("`{text}` isn't one of {}", names.join(", ")))
            }
        }
    }
}

// ------------------------------------------------------------ input → fields

fn fill_fields(fields: &mut [Field], input: &Value) {
    for field in fields {
        if let Some(value) = input.get(&field.name) {
            fill_control(&mut field.control, value);
        }
    }
}

fn fill_control(control: &mut Control, value: &Value) {
    match control {
        Control::Text(text) => {
            if let Some(s) = value.as_str() {
                *text = s.to_string();
            }
        }
        Control::Integer { text, .. } => {
            if let Some(n) = value.as_i64() {
                *text = n.to_string();
            }
        }
        Control::Bool { on, .. } => {
            if let Some(b) = value.as_bool() {
                *on = b;
            }
        }
        Control::Choice { choices, selected } => {
            if let Some(s) = value.as_str() {
                *selected = choices.iter().position(|c| c.value == s);
            }
        }
        Control::List { items, .. } => {
            if let Some(values) = value.as_array() {
                *items = values.iter().map(scalar_text).collect();
            }
        }
        Control::Records { template, records } => {
            if let Some(values) = value.as_array() {
                *records = values
                    .iter()
                    .map(|v| {
                        let mut record = template.clone();
                        fill_fields(&mut record, v);
                        record
                    })
                    .collect();
            }
        }
        Control::Group(fields) => fill_fields(fields, value),
        Control::Json { text, .. } => {
            // A string that reads as JSON (`"123"`) stays quoted, so it comes back a string.
            *text = match value {
                Value::String(s) if serde_json::from_str::<Value>(s).is_err() => s.clone(),
                v => v.to_string(),
            };
        }
    }
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}
