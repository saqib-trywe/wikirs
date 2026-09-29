//! Locating and rewriting the `tags:` entries of a Page's YAML frontmatter
//! (Tag model, item 6): only the Tags region is regenerated; every other byte
//! of the file stays as it was, and the existing list style is kept.

use std::ops::Range;

use crate::plan::Splice;

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

    #[test]
    fn reads_every_style() {
        assert_eq!(tags("---\ntags: [A, \"b/c\"]\n---\n"), ["a", "b/c"]);
        assert_eq!(tags("---\ntags:\n- a\n- 'b'\n---\n"), ["a", "b"]);
        assert_eq!(tags("---\ntags: solo\n---\n"), ["solo"]);
        assert!(tags("no frontmatter").is_empty());
    }
}
