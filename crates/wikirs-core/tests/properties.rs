//! Properties over generated Wikis (testing.md#plan-properties and
//! #index-consistency). Each case builds a small Wiki, then runs a random
//! sequence of mutations and external edits, checking along the way that:
//! - a dry run writes nothing, and its Plan equals the one that gets applied;
//! - repeating a state-setting mutation plans nothing (idempotence);
//! - `move_page a→b` then `b→a` restores every file byte for byte, and so
//!   does `rename_tag` (round trip);
//! - a move or delete breaks no Link that its Plan didn't warn about.
//!
//! At the end, the incrementally kept Index, after a reconcile scan,
//! must equal one rebuilt from scratch. An external edit reaches the Index either
//! as the watcher's would (a refresh of the files touched) or at the next
//! open (a reconcile scan).
//!
//! `PROPTEST_CASES` raises the number of cases (default 48).

use std::{collections::BTreeMap, fmt::Write, fs, path::Path};

use proptest::prelude::*;
use serde_json::{Value, json};
use wikirs_core::{Wiki, find};

const PAGES: [&str; 7] = ["a", "b", "c", "a/x", "a/y", "b/z", "a/x/deep"];
const TAGS: [&str; 4] = ["t", "t/sub", "u", "v"];
/// Targets only moves and renames use, so round trips find them free.
const FRESH_PAGES: [&str; 3] = ["n", "m/k", "a/new"];
const FRESH_TAGS: [&str; 3] = ["w", "w/x", "t/new"];

// ------------------------------------------------------------- generators

#[derive(Debug, Clone)]
enum Piece {
    Heading(u8, &'static str),
    Text(&'static str),
    WikiLink(&'static str, Option<&'static str>),
    StdLink(&'static str),
    Tag(&'static str),
}

#[derive(Debug, Clone)]
struct Content {
    front_tags: Vec<&'static str>,
    pieces: Vec<Piece>,
}

impl Content {
    /// The markdown for a Page at `page` (standard Links are relative to it).
    fn render(&self, page: &str) -> String {
        let mut out = String::new();
        if !self.front_tags.is_empty() {
            let _ = write!(out, "---\ntags: [{}]\n---\n", self.front_tags.join(", "));
        }
        for piece in &self.pieces {
            let _ = match piece {
                Piece::Heading(level, text) => {
                    write!(out, "\n{} {text}\n\n", "#".repeat(usize::from(*level)))
                }
                Piece::Text(text) => write!(out, "{text} "),
                Piece::WikiLink(target, None) => write!(out, "[[{target}]] "),
                Piece::WikiLink(target, Some(heading)) => write!(out, "[[{target}#{heading}]] "),
                Piece::StdLink(target) => write!(out, "[to {target}]({}) ", relative(page, target)),
                Piece::Tag(tag) => write!(out, "#{tag} "),
            };
        }
        out + "\n"
    }
}

/// `target.md` relative to the folder of `page`, in the shortest form (the
/// one the core writes, so a round trip can be byte for byte).
fn relative(page: &str, target: &str) -> String {
    let from: Vec<&str> = page.split('/').collect();
    let from = &from[..from.len() - 1];
    let to: Vec<&str> = target.split('/').collect();
    // Only folders are shared: the target's last segment is its file name.
    let common = from
        .iter()
        .zip(&to[..to.len() - 1])
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts = vec![".."; from.len() - common];
    parts.extend(&to[common..]);
    format!("{}.md", parts.join("/"))
}

fn page() -> impl Strategy<Value = &'static str> {
    prop::sample::select(&PAGES[..])
}

fn tag() -> impl Strategy<Value = &'static str> {
    prop::sample::select(&TAGS[..])
}

fn move_target() -> impl Strategy<Value = &'static str> {
    prop_oneof![page(), prop::sample::select(&FRESH_PAGES[..])]
}

fn rename_target() -> impl Strategy<Value = &'static str> {
    prop_oneof![tag(), prop::sample::select(&FRESH_TAGS[..])]
}

fn piece() -> impl Strategy<Value = Piece> {
    prop_oneof![
        (
            1u8..=3,
            prop::sample::select(&["Intro", "Usage", "Notes"][..])
        )
            .prop_map(|(l, t)| Piece::Heading(l, t)),
        prop::sample::select(&["Some words.", "More text here.", "x"][..]).prop_map(Piece::Text),
        (
            page(),
            prop::option::of(prop::sample::select(&["intro", "usage"][..]))
        )
            .prop_map(|(p, h)| Piece::WikiLink(p, h)),
        page().prop_map(Piece::StdLink),
        tag().prop_map(Piece::Tag),
    ]
}

fn content() -> impl Strategy<Value = Content> {
    (
        prop::collection::vec(tag(), 0..3),
        prop::collection::vec(piece(), 0..8),
    )
        .prop_map(|(mut front_tags, pieces)| {
            front_tags.dedup();
            Content { front_tags, pieces }
        })
}

#[derive(Debug, Clone)]
enum Action {
    Create(&'static str, Content),
    Write(&'static str, Content),
    Move {
        from: &'static str,
        to: &'static str,
        round_trip: bool,
    },
    Delete(&'static str, bool),
    Tag(&'static str, Vec<&'static str>),
    Untag(&'static str, Vec<&'static str>),
    RenameTag {
        from: &'static str,
        to: &'static str,
        round_trip: bool,
    },
    SetMeta(&'static str, Option<&'static str>),
    ExternalWrite(&'static str, Content, Seen),
    ExternalDelete(&'static str, Seen),
    ExternalRename(&'static str, &'static str, Seen),
}

/// How an external edit reaches the Index.
#[derive(Debug, Clone, Copy)]
enum Seen {
    /// A watcher's refresh of the files touched.
    Watcher,
    /// The next `Wiki::open` (every CLI call): a reconcile scan.
    Reopen,
}

fn seen() -> impl Strategy<Value = Seen> {
    prop_oneof![Just(Seen::Watcher), Just(Seen::Reopen)]
}

fn action() -> impl Strategy<Value = Action> {
    let tags = || prop::collection::vec(tag(), 1..3);
    prop_oneof![
        (page(), content()).prop_map(|(p, c)| Action::Create(p, c)),
        (page(), content()).prop_map(|(p, c)| Action::Write(p, c)),
        (page(), move_target(), any::<bool>()).prop_map(|(from, to, round_trip)| Action::Move {
            from,
            to,
            round_trip
        }),
        (page(), any::<bool>()).prop_map(|(p, r)| Action::Delete(p, r)),
        (page(), tags()).prop_map(|(p, t)| Action::Tag(p, t)),
        (page(), tags()).prop_map(|(p, t)| Action::Untag(p, t)),
        (tag(), rename_target(), any::<bool>()).prop_map(|(from, to, round_trip)| {
            Action::RenameTag {
                from,
                to,
                round_trip,
            }
        }),
        (
            page(),
            prop::option::of(prop::sample::select(&["draft", "done"][..]))
        )
            .prop_map(|(p, s)| Action::SetMeta(p, s)),
        (page(), content(), seen()).prop_map(|(p, c, s)| Action::ExternalWrite(p, c, s)),
        (page(), seen()).prop_map(|(p, s)| Action::ExternalDelete(p, s)),
        (page(), page(), seen()).prop_map(|(a, b, s)| Action::ExternalRename(a, b, s)),
    ]
}

// ---------------------------------------------------------------- helpers

/// Every file under the Wiki root except `.wikirs/`, with its bytes.
fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == ".wikirs") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy();
                out.insert(rel.replace('\\', "/"), fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn call(wiki: &Wiki, op: &str, input: Value) -> Result<Value, wikirs_core::Error> {
    find(op).unwrap().call(wiki, input)
}

/// Broken Links in the Wiki, as `(page, raw)`.
fn broken(wiki: &Wiki) -> Vec<(String, String)> {
    let out = call(wiki, "check", json!({ "kinds": ["broken_link"] })).unwrap();
    let mut found: Vec<(String, String)> = out["result"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let page = d["page"].as_str().unwrap().to_string();
            (page, d["message"].as_str().unwrap().to_string())
        })
        .collect();
    found.sort();
    found
}

/// No Link broke that the Plan didn't warn about. Broken Links are counted per
/// Page (`page_after` says where each Page went): one that stays broken may be
/// described differently once its Page moved (`../a.md` escaping the Wiki,
/// then naming a missing Page).
fn check_breaks_warned(
    wiki: &Wiki,
    broken_before: &[(String, String)],
    out: &Value,
    page_after: impl Fn(&str) -> String,
) -> Result<(), TestCaseError> {
    let per_page = |found: &mut dyn Iterator<Item = String>| {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for page in found {
            *counts.entry(page).or_default() += 1;
        }
        counts
    };
    let broken_after = broken(wiki);
    let was = per_page(&mut broken_before.iter().map(|(p, _)| page_after(p)));
    let now = per_page(&mut broken_after.iter().map(|(p, _)| p.clone()));
    let new: usize = now
        .iter()
        .map(|(page, n)| n.saturating_sub(was.get(page).copied().unwrap_or(0)))
        .sum();
    let warned = out["warnings"].as_array().unwrap().len();
    prop_assert!(
        new <= warned,
        "Links broke without a warning: {broken_after:?}, warned: {}",
        out["warnings"]
    );
    Ok(())
}

/// Every Tag in the Wiki, at any depth.
fn tags(wiki: &Wiki) -> Vec<String> {
    fn walk(nodes: &Value, out: &mut Vec<String>) {
        for node in nodes.as_array().into_iter().flatten() {
            out.push(node["tag"].as_str().unwrap().to_string());
            walk(&node["children"], out);
        }
    }
    let out = call(wiki, "tag_tree", json!({})).unwrap();
    let mut found = Vec::new();
    walk(&out["result"]["tags"], &mut found);
    found
}

/// A mutation, checked: its dry run writes nothing and plans what is applied.
/// Returns the applied outcome.
fn mutate(wiki: &Wiki, op: &str, input: &Value) -> Result<Value, TestCaseError> {
    let before = files(wiki.root());
    let mut dry_input = input.clone();
    dry_input["dry_run"] = json!(true);
    let dry = call(wiki, op, dry_input);
    prop_assert_eq!(&files(wiki.root()), &before, "{} dry run wrote files", op);
    let done = call(wiki, op, input.clone());
    match (&dry, &done) {
        (Ok(dry), Ok(done)) => prop_assert_eq!(
            &dry["result"]["plan"],
            &done["result"]["plan"],
            "{} dry-run Plan differs from the applied one",
            op
        ),
        (Err(dry), Err(done)) => prop_assert_eq!(dry.kind, done.kind),
        _ => prop_assert!(false, "{op}: dry run {dry:?} but applied {done:?}"),
    }
    Ok(done.unwrap_or_else(|e| e.to_json()))
}

fn applied(outcome: &Value) -> bool {
    outcome.get("result").is_some()
}

/// Repeating a state-setting mutation must plan nothing.
fn check_idempotent(wiki: &Wiki, op: &str, input: &Value) -> Result<(), TestCaseError> {
    let mut again = input.clone();
    again["dry_run"] = json!(true);
    let out = call(wiki, op, again).unwrap();
    prop_assert_eq!(
        &out["result"]["plan"]["edits"],
        &json!([]),
        "{} again planned more edits",
        op
    );
    Ok(())
}

/// Where `page` is after moving `from` (and the Pages under it) to `to`.
fn moved(page: &str, from: &str, to: &str) -> String {
    if page == from {
        to.to_string()
    } else if let Some(rest) = page.strip_prefix(&format!("{from}/")) {
        format!("{to}/{rest}")
    } else {
        page.to_string()
    }
}

/// Brings an external edit of `pages` to the Index: the watcher refreshes just
/// those files; a reopen reconciles the whole tree.
fn seen_by(wiki: Wiki, pages: &[&str], how: Seen, cache: &Path) -> Wiki {
    match how {
        Seen::Watcher => {
            let files: Vec<String> = pages.iter().map(|p| format!("{p}.md")).collect();
            let files: Vec<&str> = files.iter().map(String::as_str).collect();
            let ignore = wiki.settings().ignore();
            wiki.index().refresh(wiki.root(), &files, &ignore).unwrap();
            wiki
        }
        Seen::Reopen => {
            let root = wiki.root().to_path_buf();
            drop(wiki);
            Wiki::open_isolated(root, cache).unwrap()
        }
    }
}

fn external_write(root: &Path, page: &str, text: &str) {
    let file = root.join(format!("{page}.md"));
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

// ----------------------------------------------------------------- the run

#[allow(clippy::too_many_lines)] // one flat match over the actions
fn run(initial: &[(&'static str, Content)], actions: &[Action]) -> Result<(), TestCaseError> {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("wiki");
    fs::create_dir(&root).unwrap();
    for (page, content) in initial {
        external_write(&root, page, &content.render(page));
    }
    let kept = dir.path().join("kept");
    let mut wiki = Wiki::open_isolated(&root, &kept).unwrap();
    // Round trips are byte for byte only while every relative Link is in the
    // shortest form; a file renamed behind the Wiki's back keeps stale ones.
    let mut canonical = true;

    for action in actions {
        match action {
            Action::Create(page, content) => {
                let input = json!({ "path": page, "content": content.render(page) });
                mutate(&wiki, "create_page", &input)?;
            }
            Action::Write(page, content) => {
                let input = json!({ "page": page, "content": content.render(page) });
                if applied(&mutate(&wiki, "write_page", &input)?) {
                    check_idempotent(&wiki, "write_page", &input)?;
                }
            }
            Action::Move {
                from,
                to,
                round_trip,
            } => {
                let before = files(wiki.root());
                let broken_before = broken(&wiki);
                let input = json!({ "from": from, "to": to });
                let out = mutate(&wiki, "move_page", &input)?;
                if !applied(&out) {
                    continue;
                }
                check_breaks_warned(&wiki, &broken_before, &out, |page| moved(page, from, to))?;
                // Moving back undoes the move only if `to` was free: a move
                // onto existing Pages merges into them, and one onto the
                // target of a broken Link mends that Link.
                let nested =
                    to.starts_with(&format!("{from}/")) || from.starts_with(&format!("{to}/"));
                let to_was_free = before
                    .keys()
                    .all(|f| f != &format!("{to}.md") && !f.starts_with(&format!("{to}/")))
                    && !broken_before.iter().any(|(_, m)| {
                        m.starts_with(&format!("`{to}`")) || m.starts_with(&format!("`{to}/"))
                    });
                if *round_trip && canonical && !nested && to_was_free {
                    let back = json!({ "from": to, "to": from });
                    let out = mutate(&wiki, "move_page", &back)?;
                    prop_assert!(applied(&out), "moving back failed: {out}");
                    prop_assert_eq!(
                        &files(wiki.root()),
                        &before,
                        "move {}→{}→{} changed files",
                        from,
                        to,
                        from
                    );
                }
            }
            Action::Delete(page, recursive) => {
                let broken_before = broken(&wiki);
                let out = mutate(
                    &wiki,
                    "delete_page",
                    &json!({ "page": page, "recursive": recursive }),
                )?;
                if applied(&out) {
                    check_breaks_warned(&wiki, &broken_before, &out, str::to_string)?;
                }
            }
            Action::Tag(page, tags) => {
                let input = json!({ "page": page, "tags": tags });
                if applied(&mutate(&wiki, "tag_page", &input)?) {
                    check_idempotent(&wiki, "tag_page", &input)?;
                }
            }
            Action::Untag(page, tags) => {
                let input = json!({ "page": page, "tags": tags });
                if applied(&mutate(&wiki, "untag_page", &input)?) {
                    check_idempotent(&wiki, "untag_page", &input)?;
                }
            }
            Action::RenameTag {
                from,
                to,
                round_trip,
            } => {
                let before = files(wiki.root());
                // Renaming onto an existing Tag merges, so only a free `to` can go back.
                let to_was_free = !tags(&wiki)
                    .iter()
                    .any(|t| t == to || t.starts_with(&format!("{to}/")));
                let nested =
                    to.starts_with(&format!("{from}/")) || from.starts_with(&format!("{to}/"));
                let out = mutate(&wiki, "rename_tag", &json!({ "from": from, "to": to }))?;
                if applied(&out) && *round_trip && canonical && to_was_free && !nested {
                    let out = mutate(&wiki, "rename_tag", &json!({ "from": to, "to": from }))?;
                    prop_assert!(applied(&out), "renaming back failed: {out}");
                    prop_assert_eq!(
                        &files(wiki.root()),
                        &before,
                        "rename_tag {}→{}→{} changed files",
                        from,
                        to,
                        from
                    );
                }
            }
            Action::SetMeta(page, status) => {
                let input = json!({ "page": page, "meta": { "status": status } });
                if applied(&mutate(&wiki, "set_page_meta", &input)?) {
                    check_idempotent(&wiki, "set_page_meta", &input)?;
                }
            }
            Action::ExternalWrite(page, content, how) => {
                external_write(wiki.root(), page, &content.render(page));
                wiki = seen_by(wiki, &[page], *how, &kept);
            }
            Action::ExternalDelete(page, how) => {
                let _ = fs::remove_file(wiki.root().join(format!("{page}.md")));
                wiki = seen_by(wiki, &[page], *how, &kept);
            }
            Action::ExternalRename(from, to, how) => {
                let target = wiki.root().join(format!("{to}.md"));
                if !target.exists() {
                    fs::create_dir_all(target.parent().unwrap()).unwrap();
                    let _ = fs::rename(wiki.root().join(format!("{from}.md")), target);
                    canonical = false;
                }
                wiki = seen_by(wiki, &[from, to], *how, &kept);
            }
        }
    }

    let ignore = wiki.settings().ignore();
    wiki.index().reconcile(wiki.root(), &ignore).unwrap();
    let kept = wiki.index().dump().unwrap();
    let fresh = Wiki::open_isolated(wiki.root(), dir.path().join("fresh")).unwrap();
    let rebuilt = fresh.index().dump().unwrap();
    prop_assert_eq!(kept, rebuilt, "incremental Index differs from a rebuild");
    Ok(())
}

/// A rewrite within the filesystem's mtime granularity that keeps the size
/// must still reach the Index at the next open. Restoring the old mtime stands
/// in for a filesystem with coarse timestamps.
#[test]
fn a_same_size_rewrite_in_the_same_mtime_tick_is_reconciled() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("wiki");
    fs::create_dir(&root).unwrap();
    for round in 0..3 {
        let file = root.join("p.md");
        fs::write(&file, format!("# P\n\n[[aa]] {round:02}\n")).unwrap();
        let tick = fs::metadata(&file).unwrap().modified().unwrap();
        let wiki = Wiki::open_isolated(&root, dir.path().join("kept")).unwrap();
        fs::write(&file, format!("# P\n\n[[bb]] {round:02}\n")).unwrap();
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(tick)
            .unwrap();
        drop(wiki);
        let wiki = Wiki::open_isolated(&root, dir.path().join("kept")).unwrap();
        let fresh = Wiki::open_isolated(&root, dir.path().join(format!("fresh{round}"))).unwrap();
        assert_eq!(
            wiki.index().dump().unwrap(),
            fresh.index().dump().unwrap(),
            "round {round}"
        );
    }
}

#[test]
fn relative_links_are_shortest() {
    assert_eq!(relative("a/x/deep", "a"), "../../a.md");
    assert_eq!(relative("a/x/deep", "a/x"), "../x.md");
    assert_eq!(relative("a/y", "a/x"), "x.md");
    assert_eq!(relative("b", "a/x/deep"), "a/x/deep.md");
    assert_eq!(relative("a/y", "b/z"), "../b/z.md");
}

fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(48)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), ..ProptestConfig::default() })]

    #[test]
    fn mutations_and_external_edits_keep_the_plan_and_index_properties(
        initial in prop::collection::vec((page(), content()), 1..6),
        actions in prop::collection::vec(action(), 1..14),
    ) {
        let mut seen = std::collections::BTreeSet::new();
        let initial: Vec<_> = initial.into_iter().filter(|(p, _)| seen.insert(*p)).collect();
        run(&initial, &actions)?;
    }
}
