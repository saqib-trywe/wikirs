//! Operations that rewrite Links and Tags in place (operations.md): moving
//! and deleting Pages, tagging and untagging, renaming Tags. Each is one Plan
//! of splices, moves and deletes, applied as a unit.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Kind, Operation, Result, Wiki, frontmatter,
    index::{Scope, page_path},
    links::{LinkStatus, TargetKind, normalize_dest, resolve},
    markdown::{self, TagSource, valid_tag},
    plan::{Edit, Plan, Splice, Tx, mutate},
    rewrite::{relative, with_path, written_path},
    wiki::{PagePath, check_case_conflict},
};

/// Every file under `dir` (relative to the root), hidden ones included, so a
/// moved or deleted folder leaves nothing behind.
fn files_under(wiki: &Wiki, dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_string()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(wiki.root().join(&d)) else {
            continue;
        };
        for entry in entries.flatten() {
            let rel = format!("{d}/{}", entry.file_name().to_string_lossy());
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(rel),
                Ok(_) => out.push(rel),
                Err(_) => {}
            }
        }
    }
    out.sort();
    out
}

/// Pages are `.md` files, exactly as the Index classifies them.
fn is_page_file(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|e| e == "md")
}

fn dir_of(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(d, _)| d)
}

/// The file a resolved target lives in.
fn file_of(target: &str, kind: TargetKind) -> String {
    match kind {
        TargetKind::Page => format!("{target}.md"),
        TargetKind::Attachment => target.to_string(),
    }
}

/// How a file is named in Links: Page Path for Pages, path for Attachments.
fn display(file: &str) -> String {
    page_path(file)
}

// ---------------------------------------------------------------- move_page

pub struct MovePage;

/// Move (or rename) a Page, carrying its Child Pages and Attachments, and
/// rewrite every Link to anything moved.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct MovePageInput {
    /// Page Path to move (a Placeholder is fine).
    pub from: String,
    /// New Page Path.
    pub to: String,
    /// Return the Plan without applying it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MovePageOutput {
    pub from: String,
    pub to: String,
    pub plan: Plan,
    pub applied: bool,
    /// Files moved (Pages and Attachments).
    pub moved: usize,
    /// Links rewritten across the Wiki.
    pub links_rewritten: usize,
}

impl Operation for MovePage {
    const NAME: &'static str = "move_page";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str =
        "Move or rename a Page with its Child Pages and Attachments, rewriting every Link to them.";
    type Input = MovePageInput;
    type Output = MovePageOutput;

    fn run(wiki: &Wiki, input: MovePageInput) -> Result<MovePageOutput> {
        let from = PagePath::parse(&input.from)?;
        let to = PagePath::parse(&input.to)?;
        if from == to {
            return Err(Error::invalid_input(
                Some("to"),
                "`to` is the same as `from`",
            ));
        }
        if to.as_str().starts_with(&format!("{}/", from.as_str())) {
            return Err(Error::invalid_path(to.as_str(), "into_own_subtree"));
        }
        let (plan, (moved, links_rewritten)) = mutate(wiki, input.dry_run, |tx| {
            let file_moves = plan_moves(tx, &from, &to)?;
            let destinations: BTreeMap<String, String> = file_moves.iter().cloned().collect();
            let links_rewritten = rewrite_links_for(tx, &destinations)?;
            for (a, b) in &file_moves {
                tx.edit(Edit::Move {
                    from: a.clone(),
                    to: b.clone(),
                });
            }
            Ok((file_moves.len(), links_rewritten))
        })?;
        Ok(MovePageOutput {
            from: from.as_str().to_string(),
            to: to.as_str().to_string(),
            plan,
            applied: !input.dry_run,
            moved,
            links_rewritten,
        })
    }
}

/// `(old file, new file)` for the Page and everything under it; fails before
/// planning anything if any destination is taken.
fn plan_moves(tx: &mut Tx, from: &PagePath, to: &PagePath) -> Result<Vec<(String, String)>> {
    let wiki = tx.wiki().clone();
    if check_case_conflict(&wiki, from).is_err() {
        return Err(Error::not_found("page", from.as_str()));
    }
    let page_file = format!("{}.md", from.as_str());
    let mut moves = Vec::new();
    if tx.stat(&page_file)? {
        moves.push((page_file, format!("{}.md", to.as_str())));
    }
    for file in files_under(&wiki, from.as_str()) {
        let rest = &file[from.as_str().len()..];
        tx.stat(&file)?;
        moves.push((file.clone(), format!("{}{rest}", to.as_str())));
    }
    if moves.is_empty() {
        return Err(Error::not_found("page", from.as_str()));
    }
    check_case_conflict(&wiki, to)?;
    // Moving onto a Placeholder merges folders; any clash fails the whole Plan.
    for (_, dest) in &moves {
        if tx.stat(dest)? {
            return Err(Error::already_exists(&display(dest)));
        }
    }
    Ok(moves)
}

/// Rewrites every Link affected by `moved` (old file → new file): Links to a
/// moved file, and relative Links inside a moved Page. Returns how many.
fn rewrite_links_for(tx: &mut Tx, moved: &BTreeMap<String, String>) -> Result<usize> {
    let wiki = tx.wiki().clone();
    // The Index only says which Pages might need rewriting; each is re-parsed
    // from what's read now, so a stale Index can't produce a wrong splice.
    let mut candidates: BTreeSet<String> =
        moved.keys().filter(|f| is_page_file(f)).cloned().collect();
    {
        let index = wiki.index();
        for old in moved.keys() {
            candidates.extend(index.links_to(&display(old))?.into_iter().map(|l| l.src));
        }
    }
    let mut count = 0;
    for src in candidates {
        let Some(file) = tx.read(&src)? else { continue };
        let new_src = moved.get(&src).unwrap_or(&src).clone();
        let mut splices = Vec::new();
        for link in markdown::parse(&file.content).links {
            let dest = normalize_dest(&src, &link);
            let resolved = resolve(wiki.index().conn(), dest.as_deref(), link.wiki, None)?;
            let old_abs = match (resolved.status, &dest) {
                (LinkStatus::Broken, Some(d)) if !link.wiki => d.clone(),
                (LinkStatus::Broken, _) => continue,
                _ => file_of(&resolved.target, resolved.target_kind),
            };
            let new_abs = moved.get(&old_abs).unwrap_or(&old_abs).clone();
            let Some(current) = written_path(&link.raw, link.wiki) else {
                if new_abs != old_abs || new_src != src {
                    tx.warn(
                        "link_not_rewritten",
                        format!(
                            "{}: reference-style Link `{}` must be updated by hand",
                            display(&src),
                            link.raw
                        ),
                    );
                }
                continue;
            };
            let wanted = if link.wiki {
                if new_abs == old_abs {
                    continue;
                }
                if is_page_file(&current) {
                    new_abs.clone()
                } else {
                    display(&new_abs)
                }
            } else if current.starts_with('/') {
                if new_abs == old_abs {
                    continue;
                }
                format!("/{new_abs}")
            } else {
                relative(dir_of(&new_src), &new_abs)
            };
            if current == wanted {
                continue;
            }
            let Some(new_raw) = with_path(&link.raw, link.wiki, &wanted) else {
                continue;
            };
            splices.push(Splice {
                range: [link.range.start, link.range.end],
                old: link.raw,
                new: new_raw,
            });
        }
        if !splices.is_empty() {
            count += splices.len();
            tx.edit(Edit::Modify {
                path: src,
                base_version: file.version,
                splices,
            });
        }
    }
    Ok(count)
}

// -------------------------------------------------------------- delete_page

pub struct DeletePage;

/// Delete a Page. Its Child Pages stay behind under a Placeholder unless `recursive`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct DeletePageInput {
    /// Page Path.
    pub page: String,
    /// Also delete every Child Page and Attachment under it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub recursive: bool,
    /// Return the Plan without applying it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeletePageOutput {
    pub plan: Plan,
    pub applied: bool,
    /// Files deleted.
    pub deleted: Vec<String>,
}

impl Operation for DeletePage {
    const NAME: &'static str = "delete_page";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str = "Delete a Page (with `recursive`, its whole subtree). The Plan warns about Links that will break.";
    type Input = DeletePageInput;
    type Output = DeletePageOutput;

    fn run(wiki: &Wiki, input: DeletePageInput) -> Result<DeletePageOutput> {
        let page = PagePath::parse(&input.page)?;
        let (plan, deleted) = mutate(wiki, input.dry_run, |tx| {
            let wiki = tx.wiki().clone();
            if check_case_conflict(&wiki, &page).is_err() {
                return Err(Error::not_found("page", page.as_str()));
            }
            let page_file = format!("{}.md", page.as_str());
            let mut files = Vec::new();
            if tx.stat(&page_file)? {
                files.push(page_file);
            }
            let under = files_under(&wiki, page.as_str());
            if input.recursive {
                for f in &under {
                    tx.stat(f)?;
                }
                files.extend(under);
            } else if files.is_empty() && !under.is_empty() {
                return Err(Error::invalid_input(
                    Some("recursive"),
                    format!(
                        "`{}` is a Placeholder; pass `recursive` to delete its subtree",
                        page.as_str()
                    ),
                ));
            }
            if files.is_empty() {
                return Err(Error::not_found("page", page.as_str()));
            }
            warn_breaking_links(tx, &files)?;
            for f in &files {
                tx.edit(Edit::Delete { path: f.clone() });
            }
            Ok(files)
        })?;
        Ok(DeletePageOutput {
            plan,
            applied: !input.dry_run,
            deleted,
        })
    }
}

/// Warns about each Link from a surviving Page to a file about to go.
fn warn_breaking_links(tx: &mut Tx, going: &[String]) -> Result<()> {
    let wiki = tx.wiki().clone();
    let going: BTreeSet<&String> = going.iter().collect();
    let mut warnings = Vec::new();
    {
        let index = wiki.index();
        for file in &going {
            let target = display(file);
            for l in index.links_to(&target)? {
                if going.contains(&l.src) {
                    continue;
                }
                let r = resolve(index.conn(), l.dest.as_deref(), l.wiki, None)?;
                if r.target == target && r.status != LinkStatus::Broken {
                    warnings.push(format!("{} links to {target} ({})", display(&l.src), l.raw));
                }
            }
        }
    }
    for w in warnings {
        tx.warn("link_will_break", w);
    }
    Ok(())
}

// --------------------------------------------------------- tag_page / untag

pub struct TagPage;
pub struct UntagPage;

/// Tags to add to, or remove from, one Page.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct PageTagsInput {
    /// Page Path.
    pub page: String,
    /// Tags, e.g. `lang/rust`.
    #[cfg_attr(feature = "clap", arg(required = true))]
    pub tags: Vec<String>,
    /// Return the Plan without applying it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PageTagsOutput {
    pub path: String,
    pub plan: Plan,
    pub applied: bool,
    /// Tags actually added (or removed); ones already in that state are skipped.
    pub tags: Vec<String>,
}

fn normalized_tags(tags: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t = t.trim().trim_start_matches('#').to_lowercase();
        if !valid_tag(&t) {
            return Err(Error::invalid_input(
                Some("tags"),
                format!("`{t}` is not a valid Tag"),
            ));
        }
        if !out.contains(&t) {
            out.push(t);
        }
    }
    Ok(out)
}

impl Operation for TagPage {
    const NAME: &'static str = "tag_page";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str =
        "Add Tags to a Page's frontmatter (Tags it already carries are skipped).";
    type Input = PageTagsInput;
    type Output = PageTagsOutput;

    fn run(wiki: &Wiki, input: PageTagsInput) -> Result<PageTagsOutput> {
        let page = PagePath::parse(&input.page)?;
        let wanted = normalized_tags(&input.tags)?;
        let (plan, added) = mutate(wiki, input.dry_run, |tx| {
            let (file, content) = read_existing(tx, &page)?;
            let carried: BTreeSet<String> = markdown::parse(&content.content)
                .tags
                .iter()
                .map(|t| t.tag.to_lowercase())
                .collect();
            let added: Vec<String> = wanted
                .into_iter()
                .filter(|t| !carried.contains(t))
                .collect();
            if let Some(s) = frontmatter::edit_tags(&content.content, |cur| {
                cur.iter()
                    .map(|(raw, _)| raw.clone())
                    .chain(added.iter().cloned())
                    .collect()
            }) {
                tx.edit(Edit::Modify {
                    path: file,
                    base_version: content.version,
                    splices: vec![s],
                });
            }
            Ok(added)
        })?;
        Ok(PageTagsOutput {
            path: page.as_str().to_string(),
            plan,
            applied: !input.dry_run,
            tags: added,
        })
    }
}

impl Operation for UntagPage {
    const NAME: &'static str = "untag_page";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str = "Remove Tags from a Page: frontmatter entries go, inline `#tag`s lose their `#` (the word stays).";
    type Input = PageTagsInput;
    type Output = PageTagsOutput;

    fn run(wiki: &Wiki, input: PageTagsInput) -> Result<PageTagsOutput> {
        let page = PagePath::parse(&input.page)?;
        let going: BTreeSet<String> = normalized_tags(&input.tags)?.into_iter().collect();
        let (plan, removed) = mutate(wiki, input.dry_run, |tx| {
            let (file, content) = read_existing(tx, &page)?;
            let mut removed = BTreeSet::new();
            let mut splices = Vec::new();
            if let Some(s) = frontmatter::edit_tags(&content.content, |cur| {
                cur.iter()
                    .filter(|(_, tag)| {
                        let hit = going.contains(&tag.to_lowercase());
                        if hit {
                            removed.insert(tag.to_lowercase());
                        }
                        !hit
                    })
                    .map(|(raw, _)| raw.clone())
                    .collect()
            }) {
                splices.push(s);
            }
            for t in markdown::parse(&content.content).tags {
                if t.source != TagSource::Inline || !going.contains(&t.tag.to_lowercase()) {
                    continue;
                }
                removed.insert(t.tag.to_lowercase());
                match t.range {
                    Some(r) => splices.push(Splice {
                        range: [r.start, r.start + 1],
                        old: "#".into(),
                        new: String::new(),
                    }),
                    None => tx.warn(
                        "tag_not_rewritten",
                        format!("inline #{} could not be located", t.tag),
                    ),
                }
            }
            push_modify(tx, file, content.version, splices);
            Ok(removed.into_iter().collect())
        })?;
        Ok(PageTagsOutput {
            path: page.as_str().to_string(),
            plan,
            applied: !input.dry_run,
            tags: removed,
        })
    }
}

/// Reads an existing Page inside a mutation.
fn read_existing(tx: &mut Tx, page: &PagePath) -> Result<(String, crate::plan::ReadFile)> {
    let wiki = tx.wiki().clone();
    if check_case_conflict(&wiki, page).is_err() {
        return Err(Error::not_found("page", page.as_str()));
    }
    let file = format!("{}.md", page.as_str());
    let content = tx
        .read(&file)?
        .ok_or_else(|| Error::not_found("page", page.as_str()))?;
    Ok((file, content))
}

fn push_modify(tx: &mut Tx, path: String, base_version: String, mut splices: Vec<Splice>) {
    if splices.is_empty() {
        return;
    }
    splices.sort_by_key(|s| s.range[0]);
    tx.edit(Edit::Modify {
        path,
        base_version,
        splices,
    });
}

// ---------------------------------------------------------------- rename_tag

pub struct RenameTag;

/// Rename a Tag and its whole subtree; renaming onto an existing Tag merges them.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct RenameTagInput {
    /// The Tag to rename, e.g. `lang/rust`.
    pub from: String,
    /// Its new name, e.g. `rust`.
    pub to: String,
    /// Return the Plan without applying it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RenameTagOutput {
    pub plan: Plan,
    pub applied: bool,
    /// Pages rewritten.
    pub pages: Vec<String>,
}

impl Operation for RenameTag {
    const NAME: &'static str = "rename_tag";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str = "Rename a Tag and its subtree across the Wiki (frontmatter and inline); onto an existing Tag, this merges.";
    type Input = RenameTagInput;
    type Output = RenameTagOutput;

    fn run(wiki: &Wiki, input: RenameTagInput) -> Result<RenameTagOutput> {
        let [from, to]: [String; 2] = normalized_tags(&[input.from.clone(), input.to.clone()])?
            .try_into()
            .map_err(|_| Error::invalid_input(Some("to"), "`to` is the same as `from`"))?;
        let renamed = |tag: &str| -> Option<String> {
            let tag = tag.to_lowercase();
            if tag == from {
                Some(to.clone())
            } else {
                tag.strip_prefix(&format!("{from}/"))
                    .map(|rest| format!("{to}/{rest}"))
            }
        };
        let (plan, pages) = mutate(wiki, input.dry_run, |tx| {
            let wiki = tx.wiki().clone();
            let pages: BTreeSet<String> = wiki
                .index()
                .page_tags(&Scope::default())?
                .into_iter()
                .filter(|(_, tag)| renamed(tag).is_some())
                .map(|(page, _)| page)
                .collect();
            if pages.is_empty() {
                return Err(Error::not_found("tag", &from));
            }
            for file in &pages {
                let Some(content) = tx.read(file)? else {
                    continue;
                };
                let mut splices = Vec::new();
                if let Some(s) = frontmatter::edit_tags(&content.content, |cur| {
                    let mut seen = BTreeSet::new();
                    cur.iter()
                        .filter_map(|(raw, tag)| {
                            let (raw, key) = match renamed(tag) {
                                Some(new) => (new.clone(), new),
                                None => (raw.clone(), tag.to_lowercase()),
                            };
                            // Renaming onto a Tag the Page already has collapses the duplicate.
                            seen.insert(key).then_some(raw)
                        })
                        .collect()
                }) {
                    splices.push(s);
                }
                for t in markdown::parse(&content.content).tags {
                    let (TagSource::Inline, Some(new)) = (t.source, renamed(&t.tag)) else {
                        continue;
                    };
                    match t.range {
                        Some(r) => splices.push(Splice {
                            range: [r.start + 1, r.end],
                            old: t.tag.clone(),
                            new,
                        }),
                        None => tx.warn(
                            "tag_not_rewritten",
                            format!("inline #{} could not be located", t.tag),
                        ),
                    }
                }
                push_modify(tx, file.clone(), content.version, splices);
            }
            Ok(pages.iter().map(|p| display(p)).collect())
        })?;
        Ok(RenameTagOutput {
            plan,
            applied: !input.dry_run,
            pages,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{ErrorKind, find};

    fn wiki() -> (tempfile::TempDir, Wiki) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wiki")).unwrap();
        let wiki =
            Wiki::open_with_cache(dir.path().join("wiki"), dir.path().join("cache")).unwrap();
        (dir, wiki)
    }

    fn write(wiki: &Wiki, rel: &str, content: &str) {
        let path = wiki.root().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Re-opens so the Index sees files written behind its back.
    fn reopen(dir: &tempfile::TempDir) -> Wiki {
        Wiki::open_with_cache(dir.path().join("wiki"), dir.path().join("cache")).unwrap()
    }

    fn call(wiki: &Wiki, op: &str, input: serde_json::Value) -> Result<serde_json::Value> {
        find(op).unwrap().call(wiki, input)
    }

    /// Every file in the Wiki (hidden excluded) with its bytes.
    fn snapshot(wiki: &Wiki) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut stack = vec![wiki.root().to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if e.file_type().unwrap().is_dir() {
                    stack.push(e.path());
                } else {
                    let rel = e
                        .path()
                        .strip_prefix(wiki.root())
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.insert(rel, std::fs::read(e.path()).unwrap());
                }
            }
        }
        out
    }

    fn fixture(wiki: &Wiki) {
        write(
            wiki,
            "eng/rust.md",
            "---\ntags: [lang/rust]\n---\n# Rust\n\n[a](rust/async.md) [[eng/rust/async#X|x]] ![d](rust/d.png) [n](../notes.md) #lang/rust/unsafe\n",
        );
        write(
            wiki,
            "eng/rust/async.md",
            "# Async\n\n## X\n\n[up](../rust.md) [[notes]] #lang/rust/async\n",
        );
        write(wiki, "eng/rust/d.png", "png");
        write(
            wiki,
            "notes.md",
            "# Notes\n\n[[eng/rust]] [[Eng/Rust|c]] [r](eng/rust.md) ![](eng/rust/d.png) [x](<eng/rust/async.md>)\n",
        );
    }

    #[test]
    fn move_then_move_back_restores_every_byte() {
        let (dir, wiki) = wiki();
        fixture(&wiki);
        let wiki = reopen(&dir);
        let before = snapshot(&wiki);
        call(
            &wiki,
            "move_page",
            serde_json::json!({ "from": "eng/rust", "to": "lang/my rust" }),
        )
        .unwrap();
        let moved = snapshot(&wiki);
        assert!(
            moved.contains_key("lang/my rust/d.png") && !moved.keys().any(|k| k.starts_with("eng"))
        );
        assert!(String::from_utf8_lossy(&moved["notes.md"]).contains("[r](lang/my%20rust.md)"));
        call(
            &wiki,
            "move_page",
            serde_json::json!({ "from": "lang/my rust", "to": "eng/rust" }),
        )
        .unwrap();
        let mut after = snapshot(&wiki);
        // The case-fallback Link was corrected on the way; everything else is byte-identical.
        let notes = String::from_utf8(after.remove("notes.md").unwrap()).unwrap();
        assert_eq!(
            notes,
            "# Notes\n\n[[eng/rust]] [[eng/rust|c]] [r](eng/rust.md) ![](eng/rust/d.png) [x](<eng/rust/async.md>)\n"
        );
        let mut expected = before;
        expected.remove("notes.md");
        assert_eq!(after, expected);
    }

    #[test]
    fn a_dry_run_is_pure_and_matches_the_applied_plan() {
        let (dir, wiki) = wiki();
        fixture(&wiki);
        let wiki = reopen(&dir);
        let before = snapshot(&wiki);
        let dry = call(
            &wiki,
            "move_page",
            serde_json::json!({ "from": "eng/rust", "to": "lang/rust", "dry_run": true }),
        )
        .unwrap();
        assert_eq!(snapshot(&wiki), before, "a dry run writes nothing");
        let real = call(
            &wiki,
            "move_page",
            serde_json::json!({ "from": "eng/rust", "to": "lang/rust" }),
        )
        .unwrap();
        assert_eq!(dry["result"]["plan"], real["result"]["plan"]);
        let check = call(
            &wiki,
            "check",
            serde_json::json!({ "kinds": ["broken_link"] }),
        )
        .unwrap();
        assert_eq!(
            check["result"]["diagnostics"],
            serde_json::json!([]),
            "no Link broke"
        );
    }

    #[test]
    fn move_edge_cases() {
        let (dir, wiki) = wiki();
        fixture(&wiki);
        write(&wiki, "lang/rust/async.md", "# Other\n");
        write(&wiki, "lang/go.md", "# Go\n");
        let wiki = reopen(&dir);
        let kind = |from: &str, to: &str| {
            call(
                &wiki,
                "move_page",
                serde_json::json!({ "from": from, "to": to }),
            )
            .unwrap_err()
            .kind
        };
        assert_eq!(kind("eng/rust", "eng/rust/deeper"), ErrorKind::InvalidPath);
        assert_eq!(
            kind("eng/rust", "lang/rust"),
            ErrorKind::AlreadyExists,
            "a child path clashes in the merge"
        );
        assert_eq!(kind("eng/rust", "lang/go"), ErrorKind::AlreadyExists);
        assert_eq!(kind("eng/nope", "x"), ErrorKind::NotFound);
        assert_eq!(
            kind("Eng/rust", "x"),
            ErrorKind::NotFound,
            "Page Paths are case-sensitive"
        );
        assert_eq!(kind("eng/rust", "Lang/x"), ErrorKind::CaseConflict);
        // Moving a Placeholder (no eng.md) carries its folder.
        call(
            &wiki,
            "move_page",
            serde_json::json!({ "from": "eng", "to": "engineering" }),
        )
        .unwrap();
        assert!(
            wiki.root().join("engineering/rust/async.md").exists()
                && !wiki.root().join("eng").exists()
        );
    }

    #[test]
    fn delete_leaves_a_placeholder_unless_recursive() {
        let (dir, wiki) = wiki();
        fixture(&wiki);
        let wiki = reopen(&dir);
        let out = call(
            &wiki,
            "delete_page",
            serde_json::json!({ "page": "eng/rust" }),
        )
        .unwrap();
        assert!(
            !wiki.root().join("eng/rust.md").exists()
                && wiki.root().join("eng/rust/async.md").exists()
        );
        let warnings = out["result"]["plan"]["warnings"].as_array().unwrap();
        assert_eq!(
            warnings.len(),
            4,
            "every surviving Link to it: {warnings:?}"
        );
        assert_eq!(
            call(
                &wiki,
                "delete_page",
                serde_json::json!({ "page": "eng/rust" })
            )
            .unwrap_err()
            .kind,
            ErrorKind::InvalidInput
        );
        call(
            &wiki,
            "delete_page",
            serde_json::json!({ "page": "eng/rust", "recursive": true }),
        )
        .unwrap();
        assert!(
            !wiki.root().join("eng").exists(),
            "emptied folders are removed"
        );
    }

    #[test]
    fn tags_are_added_removed_and_renamed_in_place() {
        let (dir, wiki) = wiki();
        fixture(&wiki);
        let wiki = reopen(&dir);
        let page = |rel: &str| std::fs::read_to_string(wiki.root().join(rel)).unwrap();
        let out = call(
            &wiki,
            "tag_page",
            serde_json::json!({ "page": "eng/rust", "tags": ["LANG/rust", "#new/tag"] }),
        )
        .unwrap();
        assert_eq!(
            out["result"]["tags"],
            serde_json::json!(["new/tag"]),
            "carried Tags are skipped, input is normalised"
        );
        assert!(page("eng/rust.md").starts_with("---\ntags: [lang/rust, new/tag]\n---\n"));
        assert_eq!(
            call(
                &wiki,
                "tag_page",
                serde_json::json!({ "page": "eng/rust", "tags": ["2026"] })
            )
            .unwrap_err()
            .kind,
            ErrorKind::InvalidInput
        );

        call(
            &wiki,
            "rename_tag",
            serde_json::json!({ "from": "lang/rust", "to": "new" }),
        )
        .unwrap();
        assert!(page("eng/rust.md").starts_with("---\ntags: [new, new/tag]\n---\n"));
        assert!(
            page("eng/rust.md").ends_with("#new/unsafe\n")
                && page("eng/rust/async.md").ends_with("#new/async\n")
        );
        call(
            &wiki,
            "rename_tag",
            serde_json::json!({ "from": "new/tag", "to": "new" }),
        )
        .unwrap();
        assert!(
            page("eng/rust.md").starts_with("---\ntags: [new]\n---\n"),
            "renaming onto an existing Tag merges"
        );
        assert_eq!(
            call(
                &wiki,
                "rename_tag",
                serde_json::json!({ "from": "nope", "to": "x" })
            )
            .unwrap_err()
            .kind,
            ErrorKind::NotFound
        );

        let out = call(
            &wiki,
            "untag_page",
            serde_json::json!({ "page": "eng/rust", "tags": ["new", "new/unsafe"] }),
        )
        .unwrap();
        assert_eq!(
            out["result"]["tags"],
            serde_json::json!(["new", "new/unsafe"])
        );
        assert!(
            page("eng/rust.md").starts_with("# Rust\n")
                && page("eng/rust.md").ends_with(" new/unsafe\n"),
            "{}",
            page("eng/rust.md")
        );
    }
}
