//! Settings (wiki-selection.md#machine-settings): a closed registry of keys,
//! read from the Wiki's committed `.wikirs/config.toml` (scope `wiki`) and
//! from this machine's file for the Wiki, outside it (scope `machine`).
//! Files are edited through `toml_edit`, so comments and layout survive and a
//! change is one small splice.

use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use toml_edit::{DocumentMut, Item, Table};

use crate::plan::{Splice, Warning};

/// The Wiki's own settings file, relative to its root.
pub const WIKI_FILE: &str = ".wikirs/config.toml";

/// Which file a setting is written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum ConfigScope {
    /// `.wikirs/config.toml`, committed and shared by every machine.
    Wiki,
    /// This machine's settings for the Wiki, outside it, never synced.
    Machine,
}

/// Where a setting's value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Wiki,
    Machine,
    Default,
}

#[derive(Debug, Clone, Copy)]
enum Type {
    OneOf(&'static [&'static str]),
    Strings,
    Bool,
    Port,
    Bytes,
    /// A string, or unset (`null` default).
    Text,
}

impl Type {
    fn check(self, value: &Value) -> Result<(), String> {
        let ok = match self {
            Type::OneOf(allowed) => value.as_str().is_some_and(|v| allowed.contains(&v)),
            Type::Strings => value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string)),
            Type::Bool => value.is_boolean(),
            Type::Port => value.as_u64().is_some_and(|p| u16::try_from(p).is_ok()),
            Type::Bytes => value.as_u64().is_some_and(|b| b > 0),
            Type::Text => value.is_string(),
        };
        if ok {
            return Ok(());
        }
        Err(match self {
            Type::OneOf(allowed) => format!("one of {}", allowed.join(", ")),
            Type::Strings => "a list of strings".into(),
            Type::Bool => "true or false".into(),
            Type::Port => "a port number (0–65535)".into(),
            Type::Bytes => "a positive number of bytes".into(),
            Type::Text => "a string".into(),
        })
    }
}

struct Key {
    name: &'static str,
    /// `Machine` keys can't go in the committed file; `Wiki` keys can also be
    /// set per machine, which overrides the Wiki's value.
    home: ConfigScope,
    ty: Type,
    /// The default, as JSON.
    default: &'static str,
    description: &'static str,
}

const KEYS: &[Key] = &[
    Key {
        name: "links.syntax",
        home: ConfigScope::Wiki,
        ty: Type::OneOf(&["standard", "wikilink"]),
        default: r#""standard""#,
        description: "Link syntax the app writes: `standard` ([text](path.md)) or `wikilink` ([[path]]).",
    },
    Key {
        name: "ignore",
        home: ConfigScope::Wiki,
        ty: Type::Strings,
        default: "[]",
        description: "Globs of files and folders the Index skips. A pattern without `/` matches a name at any depth.",
    },
    Key {
        name: "search.stemming",
        home: ConfigScope::Wiki,
        ty: Type::OneOf(&["none", "english"]),
        default: r#""none""#,
        description: "Search stemming: `none`, or `english` (Porter). Changing it rebuilds the Index.",
    },
    Key {
        name: "watcher",
        home: ConfigScope::Machine,
        ty: Type::OneOf(&["native", "poll"]),
        default: r#""native""#,
        description: "How long-lived processes watch the files: `native`, or `poll` (every 2 s, for network drives).",
    },
    Key {
        name: "cache_dir",
        home: ConfigScope::Machine,
        ty: Type::Text,
        default: "null",
        description: "Where this Wiki's Index, lock and journal live (default: the OS cache dir). Takes effect when the Wiki is next opened.",
    },
    Key {
        name: "serve.bind",
        home: ConfigScope::Machine,
        ty: Type::Text,
        default: "null",
        description: "Address `serve` binds to (default: loopback). A non-loopback address also needs `--allow-remote`.",
    },
    Key {
        name: "serve.port",
        home: ConfigScope::Machine,
        ty: Type::Port,
        default: "4747",
        description: "Port `serve` listens on; 0 picks a free one.",
    },
    Key {
        name: "serve.require_token",
        home: ConfigScope::Machine,
        ty: Type::Bool,
        default: "false",
        description: "Require the bearer token on loopback too.",
    },
    Key {
        name: "serve.allowed_hosts",
        home: ConfigScope::Machine,
        ty: Type::Strings,
        default: "[]",
        description: "Extra `Host` names `serve` accepts besides localhost.",
    },
    Key {
        name: "serve.cors_origins",
        home: ConfigScope::Machine,
        ty: Type::Strings,
        default: "[]",
        description: "Browser origins allowed to call `serve`.",
    },
    Key {
        name: "serve.read_only",
        home: ConfigScope::Machine,
        ty: Type::Bool,
        default: "false",
        description: "Serve queries only.",
    },
    Key {
        name: "serve.max_body",
        home: ConfigScope::Machine,
        ty: Type::Bytes,
        default: "33554432",
        description: "Largest request body `serve` accepts, in bytes.",
    },
];

fn key(name: &str) -> Option<&'static Key> {
    KEYS.iter().find(|k| k.name == name)
}

/// One setting as `get_config` reports it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Setting {
    pub key: String,
    pub value: Value,
    pub source: Source,
    /// The file it belongs in; `wiki` settings can also be overridden per machine.
    pub scope: ConfigScope,
    pub description: String,
}

/// Both settings files of one Wiki, as read now.
#[derive(Debug, Default)]
pub struct Settings {
    wiki: Option<DocumentMut>,
    machine: Option<DocumentMut>,
    /// Unreadable files and invalid values, which fall back to the default.
    pub warnings: Vec<Warning>,
}

fn read_doc(path: &Path, label: &str, warnings: &mut Vec<Warning>) -> Option<DocumentMut> {
    let text = std::fs::read_to_string(path).ok()?;
    match text.parse::<DocumentMut>() {
        Ok(doc) => Some(doc),
        Err(e) => {
            warnings.push(bad_setting(format!(
                "{label} ({}) isn't valid TOML, so it's ignored: {e}",
                path.display()
            )));
            None
        }
    }
}

fn bad_setting(message: String) -> Warning {
    Warning {
        kind: "bad_setting".into(),
        message,
    }
}

/// The item at a dotted key, if present.
fn lookup<'d>(doc: &'d DocumentMut, name: &str) -> Option<&'d Item> {
    let mut item = doc.as_item();
    for segment in name.split('.') {
        item = item.as_table_like()?.get(segment)?;
    }
    Some(item)
}

fn to_json(item: &Item) -> Option<Value> {
    match item {
        Item::Value(v) => Some(value_to_json(v)),
        Item::Table(t) => Some(Value::Object(
            t.iter()
                .filter_map(|(k, v)| Some((k.to_string(), to_json(v)?)))
                .collect(),
        )),
        _ => None,
    }
}

fn value_to_json(v: &toml_edit::Value) -> Value {
    use toml_edit::Value as T;
    match v {
        T::String(s) => Value::from(s.value().as_str()),
        T::Integer(i) => Value::from(*i.value()),
        T::Float(f) => Value::from(*f.value()),
        T::Boolean(b) => Value::from(*b.value()),
        T::Array(items) => Value::Array(items.iter().map(value_to_json).collect()),
        T::InlineTable(t) => Value::Object(
            t.iter()
                .map(|(k, v)| (k.to_string(), value_to_json(v)))
                .collect(),
        ),
        T::Datetime(d) => Value::from(d.value().to_string()),
    }
}

fn to_toml(value: &Value) -> toml_edit::Value {
    match value {
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => n
            .as_i64()
            .map_or_else(|| n.as_f64().unwrap_or_default().into(), Into::into),
        Value::String(s) => s.as_str().into(),
        Value::Array(items) => items
            .iter()
            .map(to_toml)
            .collect::<toml_edit::Array>()
            .into(),
        // Only strings, numbers, bools and lists pass `Type::check`.
        Value::Null | Value::Object(_) => "".into(),
    }
}

impl Settings {
    /// Reads the Wiki's file under `root` and this machine's `machine_file`.
    #[must_use]
    pub fn load(root: &Path, machine_file: &Path) -> Self {
        let mut warnings = Vec::new();
        let wiki = read_doc(&root.join(WIKI_FILE), WIKI_FILE, &mut warnings);
        let machine = read_doc(machine_file, "the machine settings", &mut warnings);
        let mut settings = Self {
            wiki,
            machine,
            warnings,
        };
        let mut problems: Vec<String> = KEYS.iter().flat_map(|k| settings.resolve(k).2).collect();
        problems.extend(Ignore::new(&settings.ignore_patterns()).1);
        settings
            .warnings
            .extend(problems.into_iter().map(bad_setting));
        settings
    }

    /// A key's effective value, where it came from, and any invalid values
    /// passed over: this machine's value, then the Wiki's (for `wiki` keys),
    /// then the default.
    fn resolve(&self, k: &Key) -> (Value, Source, Vec<String>) {
        let files = [
            (self.machine.as_ref(), Source::Machine),
            (
                self.wiki.as_ref().filter(|_| k.home == ConfigScope::Wiki),
                Source::Wiki,
            ),
        ];
        let mut problems = Vec::new();
        for (doc, source) in files {
            let Some(item) = doc.and_then(|d| lookup(d, k.name)) else {
                continue;
            };
            let checked = to_json(item)
                .ok_or_else(|| "a plain value".to_string())
                .and_then(|v| k.ty.check(&v).map(|()| v));
            match checked {
                Ok(v) => return (v, source, problems),
                Err(want) => problems.push(format!(
                    "`{}` in the {} settings should be {want}, so it's ignored",
                    k.name,
                    if source == Source::Machine {
                        "machine"
                    } else {
                        "Wiki"
                    }
                )),
            }
        }
        let default = serde_json::from_str(k.default).expect("defaults are valid JSON");
        (default, Source::Default, problems)
    }

    /// Every setting, or just `name`; `None` if `name` isn't a setting.
    #[must_use]
    pub fn report(&self, name: Option<&str>) -> Option<Vec<Setting>> {
        let keys: Vec<&Key> = match name {
            Some(n) => vec![key(n)?],
            None => KEYS.iter().collect(),
        };
        Some(
            keys.into_iter()
                .map(|k| {
                    let (value, source, _) = self.resolve(k);
                    Setting {
                        key: k.name.to_string(),
                        value,
                        source,
                        scope: k.home,
                        description: k.description.to_string(),
                    }
                })
                .collect(),
        )
    }

    fn get(&self, name: &str) -> Value {
        self.resolve(key(name).expect("a registered key")).0
    }

    /// The `ignore` globs, compiled. Invalid ones are dropped (and warned about).
    #[must_use]
    pub fn ignore(&self) -> Ignore {
        Ignore::new(&self.ignore_patterns()).0
    }

    fn ignore_patterns(&self) -> Vec<String> {
        strings(&self.get("ignore"))
    }

    /// Whether search uses English stemming.
    #[must_use]
    pub fn stemming(&self) -> bool {
        self.get("search.stemming") == "english"
    }

    /// The per-Wiki cache dir override, if set.
    #[must_use]
    pub fn cache_dir(&self) -> Option<PathBuf> {
        self.get("cache_dir").as_str().map(crate::wiki::expand_home)
    }
}

/// Checks a `set_config` request: the key exists (`Err(None)`: not found),
/// may be written to `scope`, and `value` (unless `null`) has its type.
pub fn check(name: &str, scope: ConfigScope, value: &Value) -> Result<(), Option<String>> {
    let k = key(name).ok_or(None)?;
    if k.home == ConfigScope::Machine && scope == ConfigScope::Wiki {
        return Err(Some(format!(
            "`{name}` is a machine setting: it never goes in the committed {WIKI_FILE}"
        )));
    }
    if value.is_null() {
        return Ok(());
    }
    k.ty.check(value)
        .map_err(|want| Some(format!("`{name}` should be {want}")))?;
    if name == "ignore"
        && let Some(bad) = Ignore::new(&strings(value)).1.into_iter().next()
    {
        return Err(Some(bad));
    }
    Ok(())
}

/// `text` (a TOML file, empty if new) with dotted key `name` set to `value`,
/// or removed for `null`; a table left empty goes too.
pub fn edit(text: &str, name: &str, value: &Value) -> Result<String, String> {
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| format!("the file isn't valid TOML (fix it by hand): {e}"))?;
    if !value.is_null() && doc.as_table().is_empty() && !text.trim().is_empty() {
        // Only comments so far: toml_edit keeps them as trailing text, which
        // new items would be written above.
        let body = edit("", name, value)?;
        let newline = if text.ends_with('\n') { "" } else { "\n" };
        let gap = if body.starts_with('[') { "\n" } else { "" };
        return Ok(format!("{text}{newline}{gap}{body}"));
    }
    let segments: Vec<&str> = name.split('.').collect();
    let (last, parents) = segments.split_last().ok_or("the key is empty")?;
    if value.is_null() {
        let mut kept_comments = String::new();
        remove(doc.as_table_mut(), parents, last, &mut kept_comments);
        if !kept_comments.is_empty() {
            let trailing = doc.trailing().as_str().unwrap_or_default().to_string();
            doc.set_trailing(format!("{kept_comments}{trailing}"));
        }
    } else {
        let mut table = doc.as_table_mut();
        for p in parents {
            let item = table.entry(p).or_insert_with(|| Item::Table(Table::new()));
            table = item
                .as_table_mut()
                .ok_or_else(|| format!("`{p}` isn't a table in the file"))?;
        }
        let mut new = to_toml(value);
        match table.get_mut(last) {
            // Keep the key as written and the comments around the value.
            Some(Item::Value(old)) => {
                *new.decor_mut() = old.decor().clone();
                *old = new;
            }
            _ => {
                table.insert(last, Item::Value(new));
            }
        }
    }
    Ok(doc.to_string())
}

/// Removes `parents.last`; returns whether `table` is now empty. Comments
/// written above a table that goes are kept in `kept_comments`.
fn remove(table: &mut Table, parents: &[&str], last: &str, kept_comments: &mut String) -> bool {
    match parents.split_first() {
        None => {
            table.remove(last);
        }
        Some((first, rest)) => {
            let Some(child) = table.get_mut(first).and_then(Item::as_table_mut) else {
                return table.is_empty();
            };
            if remove(child, rest, last, kept_comments) {
                let prefix = child.decor().prefix().and_then(|p| p.as_str());
                if let Some(comments) = prefix.filter(|p| p.contains('#')) {
                    kept_comments.push_str(comments);
                }
                table.remove(first);
            }
        }
    }
    table.is_empty()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str().map(str::to_string))
        .collect()
}

/// The smallest single splice turning `old` into `new`; `None` if equal.
#[must_use]
pub fn diff_splice(old: &str, new: &str) -> Option<Splice> {
    if old == new {
        return None;
    }
    let mut start = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(start) || !new.is_char_boundary(start) {
        start -= 1;
    }
    let mut common_end = old[start..]
        .bytes()
        .rev()
        .zip(new[start..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - common_end)
        || !new.is_char_boundary(new.len() - common_end)
    {
        common_end -= 1;
    }
    let (old_end, new_end) = (old.len() - common_end, new.len() - common_end);
    Some(Splice {
        range: [start, old_end],
        old: old[start..old_end].to_string(),
        new: new[start..new_end].to_string(),
    })
}

/// Compiled `ignore` globs, matched against paths relative to the Wiki root.
#[derive(Debug, Clone, Default)]
pub struct Ignore(Option<GlobSet>);

impl Ignore {
    /// The set, plus a message for each pattern that isn't a valid glob.
    #[must_use]
    pub fn new(patterns: &[String]) -> (Self, Vec<String>) {
        let mut builder = GlobSetBuilder::new();
        let mut bad = Vec::new();
        for raw in patterns {
            let p = raw.trim().trim_matches('/');
            let glob = if p.contains('/') {
                p.to_string()
            } else {
                format!("**/{p}")
            };
            match GlobBuilder::new(&glob).literal_separator(true).build() {
                Ok(g) if !p.is_empty() => {
                    builder.add(g);
                }
                Ok(_) => bad.push("an `ignore` pattern is empty".to_string()),
                Err(e) => bad.push(format!("`{raw}` isn't a valid glob: {e}")),
            }
        }
        let set = builder.build().ok().filter(|s| !s.is_empty());
        (Self(set), bad)
    }

    /// Whether the file or folder at `rel` is ignored.
    #[must_use]
    pub fn matches(&self, rel: &str) -> bool {
        self.0.as_ref().is_some_and(|set| set.is_match(rel))
    }

    /// Whether `rel` or any folder above it is ignored.
    #[must_use]
    pub fn matches_path(&self, rel: &str) -> bool {
        rel.match_indices('/')
            .map(|(i, _)| &rel[..i])
            .chain([rel])
            .any(|prefix| self.matches(prefix))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn apply(old: &str, new: &str) -> String {
        match diff_splice(old, new) {
            Some(s) => crate::plan::splice(old, &[s]),
            None => old.to_string(),
        }
    }

    #[test]
    fn the_splice_is_minimal_and_on_char_boundaries() {
        for (old, new) in [
            ("a = 1\n", "a = 2\n"),
            ("x = \"é\"\n", "x = \"è\"\n"),
            ("ab", "aXb"),
            ("abc", "ac"),
            ("", "a = 1\n"),
            ("a = 1\n", ""),
        ] {
            assert_eq!(apply(old, new), new, "{old:?} → {new:?}");
        }
        let s = diff_splice("a = 1\nb = 2\n", "a = 1\nb = 3\n").unwrap();
        assert_eq!(
            (s.range, s.old.as_str(), s.new.as_str()),
            ([10, 11], "2", "3")
        );
        let s = diff_splice("x = \"é\"", "x = \"è\"").unwrap();
        assert_eq!((s.old.as_str(), s.new.as_str()), ("é", "è"));
        assert!(diff_splice("same", "same").is_none());
    }

    #[test]
    fn editing_nested_keys_keeps_the_rest() {
        let text = "# top\n[serve]\nport = 1 # mine\nbind = \"x\"\n";
        assert_eq!(
            edit(text, "serve.port", &json!(2)).unwrap(),
            "# top\n[serve]\nport = 2 # mine\nbind = \"x\"\n"
        );
        assert_eq!(
            edit(text, "serve.bind", &Value::Null).unwrap(),
            "# top\n[serve]\nport = 1 # mine\n"
        );
        let emptied = edit(
            &edit(text, "serve.bind", &Value::Null).unwrap(),
            "serve.port",
            &Value::Null,
        )
        .unwrap();
        assert_eq!(emptied, "# top\n");
        assert_eq!(
            edit("", "ignore", &json!(["a", "b/*"])).unwrap(),
            "ignore = [\"a\", \"b/*\"]\n"
        );
        assert_eq!(
            edit("# only a comment\n", "links.syntax", &json!("x")).unwrap(),
            "# only a comment\n\n[links]\nsyntax = \"x\"\n",
            "a comment-only file keeps its comment on top"
        );
        assert_eq!(
            edit("# c", "ignore", &json!([])).unwrap(),
            "# c\nignore = []\n"
        );
        assert!(edit("[serve", "serve.port", &json!(1)).is_err());
        assert!(edit("serve = 1\n", "serve.port", &json!(1)).is_err());
    }

    #[test]
    fn ignore_globs_match_names_anywhere_and_paths_from_the_root() {
        let (ignore, bad) = Ignore::new(&["drafts".into(), "*.bak".into(), "/eng/old/".into()]);
        assert!(bad.is_empty());
        for yes in ["drafts", "a/drafts", "x.bak", "a/b/x.bak", "eng/old"] {
            assert!(ignore.matches(yes), "{yes}");
        }
        let (anchored, _) = Ignore::new(&["eng/*.md".into()]);
        assert!(anchored.matches("eng/a.md"));
        assert!(!anchored.matches("eng/sub/a.md"), "`*` stops at `/`");
        for no in ["drafts2", "a/drafts/x", "old", "a/eng/old", "x.bak/y"] {
            assert!(!ignore.matches(no), "{no}");
        }
        assert!(!Ignore::default().matches("anything"));
        assert!(ignore.matches_path("a/x.bak"), "the path itself");
        assert!(
            ignore.matches_path("a/drafts/x.md"),
            "under an ignored folder"
        );
        assert!(!ignore.matches_path("a/drafts2/x.md"));
        assert_eq!(Ignore::new(&[" ".into(), "a[".into()]).1.len(), 2);
    }
}
