//! Locating and rewriting a Page's YAML frontmatter by splicing: the `tags:`
//! entries (Tag model, item 6), whose existing list style is kept, and any
//! other top-level key (`set_page_meta`, `order:`). Every other byte of the
//! file stays as it was.

use std::ops::Range;

use serde_json::Value;
use yaml_rust2::{Yaml, YamlEmitter, YamlLoader};

use crate::{markdown::yaml_to_json, plan::Splice};

/// How `tags:` is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    /// `tags: [a, b]`
    Flow,
    /// `tags:` then `  - a` lines
    Block,
    /// `tags: a`
    Scalar,
}

/// One entry: its text as written (quotes included) and the Tag it names.
#[derive(Debug, Clone)]
struct Item {
    raw: String,
    tag: String,
}

/// Where the Tags live in a file.
#[derive(Debug)]
enum Place {
    /// No frontmatter block at all.
    NoFrontmatter,
    /// A frontmatter block without a `tags:` key; new Tags go at `insert_at`.
    NoKey { insert_at: usize },
    /// `tags:` exists. `region` is regenerated as a whole; `key_line` is the
    /// whole `tags:` line(s), removed when the list becomes empty.
    Key {
        style: Style,
        items: Vec<Item>,
        region: Range<usize>,
        key_lines: Range<usize>,
        indent: String,
        /// The whole frontmatter block, delimiters included: removed when
        /// dropping `tags:` would leave it empty.
        block: Range<usize>,
    },
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Lines of `text` with their byte ranges (range excludes the `\n`).
fn lines(text: &str) -> Vec<(Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        out.push((at..at + body.len(), body.trim_end_matches('\r')));
        at += line.len();
    }
    out
}

fn locate(text: &str) -> Place {
    let all = lines(text);
    if all.first().map(|(_, l)| *l) != Some("---") {
        return Place::NoFrontmatter;
    }
    let Some(close) = all
        .iter()
        .skip(1)
        .position(|(_, l)| *l == "---" || *l == "...")
    else {
        return Place::NoFrontmatter;
    };
    let body = &all[1..=close];
    let closing_start = all[close + 1].0.start;
    let block = 0..(all[close + 1].0.end + 1).min(text.len());
    let Some(k) = body.iter().position(|(_, l)| l.starts_with("tags:")) else {
        return Place::NoKey {
            insert_at: closing_start,
        };
    };
    let (key_range, key_line) = &body[k];
    let after = key_line["tags:".len()..].trim();
    let value_start = key_range.start + key_line.find(':').map_or(0, |i| i + 1);
    if after.starts_with('[') {
        let open = text[value_start..key_range.end]
            .find('[')
            .map_or(value_start, |i| value_start + i)
            + 1;
        let close_at = text[open..key_range.end]
            .find(']')
            .map_or(key_range.end, |i| open + i);
        let items = split_flow(&text[open..close_at]);
        return Place::Key {
            style: Style::Flow,
            items,
            region: open..close_at,
            key_lines: key_range.start..key_range.end + 1,
            indent: String::new(),
            block,
        };
    }
    if after.is_empty() {
        let mut items = Vec::new();
        let mut last_end = key_range.end;
        let mut first_start = None;
        let mut indent = "  ".to_string();
        for (range, line) in &body[k + 1..] {
            let trimmed = line.trim_start();
            let Some(value) = trimmed
                .strip_prefix("- ")
                .or_else(|| (trimmed == "-").then_some(""))
            else {
                break;
            };
            if line.len() == trimmed.len() && !line.starts_with('-') {
                break;
            }
            indent = line[..line.len() - trimmed.len()].to_string();
            first_start.get_or_insert(range.start);
            last_end = range.end;
            items.push(Item {
                raw: value.trim().to_string(),
                tag: unquote(value),
            });
        }
        let region = first_start.map_or(key_range.end..key_range.end, |s| s..last_end);
        return Place::Key {
            style: Style::Block,
            items,
            region,
            key_lines: key_range.start..last_end + 1,
            indent,
            block,
        };
    }
    let value_at = value_start
        + (text[value_start..key_range.end].len()
            - text[value_start..key_range.end].trim_start().len());
    Place::Key {
        style: Style::Scalar,
        items: vec![Item {
            raw: after.to_string(),
            tag: unquote(after),
        }],
        region: value_at..key_range.end,
        key_lines: key_range.start..key_range.end + 1,
        indent: String::new(),
        block,
    }
}

fn split_flow(inner: &str) -> Vec<Item> {
    inner
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|raw| Item {
            raw: raw.to_string(),
            tag: unquote(raw),
        })
        .collect()
}

/// The frontmatter Tags of a Page, lowercase, in order.
#[must_use]
pub fn tags(text: &str) -> Vec<String> {
    match locate(text) {
        Place::Key { items, .. } => items.into_iter().map(|i| i.tag.to_lowercase()).collect(),
        _ => Vec::new(),
    }
}

/// A splice that rewrites the frontmatter Tags with `edit`, which maps the
/// current entries (raw text, Tag) to the new list of raw entries. `None` if
/// nothing changes.
pub fn edit_tags(
    text: &str,
    edit: impl FnOnce(&[(String, String)]) -> Vec<String>,
) -> Option<Splice> {
    let place = locate(text);
    let current: Vec<(String, String)> = match &place {
        Place::Key { items, .. } => items
            .iter()
            .map(|i| (i.raw.clone(), i.tag.clone()))
            .collect(),
        _ => Vec::new(),
    };
    let new = edit(&current);
    if new
        == current
            .iter()
            .map(|(raw, _)| raw.clone())
            .collect::<Vec<_>>()
    {
        return None;
    }
    let block = |indent: &str| {
        new.iter()
            .map(|t| format!("{indent}- {t}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (range, replacement) = match place {
        // Nothing to remove where there are no Tags.
        Place::NoFrontmatter | Place::NoKey { .. } if new.is_empty() => return None,
        // A new block uses block style: one item per line merges cleanly.
        Place::NoFrontmatter => (0..0, format!("---\ntags:\n{}\n---\n", block("  "))),
        Place::NoKey { insert_at } => (insert_at..insert_at, format!("tags:\n{}\n", block("  "))),
        Place::Key {
            key_lines, block, ..
        } if new.is_empty() => {
            let key_lines = key_lines.start..key_lines.end.min(text.len());
            // Nothing else in the frontmatter? Then the whole block goes.
            let rest_blank = text[4..key_lines.start].trim().is_empty()
                && text[key_lines.end..block.end]
                    .trim_end()
                    .trim_end_matches("---")
                    .trim()
                    .is_empty();
            if rest_blank {
                (block, String::new())
            } else {
                (key_lines, String::new())
            }
        }
        Place::Key {
            style: Style::Flow,
            region,
            ..
        } => (region, new.join(", ")),
        Place::Key {
            style: Style::Scalar,
            region,
            ..
        } if new.len() == 1 => (region, new[0].clone()),
        Place::Key {
            style: Style::Scalar,
            region,
            ..
        } => (region, format!("[{}]", new.join(", "))),
        Place::Key {
            style: Style::Block,
            region,
            indent,
            ..
        } if region.is_empty() => (region.clone(), format!("\n{}", block(&indent))),
        Place::Key {
            style: Style::Block,
            region,
            indent,
            ..
        } => (region, block(&indent)),
    };
    Some(Splice {
        range: [range.start, range.end],
        old: text[range.clone()].to_string(),
        new: replacement,
    })
}

/// A top-level `key:` entry: the key (unquoted) and its lines, from the key
/// line through its last continuation line, trailing `\n` included.
#[derive(Debug)]
struct Entry {
    key: String,
    lines: Range<usize>,
}

/// A frontmatter block: its whole range (delimiters included), its body
/// between the delimiters, and its top-level entries.
#[derive(Debug)]
struct Block {
    whole: Range<usize>,
    body: Range<usize>,
    entries: Vec<Entry>,
}

/// The key a top-level line starts, if it starts one (`key:` or `"key":`).
fn key_of(line: &str) -> Option<String> {
    if line.is_empty()
        || line.starts_with([' ', '\t', '#'])
        || line == "-"
        || line.starts_with("- ")
    {
        return None;
    }
    let end = if line.starts_with(['"', '\'']) {
        let quote = &line[..1];
        line[1..].find(quote)? + 2
    } else {
        // A plain key ends at the first `:` followed by a space or the line end.
        line.match_indices(':')
            .map(|(i, _)| i)
            .find(|&i| line[i + 1..].is_empty() || line[i + 1..].starts_with([' ', '\t']))?
    };
    line[end..].starts_with(':').then(|| unquote(&line[..end]))
}

fn block(text: &str) -> Option<Block> {
    let all = lines(text);
    if all.first().map(|(_, l)| *l) != Some("---") {
        return None;
    }
    let close = 1 + all
        .iter()
        .skip(1)
        .position(|(_, l)| *l == "---" || *l == "...")?;
    let mut entries: Vec<Entry> = Vec::new();
    for (range, line) in &all[1..close] {
        let end = (range.end + 1).min(text.len());
        if let Some(key) = key_of(line) {
            entries.push(Entry {
                key,
                lines: range.start..end,
            });
        } else if !line.trim().is_empty() && line.starts_with([' ', '\t', '-']) {
            // A continuation: blank lines and comments before it come along.
            if let Some(e) = entries.last_mut() {
                e.lines.end = end;
            }
        }
    }
    Some(Block {
        whole: 0..(all[close].0.end + 1).min(text.len()),
        body: all[1].0.start..all[close].0.start,
        entries,
    })
}

fn to_yaml(value: &Value) -> Yaml {
    match value {
        Value::Null => Yaml::Null,
        Value::Bool(b) => Yaml::Boolean(*b),
        Value::Number(n) => n
            .as_i64()
            .map_or_else(|| Yaml::Real(n.to_string()), Yaml::Integer),
        Value::String(s) => Yaml::String(s.clone()),
        Value::Array(items) => Yaml::Array(items.iter().map(to_yaml).collect()),
        Value::Object(map) => Yaml::Hash(
            map.iter()
                .map(|(k, v)| (Yaml::String(k.clone()), to_yaml(v)))
                .collect(),
        ),
    }
}

/// `key: value` as YAML, one entry, ending in `\n`.
fn entry_text(key: &str, value: &Value) -> String {
    let mut hash = yaml_rust2::yaml::Hash::new();
    hash.insert(Yaml::String(key.to_string()), to_yaml(value));
    let mut out = String::new();
    YamlEmitter::new(&mut out)
        .dump(&Yaml::Hash(hash))
        .expect("emitting into a String cannot fail");
    format!("{}\n", out.strip_prefix("---\n").unwrap_or(&out))
}

/// The value an entry's lines hold, as JSON.
fn entry_value(text: &str, entry: &Entry) -> Option<Value> {
    let doc = YamlLoader::load_from_str(&text[entry.lines.clone()])
        .ok()?
        .into_iter()
        .next()?;
    match yaml_to_json(&doc) {
        Value::Object(mut map) => map.remove(&entry.key),
        _ => None,
    }
}

/// Splices that set top-level frontmatter keys (`None` removes one). Only
/// the changed keys' lines are touched: a changed key is rewritten in place,
/// new keys go at the end of the block, the block is created if absent and
/// removed once nothing is left in it. Keys already holding the value are
/// left alone.
#[must_use]
pub fn set_keys(text: &str, changes: &[(String, Option<Value>)]) -> Vec<Splice> {
    let found = block(text);
    let entries = found.as_ref().map_or(&[][..], |b| &b.entries[..]);
    let mut splices = Vec::new();
    let mut added = String::new();
    let mut removed = Vec::new();
    for (key, value) in changes {
        let existing = entries.iter().find(|e| &e.key == key);
        match (existing, value) {
            (None, None) => {}
            (None, Some(v)) => added.push_str(&entry_text(key, v)),
            (Some(e), Some(v)) if entry_value(text, e).as_ref() == Some(v) => {}
            (Some(e), v) => {
                let new = v.as_ref().map(|v| entry_text(key, v)).unwrap_or_default();
                if v.is_none() {
                    removed.push(e.lines.clone());
                }
                splices.push(Splice {
                    range: [e.lines.start, e.lines.end],
                    old: text[e.lines.clone()].to_string(),
                    new,
                });
            }
        }
    }
    let splice_at = |at: usize, new: String| Splice {
        range: [at, at],
        old: String::new(),
        new,
    };
    match found {
        None if !added.is_empty() => splices.push(splice_at(0, format!("---\n{added}---\n"))),
        None => {}
        Some(b) if !added.is_empty() => splices.push(splice_at(b.body.end, added)),
        Some(b) => {
            // Nothing else in the frontmatter? Then the whole block goes.
            let left = b
                .body
                .clone()
                .filter(|i| !removed.iter().any(|r| r.contains(i)));
            let emptied = !removed.is_empty()
                && left
                    .clone()
                    .all(|i| text.as_bytes()[i].is_ascii_whitespace());
            if emptied {
                return vec![Splice {
                    range: [b.whole.start, b.whole.end],
                    old: text[b.whole.clone()].to_string(),
                    new: String::new(),
                }];
            }
        }
    }
    splices.sort_by_key(|s| s.range[0]);
    splices
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, edit: impl FnOnce(&[(String, String)]) -> Vec<String>) -> String {
        match edit_tags(text, edit) {
            Some(s) => crate::plan::splice(text, &[s]),
            None => text.to_string(),
        }
    }

    fn add(tag: &'static str) -> impl FnOnce(&[(String, String)]) -> Vec<String> {
        move |cur| {
            cur.iter()
                .map(|(r, _)| r.clone())
                .chain([tag.to_string()])
                .collect()
        }
    }

    #[test]
    fn adding_keeps_the_existing_style() {
        assert_eq!(
            apply("# Body\n", add("a")),
            "---\ntags:\n  - a\n---\n# Body\n"
        );
        assert_eq!(
            apply("---\ntitle: T\n---\nx\n", add("a")),
            "---\ntitle: T\ntags:\n  - a\n---\nx\n"
        );
        assert_eq!(
            apply("---\ntags: [x, \"y\"]\nz: 1\n---\n", add("a")),
            "---\ntags: [x, \"y\", a]\nz: 1\n---\n"
        );
        assert_eq!(
            apply("---\ntags:\n    - x\nz: 1\n---\n", add("a")),
            "---\ntags:\n    - x\n    - a\nz: 1\n---\n"
        );
        assert_eq!(
            apply("---\ntags: x\n---\n", add("a")),
            "---\ntags: [x, a]\n---\n"
        );
        assert_eq!(
            apply("---\ntags: []\n---\n", add("a")),
            "---\ntags: [a]\n---\n"
        );
    }

    #[test]
    fn removing_the_last_tag_removes_the_key() {
        let drop_all = |_: &[(String, String)]| Vec::new();
        assert_eq!(
            apply("---\ntitle: T\ntags: [x]\n---\n", drop_all),
            "---\ntitle: T\n---\n"
        );
        assert_eq!(
            apply("---\ntags:\n  - x\n  - y\ntitle: T\n---\n", drop_all),
            "---\ntitle: T\n---\n"
        );
        assert_eq!(
            apply("---\ntags: [x]\n---\n# Body\n", drop_all),
            "# Body\n",
            "an emptied block goes entirely"
        );
        let drop_x = |cur: &[(String, String)]| {
            cur.iter()
                .filter(|(_, t)| t != "x")
                .map(|(r, _)| r.clone())
                .collect()
        };
        assert_eq!(
            apply("---\ntags:\n  - x\n  - y\n---\n", drop_x),
            "---\ntags:\n  - y\n---\n"
        );
    }

    fn set(text: &str, changes: &[(&str, Option<Value>)]) -> String {
        let changes: Vec<(String, Option<Value>)> = changes
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        crate::plan::splice(text, &set_keys(text, &changes))
    }

    #[test]
    fn setting_a_key_touches_only_its_lines() {
        use serde_json::json;
        assert_eq!(
            set("# Body\n", &[("title", Some(json!("Hi")))]),
            "---\ntitle: Hi\n---\n# Body\n",
            "the block is created"
        );
        assert_eq!(
            set(
                "---\ntitle: Old\n# note\ntags: [a]\n---\nx\n",
                &[
                    ("title", Some(json!("New: two"))),
                    ("order", Some(json!(10)))
                ]
            ),
            "---\ntitle: \"New: two\"\n# note\ntags: [a]\norder: 10\n---\nx\n",
            "rewritten in place, new keys appended, quoted when needed"
        );
        assert_eq!(
            set(
                "---\nlist:\n  - a\n\n  - b\nz: 1\n---\n",
                &[("list", Some(json!(["c"])))]
            ),
            "---\nlist:\n  - c\nz: 1\n---\n",
            "a multi-line value is replaced whole"
        );
        assert_eq!(
            set(
                "---\nnested: {a: 1}\n---\n",
                &[("nested", Some(json!({"a": [1, 2]})))]
            ),
            "---\nnested:\n  a:\n    - 1\n    - 2\n---\n"
        );
        let same = "---\norder:   10\n\"odd key\": x\n---\n";
        assert_eq!(
            set(
                same,
                &[("order", Some(json!(10))), ("odd key", Some(json!("x")))]
            ),
            same,
            "an unchanged value keeps its formatting"
        );
    }

    #[test]
    fn removing_the_last_key_removes_the_block() {
        assert_eq!(
            set("---\na: 1\nb:\n  - x\n---\n", &[("b", None)]),
            "---\na: 1\n---\n"
        );
        assert_eq!(
            set("---\nb:\n# note\n  - x\na: 1\n---\n", &[("b", None)]),
            "---\na: 1\n---\n",
            "a comment inside a value doesn't end it"
        );
        assert_eq!(
            set("---\nb:\n- x\n- y\na: 1\n---\n", &[("b", None)]),
            "---\na: 1\n---\n",
            "list items at column 0 belong to the key"
        );
        assert_eq!(
            set("---\nb: 1\n  \na: 1\n---\n", &[("b", None)]),
            "---\n  \na: 1\n---\n",
            "a whitespace-only line doesn't continue a value"
        );
        assert_eq!(
            set(
                "---\na: 1\n\nb: 2\n---\n# Body\n",
                &[("a", None), ("b", None)]
            ),
            "# Body\n"
        );
        assert_eq!(
            set("# Body\n", &[("a", None)]),
            "# Body\n",
            "absent: nothing"
        );
        assert_eq!(
            set("---\n# keep me\na: 1\n---\n", &[("a", None)]),
            "---\n# keep me\n---\n",
            "a comment is something left"
        );
    }

    #[test]
    fn keys_are_found_by_their_top_level_lines() {
        assert_eq!(key_of("title: x"), Some("title".into()));
        assert_eq!(key_of("url: http://x"), Some("url".into()));
        assert_eq!(key_of("a:b: x"), Some("a:b".into()), "`:` without a space");
        assert_eq!(key_of("\"a: b\": x"), Some("a: b".into()));
        assert_eq!(key_of("empty:"), Some("empty".into()));
        assert_eq!(key_of("  nested: x"), None);
        assert_eq!(key_of("- item"), None);
        assert_eq!(key_of("- a: b"), None, "a list item, not a key");
        assert_eq!(key_of("# c: x"), None);
        assert_eq!(key_of("no colon"), None);
    }

    #[test]
    fn reads_every_style() {
        assert_eq!(tags("---\ntags: [A, \"b/c\"]\n---\n"), ["a", "b/c"]);
        assert_eq!(tags("---\ntags:\n- a\n- 'b'\n---\n"), ["a", "b"]);
        assert_eq!(tags("---\ntags: solo\n---\n"), ["solo"]);
        assert!(tags("no frontmatter").is_empty());
    }
}
