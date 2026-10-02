//! The page hierarchy (ADR 0003): Child Pages and Placeholders from the folder
//! tree, sibling order from `order:` (on-disk layout, item 5), and Spaces.

use std::{cmp::Ordering, collections::BTreeMap};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    Error, Kind, Operation, Result, Wiki, frontmatter,
    index::{HierarchyRow, page_path},
    mutations::{push_modify, read_existing},
    plan::{Plan, Tx, mutate},
    wiki::PagePath,
};

/// Gap left between `order:` values, so a reorder usually writes one Page.
const GAP: i64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Page,
    Placeholder,
}

/// One position in the hierarchy, as `children` lists it.
#[derive(Debug, Clone)]
struct Node {
    path: String,
    title: String,
    kind: NodeKind,
    order: Option<f64>,
    has_children: bool,
}

/// The Child Pages and Placeholders directly under `parent` ("" for the
/// Wiki root), in display order, from `rows` (every Page at or under it).
fn child_nodes(rows: &[HierarchyRow], parent: &str) -> Vec<Node> {
    let prefix = if parent.is_empty() {
        String::new()
    } else {
        format!("{parent}/")
    };
    let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
    for row in rows {
        let Some(rest) = row.file.strip_prefix(&prefix) else {
            continue;
        };
        let (name, below) = match rest.split_once('/') {
            Some((folder, _)) => (folder, true),
            None => (rest.strip_suffix(".md").unwrap_or(rest), false),
        };
        let path = format!("{prefix}{name}");
        let node = nodes.entry(path.clone()).or_insert_with(|| Node {
            path,
            title: name.to_string(),
            kind: NodeKind::Placeholder,
            order: None,
            has_children: false,
        });
        if below {
            node.has_children = true;
        } else {
            node.kind = NodeKind::Page;
            node.title.clone_from(&row.title);
            node.order = row.order;
        }
    }
    let mut nodes: Vec<Node> = nodes.into_values().collect();
    nodes.sort_by(display_order);
    nodes
}

/// Pages with an `order` first, by it; then the rest, Pages by Title and
/// Placeholders by name, in natural order.
fn display_order(a: &Node, b: &Node) -> Ordering {
    let by_order = match (a.order, b.order) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    by_order
        .then_with(|| natural(&a.title, &b.title))
        .then_with(|| a.path.cmp(&b.path))
}

/// Case-insensitive, with runs of digits compared as numbers ("Part 2" < "Part 10").
fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    let digits = |it: &mut std::iter::Peekable<std::str::Chars>| {
        let mut run = String::new();
        while let Some(c) = it.next_if(char::is_ascii_digit) {
            run.push(c);
        }
        run
    };
    loop {
        let found = match (a.peek(), b.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (x, y) = (digits(&mut a), digits(&mut b));
                let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                x.len().cmp(&y.len()).then_with(|| x.cmp(y))
            }
            (Some(x), Some(y)) => {
                let found = x.to_lowercase().cmp(y.to_lowercase());
                a.next();
                b.next();
                found
            }
        };
        if found != Ordering::Equal {
            return found;
        }
    }
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

// ------------------------------------------------------------------ children

pub struct Children;

/// List the Child Pages and Placeholders under a Page, in display order.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct ChildrenInput {
    /// Page Path or Placeholder; the Wiki root if absent.
    pub parent: Option<String>,
    /// Levels to list: 1 is direct children only.
    #[serde(default = "one")]
    #[cfg_attr(feature = "clap", arg(long, default_value_t = 1))]
    pub depth: u32,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ChildNode {
    pub path: String,
    /// The Page's Title, or a Placeholder's folder name.
    pub title: String,
    pub kind: NodeKind,
    pub has_children: bool,
    /// Its own children, while `depth` lasts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<ChildNode>>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ChildrenOutput {
    pub children: Vec<ChildNode>,
}

fn tree(rows: &[HierarchyRow], parent: &str, depth: u32) -> Vec<ChildNode> {
    child_nodes(rows, parent)
        .into_iter()
        .map(|n| ChildNode {
            children: (n.has_children && depth > 1).then(|| tree(rows, &n.path, depth - 1)),
            path: n.path,
            title: n.title,
            kind: n.kind,
            has_children: n.has_children,
        })
        .collect()
}

impl Operation for Children {
    const NAME: &'static str = "children";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str =
        "List the Child Pages and Placeholders under a Page (or the Wiki root), in display order.";
    type Input = ChildrenInput;
    type Output = ChildrenOutput;

    fn run(wiki: &Wiki, input: ChildrenInput) -> Result<ChildrenOutput> {
        if input.depth == 0 {
            return Err(Error::invalid_input(Some("depth"), "`depth` is at least 1"));
        }
        let parent = match &input.parent {
            Some(raw) => PagePath::parse(raw)?.as_str().to_string(),
            None => String::new(),
        };
        let rows = wiki.index().pages_under(&parent)?;
        if !parent.is_empty() && rows.is_empty() {
            return Err(Error::not_found("page", &parent));
        }
        Ok(ChildrenOutput {
            children: tree(&rows, &parent, input.depth),
        })
    }
}

// --------------------------------------------------------------- list_spaces

pub struct ListSpaces;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct ListSpacesInput {}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Space {
    pub name: String,
    /// `<name>`, if the Space has a home Page.
    pub home_page: Option<String>,
    /// Pages in the Space, its home Page included.
    pub page_count: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListSpacesOutput {
    pub spaces: Vec<Space>,
}

impl Operation for ListSpaces {
    const NAME: &'static str = "list_spaces";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str =
        "List the Spaces (top-level folders holding Pages) with their home Pages and Page counts.";
    type Input = ListSpacesInput;
    type Output = ListSpacesOutput;

    fn run(wiki: &Wiki, _input: ListSpacesInput) -> Result<ListSpacesOutput> {
        let rows = wiki.index().pages_under("")?;
        let mut spaces: BTreeMap<&str, Space> = BTreeMap::new();
        for row in &rows {
            if let Some((name, _)) = row.file.split_once('/') {
                spaces
                    .entry(name)
                    .or_insert_with(|| Space {
                        name: name.to_string(),
                        home_page: None,
                        page_count: 0,
                    })
                    .page_count += 1;
            }
        }
        // A root Page is a home Page only if its Space exists.
        for row in &rows {
            let path = page_path(&row.file);
            if let Some(space) = spaces.get_mut(path.as_str()) {
                space.home_page = Some(path);
                space.page_count += 1;
            }
        }
        Ok(ListSpacesOutput {
            spaces: spaces.into_values().collect(),
        })
    }
}

// -------------------------------------------------------------- reorder_page

pub struct ReorderPage;

/// Move a Page before or after one of its siblings, by writing its `order:`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct ReorderPageInput {
    /// Page Path.
    pub page: String,
    /// Place it just before this sibling.
    #[cfg_attr(
        feature = "clap",
        arg(long, conflicts_with = "after", required_unless_present = "after")
    )]
    pub before: Option<String>,
    /// Place it just after this sibling.
    #[cfg_attr(feature = "clap", arg(long))]
    pub after: Option<String>,
    /// Return the Plan without applying it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ReorderPageOutput {
    pub path: String,
    pub plan: Plan,
    pub applied: bool,
    /// Pages whose `order:` is written: the Page, plus any renumbered siblings.
    pub written: BTreeMap<String, i64>,
}

impl Operation for ReorderPage {
    const NAME: &'static str = "reorder_page";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str = "Place a Page before or after a sibling by writing its `order:` (siblings are renumbered only when no gap is left).";
    type Input = ReorderPageInput;
    type Output = ReorderPageOutput;

    fn run(wiki: &Wiki, input: ReorderPageInput) -> Result<ReorderPageOutput> {
        let page = PagePath::parse(&input.page)?;
        let (field, anchor, after) = match (&input.before, &input.after) {
            (Some(b), None) => ("before", b, false),
            (None, Some(a)) => ("after", a, true),
            _ => {
                return Err(Error::invalid_input(
                    None,
                    "give exactly one of `before` or `after`",
                ));
            }
        };
        let anchor = PagePath::parse(anchor)?;
        let (plan, written) = mutate(wiki, input.dry_run, |tx| {
            read_existing(tx, &page)?;
            let parent = parent_of(page.as_str());
            let not_sibling = || {
                Error::invalid_input(
                    Some(field),
                    format!(
                        "`{}` is not a sibling of `{}`",
                        anchor.as_str(),
                        page.as_str()
                    ),
                )
            };
            if parent_of(anchor.as_str()) != parent {
                return Err(not_sibling());
            }
            let siblings = child_nodes(&tx.wiki().index().pages_under(parent)?, parent);
            let mut others: Vec<Node> = siblings
                .iter()
                .filter(|n| n.path != page.as_str())
                .cloned()
                .collect();
            let at = others
                .iter()
                .position(|n| n.path == anchor.as_str())
                .ok_or_else(not_sibling)?
                + usize::from(after);
            let moved = siblings
                .iter()
                .find(|n| n.path == page.as_str())
                .cloned()
                .ok_or_else(|| Error::internal("the Page is missing from the Index"))?;
            others.insert(at, moved);
            let new = others;
            if new
                .iter()
                .map(|n| &n.path)
                .eq(siblings.iter().map(|n| &n.path))
            {
                return Ok(BTreeMap::new());
            }
            let orders = orders_for(&new, at);
            for n in &new[..=at] {
                if n.kind == NodeKind::Placeholder {
                    tx.warn(
                        "placeholder_unordered",
                        format!(
                            "`{}` is a Placeholder and can't carry `order:`, so it sorts after the ordered Pages",
                            n.path
                        ),
                    );
                }
            }
            for (path, order) in &orders {
                write_order(tx, path, *order)?;
            }
            Ok(orders)
        })?;
        Ok(ReorderPageOutput {
            path: page.as_str().to_string(),
            plan,
            applied: !input.dry_run,
            written,
        })
    }
}

/// The `order:` values to write so that `seq[at]` (the moved Page) sorts at
/// `at`: one value in the gap between its neighbours if there is one, else
/// the ordered run is renumbered in steps of [`GAP`]. Pages already holding
/// their value are left out.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // orders are small integers
fn orders_for(seq: &[Node], at: usize) -> BTreeMap<String, i64> {
    // The siblings with an `order` come first; the moved Page's own value is stale.
    let ordered = (0..seq.len())
        .filter(|&i| i != at && seq[i].order.is_some())
        .count();
    let before = seq[..at].iter().rev().find_map(|n| n.order);
    let after = seq[at + 1..].iter().find_map(|n| n.order);
    let mut out = BTreeMap::new();
    if at <= ordered {
        // Among (or right after) the ordered siblings: fit into the gap.
        let lo = before.unwrap_or(0.0);
        let fit = match after {
            None => Some(lo.floor() as i64 + GAP),
            Some(hi) => {
                let mid = f64::midpoint(lo, hi).floor();
                (mid > lo && mid < hi).then_some(mid as i64)
            }
        };
        if let Some(order) = fit {
            out.insert(seq[at].path.clone(), order);
            return out;
        }
        // No gap left: renumber the ordered run, the moved Page included.
        let mut next = GAP;
        for n in &seq[..=ordered] {
            if n.kind == NodeKind::Page {
                if n.order != Some(next as f64) {
                    out.insert(n.path.clone(), next);
                }
                next += GAP;
            }
        }
        return out;
    }
    // Among the unordered siblings: order them up to the moved Page.
    let mut next = before.map_or(0, |o| o.floor() as i64) + GAP;
    for n in &seq[..=at] {
        if (n.order.is_none() && n.kind == NodeKind::Page) || n.path == seq[at].path {
            out.insert(n.path.clone(), next);
            next += GAP;
        }
    }
    out
}

fn write_order(tx: &mut Tx, path: &str, order: i64) -> Result<()> {
    let page = PagePath::parse(path)?;
    let (file, current) = read_existing(tx, &page)?;
    let splices = frontmatter::set_keys(&current.content, &[("order".into(), Some(json!(order)))]);
    push_modify(tx, file, current.version, splices);
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::{ErrorKind, find};

    fn wiki(files: &[(&str, &str)]) -> (tempfile::TempDir, Wiki) {
        let dir = tempfile::tempdir().unwrap();
        for (rel, content) in files {
            let path = dir.path().join("wiki").join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        std::fs::create_dir_all(dir.path().join("wiki")).unwrap();
        let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("cache")).unwrap();
        (dir, wiki)
    }

    fn call(wiki: &Wiki, op: &str, input: Value) -> Result<Value> {
        find(op)
            .unwrap()
            .call(wiki, input)
            .map(|v| v["result"].clone())
    }

    /// `children` of `parent` as `path` (Placeholders marked `?`, `+` if it has children).
    fn listed(wiki: &Wiki, parent: Option<&str>) -> Vec<String> {
        let out = call(wiki, "children", json!({ "parent": parent })).unwrap();
        out["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                format!(
                    "{}{}{}",
                    c["path"].as_str().unwrap(),
                    if c["kind"] == "placeholder" { "?" } else { "" },
                    if c["has_children"] == true { "+" } else { "" }
                )
            })
            .collect()
    }

    fn order_of(wiki: &Wiki, page: &str) -> Value {
        let out = call(wiki, "get_page", json!({ "page": page })).unwrap();
        out["frontmatter"]["order"].clone()
    }

    #[test]
    fn children_put_ordered_pages_first_then_titles_and_names_naturally() {
        let (_dir, wiki) = wiki(&[
            ("eng.md", "# Engineering\n"),
            ("eng/b.md", "---\norder: 20\n---\n# Zed\n"),
            ("eng/a.md", "---\norder: 5\n---\n# Yak\n"),
            ("eng/part-10.md", "# Part 10\n"),
            ("eng/part-2.md", "# part 2\n"),
            ("eng/drafts/x.md", "# X\n"),
            ("eng/ops.md", "# Ops\n"),
            ("eng/ops/run.md", "# Run\n"),
            ("eng/img/only.png", "png"),
            ("eng/.hidden/h.md", "# H\n"),
            ("root.md", "# Root\n"),
        ]);
        assert_eq!(
            listed(&wiki, Some("eng")),
            [
                "eng/a",
                "eng/b",
                "eng/drafts?+",
                "eng/ops+",
                "eng/part-2",
                "eng/part-10"
            ],
            "an Attachment-only folder isn't a Placeholder; hidden folders don't count"
        );
        assert_eq!(listed(&wiki, None), ["eng+", "root"]);
        let titles: Vec<Value> =
            call(&wiki, "children", json!({ "parent": "eng" })).unwrap()["children"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["title"].clone())
                .collect();
        assert_eq!(
            titles,
            ["Yak", "Zed", "drafts", "Ops", "part 2", "Part 10"],
            "Pages show their Title, Placeholders their folder name"
        );
        assert_eq!(listed(&wiki, Some("eng/drafts")), ["eng/drafts/x"]);
        assert!(listed(&wiki, Some("root")).is_empty());

        let deep = call(&wiki, "children", json!({ "parent": "eng", "depth": 2 })).unwrap();
        let ops = &deep["children"][3];
        assert_eq!(ops["children"][0]["path"], "eng/ops/run");
        assert!(
            deep["children"][0].get("children").is_none(),
            "leaves aren't expanded"
        );
        assert!(
            call(&wiki, "children", json!({ "parent": "eng" })).unwrap()["children"][3]
                .get("children")
                .is_none(),
            "depth 1 lists one level"
        );

        for missing in ["nope", "Eng", "eng/img"] {
            let err = call(&wiki, "children", json!({ "parent": missing })).unwrap_err();
            assert_eq!(err.kind, ErrorKind::NotFound, "{missing}");
        }
        let err = call(&wiki, "children", json!({ "depth": 0 })).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
    }

    #[test]
    fn spaces_are_top_level_folders_holding_pages() {
        let (_dir, wiki) = wiki(&[
            ("eng.md", "# Eng\n"),
            ("eng/a.md", "# A\n"),
            ("eng/a/b.md", "# B\n"),
            ("ops/x.md", "# X\n"),
            ("assets/logo.png", "png"),
            ("loose.md", "# Loose\n"),
        ]);
        let out = call(&wiki, "list_spaces", json!({})).unwrap();
        assert_eq!(
            out["spaces"],
            json!([
                { "name": "eng", "home_page": "eng", "page_count": 3 },
                { "name": "ops", "home_page": null, "page_count": 1 },
            ])
        );
    }

    fn reorder(wiki: &Wiki, page: &str, rel: &str, anchor: &str) -> Result<Value> {
        call(wiki, "reorder_page", json!({ "page": page, rel: anchor }))
    }

    #[test]
    fn reorder_fills_a_gap_and_writes_only_the_moved_page() {
        let (_dir, wiki) = wiki(&[
            ("s/a.md", "---\norder: 10\n---\n# A\n"),
            ("s/b.md", "---\norder: 20\n---\n# B\n"),
            ("s/c.md", "---\norder: 30\ntitle: C\n---\nbody\n"),
            ("s/d.md", "# D\n"),
        ]);
        let out = reorder(&wiki, "s/c", "before", "s/b").unwrap();
        assert_eq!(out["written"], json!({ "s/c": 15 }));
        assert_eq!(out["plan"]["edits"].as_array().unwrap().len(), 1);
        assert_eq!(listed(&wiki, Some("s")), ["s/a", "s/c", "s/b", "s/d"]);
        let c = call(&wiki, "get_page", json!({ "page": "s/c" })).unwrap();
        assert_eq!(c["content"], "---\norder: 15\ntitle: C\n---\nbody\n");

        let out = reorder(&wiki, "s/a", "after", "s/b").unwrap();
        assert_eq!(out["written"], json!({ "s/a": 30 }), "past the last: +10");
        let out = reorder(&wiki, "s/a", "before", "s/c").unwrap();
        assert_eq!(
            out["written"],
            json!({ "s/a": 7 }),
            "first: between 0 and 15"
        );
        assert_eq!(listed(&wiki, Some("s")), ["s/a", "s/c", "s/b", "s/d"]);

        let out = reorder(&wiki, "s/c", "after", "s/a").unwrap();
        assert!(
            out["plan"]["edits"].as_array().unwrap().is_empty(),
            "already there"
        );
    }

    #[test]
    fn reorder_renumbers_the_ordered_run_when_no_gap_is_left() {
        let (_dir, wiki) = wiki(&[
            ("s/a.md", "---\norder: 1\n---\n# A\n"),
            ("s/b.md", "---\norder: 2\n---\n# B\n"),
            ("s/c.md", "---\norder: 20\n---\n# C\n"),
            ("s/d.md", "# D\n"),
        ]);
        let out = reorder(&wiki, "s/c", "after", "s/a").unwrap();
        assert_eq!(
            out["written"],
            json!({ "s/a": 10, "s/b": 30 }),
            "every ordered Page whose number changes (c already holds 20)"
        );
        assert_eq!(listed(&wiki, Some("s")), ["s/a", "s/c", "s/b", "s/d"]);
        assert_eq!(
            order_of(&wiki, "s/d"),
            Value::Null,
            "unordered ones stay so"
        );
    }

    #[test]
    fn reorder_among_unordered_siblings_orders_those_before_it() {
        let (_dir, wiki) = wiki(&[
            ("s/a.md", "---\norder: 10\n---\n# A\n"),
            ("s/b.md", "# B\n"),
            ("s/c.md", "# C\n"),
            ("s/d.md", "# D\n"),
            ("s/p/x.md", "# X\n"),
        ]);
        let out = reorder(&wiki, "s/d", "after", "s/b").unwrap();
        assert_eq!(out["written"], json!({ "s/b": 20, "s/d": 30 }));
        assert_eq!(
            listed(&wiki, Some("s")),
            ["s/a", "s/b", "s/d", "s/c", "s/p?+"]
        );

        let out = call(
            &wiki,
            "reorder_page",
            json!({ "page": "s/c", "after": "s/p", "dry_run": true }),
        )
        .unwrap();
        let warnings: Vec<&str> = out["plan"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["kind"].as_str().unwrap())
            .collect();
        assert_eq!(warnings, ["placeholder_unordered"]);

        let out = reorder(&wiki, "s/a", "after", "s/c").unwrap();
        assert_eq!(
            out["written"],
            json!({ "s/c": 40, "s/a": 50 }),
            "an ordered Page moved among unordered ones gets a new value"
        );
        assert_eq!(
            listed(&wiki, Some("s")),
            ["s/b", "s/d", "s/c", "s/a", "s/p?+"]
        );
    }

    #[test]
    fn reorder_rejects_what_isnt_a_sibling() {
        let (_dir, wiki) = wiki(&[
            ("s/a.md", "# A\n"),
            ("s/b.md", "# B\n"),
            ("t/c.md", "# C\n"),
        ]);
        for (anchor, kind) in [
            ("t/c", ErrorKind::InvalidInput),
            ("s/zz", ErrorKind::InvalidInput),
            ("s/a", ErrorKind::InvalidInput),
        ] {
            let err = reorder(&wiki, "s/a", "before", anchor).unwrap_err();
            assert_eq!(err.kind, kind, "{anchor}");
        }
        let err = reorder(&wiki, "s/nope", "before", "s/a").unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        let err = call(
            &wiki,
            "reorder_page",
            json!({ "page": "s/a", "before": "s/b", "after": "s/b" }),
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
    }

    #[test]
    fn natural_order_ignores_case_and_reads_numbers() {
        let mut v = vec!["part 10", "Part 2", "part 1", "Alpha", "beta", "part 02x"];
        v.sort_by(|a, b| natural(a, b).then_with(|| a.cmp(b)));
        assert_eq!(
            v,
            ["Alpha", "beta", "part 1", "Part 2", "part 02x", "part 10"]
        );
    }
}
