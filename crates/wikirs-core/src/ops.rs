//! The Operation catalogue (docs/spec/operations.md). Walking skeleton: `get_page`, `create_page`.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Kind, Operation, Result, Wiki,
    config::{GetConfig, Init, SetConfig},
    error::ChangedFile,
    hierarchy::{Children, ListSpaces, ReorderPage},
    index::{Filter, Hit, PageRow, Scope, Skipped, Sort, page_path},
    links::{LinkStatus, Resolved, normalize_dest, resolve},
    markdown,
    mutations::{DeletePage, MovePage, RenameTag, SetPageMeta, TagPage, UntagPage},
    plan::{Edit, Plan, Splice, UnrecoveredEdit, Warning, mutate, unrecovered_edits, version_of},
    wiki::{PagePath, check_case_conflict, slugify},
};

crate::operations![
    Init,
    GetConfig,
    SetConfig,
    GetPage,
    ListPages,
    Children,
    ListSpaces,
    Links,
    Backlinks,
    ResolveLink,
    Outline,
    TagTree,
    Check,
    CreatePage,
    WritePage,
    EditPage,
    SetPageMeta,
    MovePage,
    DeletePage,
    ReorderPage,
    TagPage,
    UntagPage,
    RenameTag,
    Search,
    IndexStatus,
    RebuildIndex,
];

// ------------------------------------------------------------------ get_page

pub struct GetPage;

/// Read one Page.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct GetPageInput {
    /// Page Path, e.g. `eng/rust/async-notes`.
    pub page: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GetPageOutput {
    pub path: String,
    pub title: String,
    /// Raw markdown, exactly as stored.
    pub content: String,
    /// The frontmatter as JSON, or null if the Page has none.
    pub frontmatter: Option<serde_json::Value>,
    /// The Page's Tags (frontmatter and inline), lowercase, deduplicated.
    pub tags: Vec<String>,
    /// Opaque content hash; pass it back as `base_version` when writing.
    pub version: String,
}

impl Operation for GetPage {
    const NAME: &'static str = "get_page";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str = "Read a Page: its raw markdown, Title and version.";
    type Input = GetPageInput;
    type Output = GetPageOutput;

    fn run(wiki: &Wiki, input: GetPageInput) -> Result<GetPageOutput> {
        let page = PagePath::parse(&input.page)?;
        // Page Paths are case-sensitive even where the filesystem isn't.
        if check_case_conflict(wiki, &page).is_err() {
            return Err(Error::not_found("page", page.as_str()));
        }
        let file = wiki.page_file(&page);
        let bytes = match std::fs::read(&file) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::not_found("page", page.as_str()));
            }
            Err(e) => return Err(Error::io(Some(page.as_str()), &e)),
        };
        let content = String::from_utf8(bytes).map_err(|_| {
            Error::invalid_input(Some("page"), format!("`{}` is not UTF-8", page.as_str()))
        })?;
        let parsed = markdown::parse(&content);
        let tags: BTreeSet<String> = parsed.tags.iter().map(|t| t.tag.to_lowercase()).collect();
        Ok(GetPageOutput {
            path: page.as_str().to_string(),
            title: parsed.title.unwrap_or_else(|| page.file_stem().to_string()),
            frontmatter: parsed.frontmatter,
            tags: tags.into_iter().collect(),
            version: version_of(content.as_bytes()),
            content,
        })
    }
}

// --------------------------------------------------------------- create_page

pub struct CreatePage;

/// Create a Page, either at `path` or under `parent` from a `title`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct CreatePageInput {
    /// Page Path to create. Give either this or `title`.
    #[cfg_attr(feature = "clap", arg(long))]
    pub path: Option<String>,
    /// Parent Page Path for a `title`-based create (Wiki root if absent).
    #[cfg_attr(feature = "clap", arg(long))]
    pub parent: Option<String>,
    /// Title: slugified into the filename and written as the H1.
    #[cfg_attr(feature = "clap", arg(long))]
    pub title: Option<String>,
    /// Initial markdown. (May start with `---` frontmatter.)
    #[cfg_attr(feature = "clap", arg(long, allow_hyphen_values = true))]
    pub content: Option<String>,
    /// Return the Plan without applying it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CreatePageOutput {
    /// The Page Path that was (or would be) created.
    pub path: String,
    pub plan: Plan,
    pub applied: bool,
}

impl Operation for CreatePage {
    const NAME: &'static str = "create_page";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str = "Create a Page at a path, or under a parent from a Title.";
    type Input = CreatePageInput;
    type Output = CreatePageOutput;

    fn run(wiki: &Wiki, input: CreatePageInput) -> Result<CreatePageOutput> {
        let (page, content) = match (&input.path, &input.title) {
            (Some(path), None) => {
                if input.parent.is_some() {
                    return Err(Error::invalid_input(
                        Some("parent"),
                        "`parent` only goes with `title`",
                    ));
                }
                (
                    PagePath::parse(path)?,
                    input.content.clone().unwrap_or_default(),
                )
            }
            (None, Some(title)) => {
                let slug = slugify(title);
                if slug.is_empty() {
                    return Err(Error::invalid_input(
                        Some("title"),
                        "the title has no letters or digits",
                    ));
                }
                let raw = match &input.parent {
                    Some(parent) => format!("{}/{slug}", PagePath::parse(parent)?.as_str()),
                    None => slug,
                };
                let content = match &input.content {
                    Some(body) => format!("# {title}\n\n{body}"),
                    None => format!("# {title}\n"),
                };
                (PagePath::parse(&raw)?, content)
            }
            _ => {
                return Err(Error::invalid_input(
                    None,
                    "give exactly one of `path` or `title`",
                ));
            }
        };

        let (plan, ()) = mutate(wiki, input.dry_run, |tx| {
            // Case first: on case-insensitive filesystems (macOS, Windows) a case-only
            // clash would otherwise look like the exact path already existing.
            check_case_conflict(wiki, &page)?;
            let file = format!("{}.md", page.as_str());
            if tx.read(&file)?.is_some() {
                return Err(Error::already_exists(page.as_str()));
            }
            tx.edit(Edit::Create {
                path: file,
                content,
            });
            Ok(())
        })?;
        Ok(CreatePageOutput {
            path: page.as_str().to_string(),
            plan,
            applied: !input.dry_run,
        })
    }

    fn warnings(output: &CreatePageOutput) -> Vec<Warning> {
        output.plan.warnings.clone()
    }
}

// ---------------------------------------------------------------- write_page

pub struct WritePage;

/// Replace a Page's whole content.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct WritePageInput {
    /// Page Path.
    pub page: String,
    /// The new markdown. (May start with `---` frontmatter.)
    #[cfg_attr(feature = "clap", arg(long, allow_hyphen_values = true))]
    pub content: String,
    /// The `version` you read; if the Page changed since, nothing is written (`conflict`).
    #[cfg_attr(feature = "clap", arg(long))]
    pub base_version: Option<String>,
    /// Return the Plan without applying it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub dry_run: bool,
}

/// Result of a content write: the Plan and the Page's version afterwards.
#[derive(Debug, Serialize, JsonSchema)]
pub struct WriteOutput {
    pub path: String,
    pub plan: Plan,
    pub applied: bool,
    /// The Page's version once the Plan is applied.
    pub version: String,
}

impl Operation for WritePage {
    const NAME: &'static str = "write_page";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str =
        "Replace a Page's content. Pass `base_version` to refuse if it changed since you read it.";
    type Input = WritePageInput;
    type Output = WriteOutput;

    fn run(wiki: &Wiki, input: WritePageInput) -> Result<WriteOutput> {
        let page = PagePath::parse(&input.page)?;
        let content = input.content;
        let (plan, version) = mutate(wiki, input.dry_run, |tx| {
            let (file, current) = read_page(wiki, tx, &page, input.base_version.as_deref())?;
            if current.content != content {
                tx.edit(Edit::Modify {
                    path: file,
                    base_version: current.version,
                    splices: vec![Splice {
                        range: [0, current.content.len()],
                        old: current.content,
                        new: content.clone(),
                    }],
                });
            }
            Ok(version_of(content.as_bytes()))
        })?;
        Ok(WriteOutput {
            path: page.as_str().to_string(),
            plan,
            applied: !input.dry_run,
            version,
        })
    }

    fn warnings(output: &WriteOutput) -> Vec<Warning> {
        output.plan.warnings.clone()
    }
}

/// Reads an existing Page inside a mutation, enforcing `base_version`.
fn read_page(
    wiki: &Wiki,
    tx: &mut crate::plan::Tx,
    page: &PagePath,
    base_version: Option<&str>,
) -> Result<(String, crate::plan::ReadFile)> {
    if check_case_conflict(wiki, page).is_err() {
        return Err(Error::not_found("page", page.as_str()));
    }
    let file = format!("{}.md", page.as_str());
    let current = tx
        .read(&file)?
        .ok_or_else(|| Error::not_found("page", page.as_str()))?;
    if let Some(base) = base_version
        && base != current.version
    {
        return Err(Error::conflict_changed(&[ChangedFile {
            path: file,
            expected: Some(base.to_string()),
            actual: Some(current.version),
        }]));
    }
    Ok((file, current))
}

// ----------------------------------------------------------------- edit_page

pub struct EditPage;

/// One exact-string replacement: `old` must occur exactly once in the Page.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Replacement {
    pub old: String,
    pub new: String,
}

/// Replace exact strings in a Page. All replacements apply, or none do.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct EditPageInput {
    /// Page Path.
    pub page: String,
    /// Replacements, each matched against the Page as it is now.
    #[cfg_attr(
        feature = "clap",
        arg(long = "edit", value_name = r#"{"old":…,"new":…}"#, value_parser = parse_replacement, required = true)
    )]
    pub edits: Vec<Replacement>,
    /// The `version` you read; if the Page changed since, nothing is written (`conflict`).
    #[cfg_attr(feature = "clap", arg(long))]
    pub base_version: Option<String>,
    /// Return the Plan without applying it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub dry_run: bool,
}

#[cfg(feature = "clap")]
fn parse_replacement(raw: &str) -> std::result::Result<Replacement, String> {
    serde_json::from_str(raw).map_err(|e| format!("expected {{\"old\": …, \"new\": …}}: {e}"))
}

impl Operation for EditPage {
    const NAME: &'static str = "edit_page";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str =
        "Replace exact strings in a Page; each `old` must match exactly once.";
    type Input = EditPageInput;
    type Output = WriteOutput;

    fn run(wiki: &Wiki, input: EditPageInput) -> Result<WriteOutput> {
        let page = PagePath::parse(&input.page)?;
        if input.edits.is_empty() {
            return Err(Error::invalid_input(
                Some("edits"),
                "give at least one replacement",
            ));
        }
        let (plan, version) = mutate(wiki, input.dry_run, |tx| {
            let (file, current) = read_page(wiki, tx, &page, input.base_version.as_deref())?;
            let splices = plan_replacements(&current.content, &input.edits)?;
            let after = crate::plan::splice(&current.content, &splices);
            tx.edit(Edit::Modify {
                path: file,
                base_version: current.version,
                splices,
            });
            Ok(version_of(after.as_bytes()))
        })?;
        Ok(WriteOutput {
            path: page.as_str().to_string(),
            plan,
            applied: !input.dry_run,
            version,
        })
    }

    fn warnings(output: &WriteOutput) -> Vec<Warning> {
        output.plan.warnings.clone()
    }
}

/// Finds each `old` exactly once in `content`, as ordered, non-overlapping splices.
fn plan_replacements(content: &str, edits: &[Replacement]) -> Result<Vec<Splice>> {
    let mut splices = Vec::with_capacity(edits.len());
    for (i, edit) in edits.iter().enumerate() {
        if edit.old.is_empty() {
            return Err(Error::invalid_input(
                Some("edits"),
                format!("edit {i}: `old` is empty"),
            ));
        }
        let hits: Vec<usize> = content.match_indices(&edit.old).map(|(at, _)| at).collect();
        match hits.as_slice() {
            [] => return Err(Error::match_count(i, 0)),
            [at] => splices.push(Splice {
                range: [*at, at + edit.old.len()],
                old: edit.old.clone(),
                new: edit.new.clone(),
            }),
            many => return Err(Error::match_count(i, many.len())),
        }
    }
    splices.sort_by_key(|s| s.range[0]);
    if splices.windows(2).any(|w| w[0].range[1] > w[1].range[0]) {
        return Err(Error::invalid_input(Some("edits"), "replacements overlap"));
    }
    Ok(splices)
}

// ---------------------------------------------------------------- list_pages

pub struct ListPages;

/// List Pages, optionally filtered, sorted and paged.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct ListPagesInput {
    #[serde(default)]
    #[cfg_attr(feature = "clap", command(flatten))]
    pub filter: Filter,
    /// `path` (default), `title` or `modified` (newest first).
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long, value_enum, default_value_t))]
    pub sort: Sort,
    /// At most this many (default 100).
    #[cfg_attr(feature = "clap", arg(long))]
    pub limit: Option<u32>,
    /// Skip this many first.
    #[cfg_attr(feature = "clap", arg(long))]
    pub offset: Option<u32>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListPagesOutput {
    pub pages: Vec<PageRow>,
    /// Matching Pages before `limit` / `offset`.
    pub total: i64,
}

const DEFAULT_LIMIT: u32 = 100;

impl Operation for ListPages {
    const NAME: &'static str = "list_pages";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str =
        "List Pages, filtered by Space or path prefix, sorted and paged.";
    type Input = ListPagesInput;
    type Output = ListPagesOutput;

    fn run(wiki: &Wiki, input: ListPagesInput) -> Result<ListPagesOutput> {
        let (pages, total) = wiki.index().pages(
            &input.filter,
            input.sort,
            i64::from(input.limit.unwrap_or(DEFAULT_LIMIT)),
            i64::from(input.offset.unwrap_or(0)),
        )?;
        Ok(ListPagesOutput { pages, total })
    }
}

// -------------------------------------------------------------------- search

pub struct Search;

/// Full-text search over Pages.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct SearchInput {
    /// Plain terms (all must match), `"quoted phrases"`, and `prefix*`.
    #[cfg_attr(feature = "clap", arg(allow_hyphen_values = true))]
    pub text: String,
    #[serde(default)]
    #[cfg_attr(feature = "clap", command(flatten))]
    pub filter: Filter,
    /// At most this many (default 100).
    #[cfg_attr(feature = "clap", arg(long))]
    pub limit: Option<u32>,
    /// Skip this many first.
    #[cfg_attr(feature = "clap", arg(long))]
    pub offset: Option<u32>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchOutput {
    pub hits: Vec<Hit>,
    /// Matching Pages before `limit` / `offset`.
    pub total: i64,
}

impl Operation for Search {
    const NAME: &'static str = "search";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str =
        "Full-text search: plain terms, \"quoted phrases\" and prefix* (all must match).";
    type Input = SearchInput;
    type Output = SearchOutput;

    fn run(wiki: &Wiki, input: SearchInput) -> Result<SearchOutput> {
        let query = fts_query(&input.text)?;
        let (hits, total) = wiki.index().search(
            &query,
            &input.filter,
            i64::from(input.limit.unwrap_or(DEFAULT_LIMIT)),
            i64::from(input.offset.unwrap_or(0)),
        )?;
        Ok(SearchOutput { hits, total })
    }
}

/// Plain terms and `"phrases"` → an FTS5 query where every term is quoted, so
/// FTS5's own syntax (`AND`, `NEAR`, `col:`…) is never exposed.
fn fts_query(text: &str) -> Result<String> {
    let mut terms = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('"') {
            let end = after.find('"').unwrap_or(after.len());
            let phrase = after[..end].trim();
            if !phrase.is_empty() {
                terms.push(format!("\"{}\"", phrase.replace('"', "")));
            }
            rest = after.get(end + 1..).unwrap_or("").trim_start();
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let word = &rest[..end];
            let (word, prefix) = match word.strip_suffix('*') {
                Some(w) => (w, true),
                None => (word, false),
            };
            let word = word.replace('"', "");
            if !word.is_empty() {
                terms.push(format!("\"{word}\"{}", if prefix { "*" } else { "" }));
            }
            rest = rest[end..].trim_start();
        }
    }
    if terms.is_empty() {
        return Err(Error::invalid_input(Some("text"), "search text is empty"));
    }
    Ok(terms.join(" "))
}

// -------------------------------------------------------------- index_status

pub struct IndexStatus;

/// Report on the Index.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct IndexStatusInput {}

#[derive(Debug, Serialize, JsonSchema)]
pub struct IndexStatusOutput {
    /// The Wiki root this process resolved.
    pub root: String,
    /// Per-Wiki cache dir (Index, write lock, journal).
    pub cache_dir: String,
    pub pages: i64,
    pub attachments: i64,
    /// Links written in Pages (resolved or not).
    pub links: i64,
    /// Distinct Tags.
    pub tags: i64,
    /// When the Index last changed, milliseconds since the Unix epoch.
    pub last_updated: Option<i64>,
    /// Whether the Index may be behind the files (always false right after open).
    pub stale: bool,
    /// File watching in this process: `none` (one-shot) until watchers exist.
    pub watcher: String,
    pub skipped: Vec<Skipped>,
    pub unrecovered_edits: Vec<UnrecoveredEdit>,
}

impl Operation for IndexStatus {
    const NAME: &'static str = "index_status";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str =
        "Report on the Index: counts, freshness, skipped files, unrecovered edits.";
    type Input = IndexStatusInput;
    type Output = IndexStatusOutput;

    fn run(wiki: &Wiki, _input: IndexStatusInput) -> Result<IndexStatusOutput> {
        status(wiki)
    }
}

fn status(wiki: &Wiki) -> Result<IndexStatusOutput> {
    let index = wiki.index();
    Ok(IndexStatusOutput {
        root: wiki.root().display().to_string(),
        cache_dir: wiki.cache_dir().display().to_string(),
        pages: index.count("page")?,
        attachments: index.count("attachment")?,
        links: index.count_links()?,
        tags: index.count_tags()?,
        last_updated: index.last_updated()?,
        stale: false,
        watcher: "none".into(),
        skipped: index.skipped()?,
        unrecovered_edits: unrecovered_edits(wiki),
    })
}

// ------------------------------------------------------------- rebuild_index

pub struct RebuildIndex;

/// Drop the Index and rebuild it from the files.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct RebuildIndexInput {}

impl Operation for RebuildIndex {
    const NAME: &'static str = "rebuild_index";
    const KIND: Kind = Kind::Maintenance;
    const DESCRIPTION: &'static str =
        "Drop the Index and rebuild it from the files. Changes no Wiki file.";
    type Input = RebuildIndexInput;
    type Output = IndexStatusOutput;

    fn run(wiki: &Wiki, _input: RebuildIndexInput) -> Result<IndexStatusOutput> {
        let ignore = wiki.settings().ignore();
        wiki.index().rebuild(wiki.root(), &ignore)?;
        status(wiki)
    }
}

// --------------------------------------------------------------------- links

pub struct Links;

/// The Links a Page makes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct LinksInput {
    /// Page Path.
    pub page: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LinkOut {
    /// The Link's source text.
    pub raw: String,
    /// Byte range `[start, end)` in the Page.
    pub range: [i64; 2],
    #[serde(flatten)]
    pub resolved: Resolved,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LinksOutput {
    pub links: Vec<LinkOut>,
}

impl Operation for Links {
    const NAME: &'static str = "links";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str =
        "The Links a Page makes, each resolved: ok, broken, heading_missing or case_fallback.";
    type Input = LinksInput;
    type Output = LinksOutput;

    fn run(wiki: &Wiki, input: LinksInput) -> Result<LinksOutput> {
        let page = existing_page(wiki, &input.page)?;
        let file = format!("{}.md", page.as_str());
        let index = wiki.index();
        let scope = Scope {
            space: None,
            path_prefix: Some(file.clone()),
        };
        let links = index
            .stored_links(&scope)?
            .into_iter()
            .filter(|l| l.src == file)
            .map(|l| {
                Ok(LinkOut {
                    resolved: resolve(
                        index.conn(),
                        l.dest.as_deref(),
                        l.wiki,
                        l.heading.as_deref(),
                    )?,
                    raw: l.raw,
                    range: l.range,
                })
            })
            .collect::<Result<_>>()?;
        Ok(LinksOutput { links })
    }
}

/// A Page that must exist (exact case), for read Operations.
fn existing_page(wiki: &Wiki, raw: &str) -> Result<PagePath> {
    let page = PagePath::parse(raw)?;
    if check_case_conflict(wiki, &page).is_err() || !wiki.page_file(&page).is_file() {
        return Err(Error::not_found("page", page.as_str()));
    }
    Ok(page)
}

// ----------------------------------------------------------------- backlinks

pub struct Backlinks;

/// The Links pointing at a Page or Attachment.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct BacklinksInput {
    /// Page Path or Attachment path. A missing target is fine: Broken Links point at it.
    pub target: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Backlink {
    /// The linking Page.
    pub from: String,
    pub range: [i64; 2],
    pub raw: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BacklinksOutput {
    pub backlinks: Vec<Backlink>,
}

impl Operation for Backlinks {
    const NAME: &'static str = "backlinks";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str =
        "The Links pointing at a Page or Attachment (including Broken ones).";
    type Input = BacklinksInput;
    type Output = BacklinksOutput;

    fn run(wiki: &Wiki, input: BacklinksInput) -> Result<BacklinksOutput> {
        let target = input.target.trim().trim_start_matches('/').to_string();
        let index = wiki.index();
        let mut backlinks = Vec::new();
        for l in index.links_to(&target)? {
            let resolved = resolve(index.conn(), l.dest.as_deref(), l.wiki, None)?;
            if resolved.target == target {
                backlinks.push(Backlink {
                    from: page_path(&l.src),
                    range: l.range,
                    raw: l.raw,
                });
            }
        }
        Ok(BacklinksOutput { backlinks })
    }
}

// -------------------------------------------------------------- resolve_link

pub struct ResolveLink;

/// Resolve one Link as it would be written in a Page.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct ResolveLinkInput {
    /// The Page the Link is written in (relative Links resolve from here).
    pub from_page: String,
    /// The Link's source, e.g. `[[eng/rust#Pinning]]` or `[x](../a.md)`.
    pub raw: String,
}

impl Operation for ResolveLink {
    const NAME: &'static str = "resolve_link";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str = "Resolve a Link as written in a Page: its target and status.";
    type Input = ResolveLinkInput;
    type Output = Resolved;

    fn run(wiki: &Wiki, input: ResolveLinkInput) -> Result<Resolved> {
        let from = PagePath::parse(&input.from_page)?;
        let parsed = markdown::parse(&input.raw);
        let [link] = parsed.links.as_slice() else {
            return Err(Error::invalid_input(Some("raw"), "not exactly one Link"));
        };
        let dest = normalize_dest(&format!("{}.md", from.as_str()), link);
        resolve(
            wiki.index().conn(),
            dest.as_deref(),
            link.wiki,
            link.heading.as_deref(),
        )
    }
}

// ------------------------------------------------------------------- outline

pub struct Outline;

/// A Page's headings.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct OutlineInput {
    /// Page Path.
    pub page: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct OutlineHeading {
    pub level: u8,
    pub text: String,
    /// Use as a Link's `#heading`.
    pub anchor: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct OutlineOutput {
    pub headings: Vec<OutlineHeading>,
}

impl Operation for Outline {
    const NAME: &'static str = "outline";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str = "A Page's headings, with their level and anchor.";
    type Input = OutlineInput;
    type Output = OutlineOutput;

    fn run(wiki: &Wiki, input: OutlineInput) -> Result<OutlineOutput> {
        let page = GetPage::run(wiki, GetPageInput { page: input.page })?;
        let headings = markdown::parse(&page.content)
            .headings
            .into_iter()
            .map(|h| OutlineHeading {
                level: h.level,
                text: h.text,
                anchor: h.anchor,
            })
            .collect();
        Ok(OutlineOutput { headings })
    }
}

// ------------------------------------------------------------------ tag_tree

pub struct TagTree;

/// The Tag hierarchy with Page counts.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct TagTreeInput {
    /// Only Pages in this part of the Wiki.
    #[serde(default)]
    #[cfg_attr(feature = "clap", command(flatten))]
    pub scope: Scope,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TagNode {
    /// Full Tag, lowercase (e.g. `lang/rust`).
    pub tag: String,
    /// Pages carrying exactly this Tag.
    pub direct: usize,
    /// Pages carrying this Tag or any descendant.
    pub inclusive: usize,
    pub children: Vec<TagNode>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TagTreeOutput {
    pub tags: Vec<TagNode>,
}

impl Operation for TagTree {
    const NAME: &'static str = "tag_tree";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str = "The Tag hierarchy with direct and inclusive Page counts.";
    type Input = TagTreeInput;
    type Output = TagTreeOutput;

    fn run(wiki: &Wiki, input: TagTreeInput) -> Result<TagTreeOutput> {
        let pairs = wiki.index().page_tags(&input.scope)?;
        // Every Tag and each of its ancestors, with the Pages under it.
        let mut direct: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut inclusive: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (page, tag) in &pairs {
            direct.entry(tag.clone()).or_default().insert(page.clone());
            let segs: Vec<&str> = tag.split('/').collect();
            for i in 1..=segs.len() {
                inclusive
                    .entry(segs[..i].join("/"))
                    .or_default()
                    .insert(page.clone());
            }
        }
        Ok(TagTreeOutput {
            tags: tag_nodes(None, &inclusive, &direct),
        })
    }
}

/// Tag nodes under `parent` (top level when `None`), with direct and inclusive counts.
fn tag_nodes(
    parent: Option<&str>,
    inclusive: &BTreeMap<String, BTreeSet<String>>,
    direct: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<TagNode> {
    inclusive
        .iter()
        .filter(|(tag, _)| tag.rsplit_once('/').map(|(p, _)| p) == parent)
        .map(|(tag, pages)| TagNode {
            tag: tag.clone(),
            direct: direct.get(tag).map_or(0, BTreeSet::len),
            inclusive: pages.len(),
            children: tag_nodes(Some(tag), inclusive, direct),
        })
        .collect()
}

// --------------------------------------------------------------------- check

pub struct Check;

/// Every diagnostic in the Wiki, or a part of it.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct CheckInput {
    /// Only Pages in this part of the Wiki.
    #[serde(default)]
    #[cfg_attr(feature = "clap", command(flatten))]
    pub scope: Scope,
    /// Only these kinds (default: all).
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long = "kind", value_enum))]
    pub kinds: Vec<DiagnosticKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum DiagnosticKind {
    BrokenLink,
    HeadingMissing,
    CaseFallback,
    MixedCaseTag,
    UnrecoveredEdit,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    /// The Page (or file) it's about.
    pub page: String,
    /// Byte range in the Page, when there is one.
    pub range: Option<[i64; 2]>,
    pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CheckOutput {
    pub diagnostics: Vec<Diagnostic>,
}

impl Operation for Check {
    const NAME: &'static str = "check";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str = "Diagnostics: Broken Links, missing headings, case fallbacks, mixed-case Tags, unrecovered edits.";
    type Input = CheckInput;
    type Output = CheckOutput;

    fn run(wiki: &Wiki, input: CheckInput) -> Result<CheckOutput> {
        let wanted = |k: DiagnosticKind| input.kinds.is_empty() || input.kinds.contains(&k);
        let index = wiki.index();
        let mut diagnostics = Vec::new();
        for l in index.stored_links(&input.scope)? {
            let r = resolve(
                index.conn(),
                l.dest.as_deref(),
                l.wiki,
                l.heading.as_deref(),
            )?;
            let (kind, message) = match r.status {
                LinkStatus::Ok => continue,
                LinkStatus::Broken => (
                    DiagnosticKind::BrokenLink,
                    format!("`{}` does not exist", r.target),
                ),
                LinkStatus::HeadingMissing => (
                    DiagnosticKind::HeadingMissing,
                    format!(
                        "`{}` has no heading `{}`",
                        r.target,
                        r.heading.unwrap_or_default()
                    ),
                ),
                LinkStatus::CaseFallback => (
                    DiagnosticKind::CaseFallback,
                    format!("resolves to `{}` only ignoring case", r.target),
                ),
            };
            if wanted(kind) {
                diagnostics.push(Diagnostic {
                    kind,
                    page: page_path(&l.src),
                    range: Some(l.range),
                    message,
                });
            }
        }
        if wanted(DiagnosticKind::MixedCaseTag) {
            for (page, raw, range) in index.mixed_case_tags(&input.scope)? {
                diagnostics.push(Diagnostic {
                    kind: DiagnosticKind::MixedCaseTag,
                    page: page_path(&page),
                    range,
                    message: format!(
                        "Tag `{raw}` is written in mixed case; wikirs writes `{}`",
                        raw.to_lowercase()
                    ),
                });
            }
        }
        drop(index);
        if wanted(DiagnosticKind::UnrecoveredEdit) {
            for edit in unrecovered_edits(wiki) {
                diagnostics.push(Diagnostic {
                    kind: DiagnosticKind::UnrecoveredEdit,
                    page: page_path(&edit.path),
                    range: None,
                    message: "an edit from a crashed Plan was left alone because the file changed"
                        .into(),
                });
            }
        }
        Ok(CheckOutput { diagnostics })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    fn wiki() -> (tempfile::TempDir, Wiki) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wiki")).unwrap();
        let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("cache")).unwrap();
        (dir, wiki)
    }

    fn create(wiki: &Wiki, path: &str, dry_run: bool) -> Result<CreatePageOutput> {
        CreatePage::run(
            wiki,
            CreatePageInput {
                path: Some(path.into()),
                parent: None,
                title: None,
                content: Some("# Hi\n".into()),
                dry_run,
            },
        )
    }

    #[test]
    fn dry_run_writes_nothing_and_returns_the_same_plan() {
        let (_d, wiki) = wiki();
        let dry = create(&wiki, "eng/rust", true).unwrap();
        assert!(!wiki.root().join("eng/rust.md").exists());
        let real = create(&wiki, "eng/rust", false).unwrap();
        assert_eq!(dry.plan, real.plan);
        assert_eq!(
            std::fs::read_to_string(wiki.root().join("eng/rust.md")).unwrap(),
            "# Hi\n"
        );
    }

    #[test]
    fn title_create_slugifies_and_writes_the_h1() {
        let (_d, wiki) = wiki();
        let out = CreatePage::run(
            &wiki,
            CreatePageInput {
                path: None,
                parent: Some("eng".into()),
                title: Some("Async Notes, Part 2".into()),
                content: None,
                dry_run: false,
            },
        )
        .unwrap();
        assert_eq!(out.path, "eng/async-notes-part-2");
        let page = GetPage::run(&wiki, GetPageInput { page: out.path }).unwrap();
        assert_eq!(page.title, "Async Notes, Part 2");
    }

    #[test]
    fn existing_and_case_clashing_paths_are_rejected() {
        let (_d, wiki) = wiki();
        create(&wiki, "eng/rust", false).unwrap();
        assert_eq!(
            create(&wiki, "eng/rust", false).unwrap_err().kind,
            ErrorKind::AlreadyExists
        );
        assert_eq!(
            create(&wiki, "eng/Rust", false).unwrap_err().kind,
            ErrorKind::CaseConflict
        );
        assert_eq!(
            create(&wiki, "Eng/other", false).unwrap_err().kind,
            ErrorKind::CaseConflict
        );
    }

    fn write(wiki: &Wiki, content: &str, base: Option<&str>) -> Result<WriteOutput> {
        WritePage::run(
            wiki,
            WritePageInput {
                page: "eng/rust".into(),
                content: content.into(),
                base_version: base.map(Into::into),
                dry_run: false,
            },
        )
    }

    fn edit(wiki: &Wiki, pairs: &[(&str, &str)], dry_run: bool) -> Result<WriteOutput> {
        EditPage::run(
            wiki,
            EditPageInput {
                page: "eng/rust".into(),
                edits: pairs
                    .iter()
                    .map(|(o, n)| Replacement {
                        old: (*o).into(),
                        new: (*n).into(),
                    })
                    .collect(),
                base_version: None,
                dry_run,
            },
        )
    }

    fn read(wiki: &Wiki) -> String {
        std::fs::read_to_string(wiki.root().join("eng/rust.md")).unwrap()
    }

    #[test]
    fn write_page_refuses_a_stale_base_version() {
        let (_d, wiki) = wiki();
        create(&wiki, "eng/rust", false).unwrap();
        let v1 = GetPage::run(
            &wiki,
            GetPageInput {
                page: "eng/rust".into(),
            },
        )
        .unwrap()
        .version;
        let out = write(&wiki, "# Two\n", Some(&v1)).unwrap();
        assert_eq!(read(&wiki), "# Two\n");
        assert_eq!(
            out.version,
            GetPage::run(
                &wiki,
                GetPageInput {
                    page: "eng/rust".into()
                }
            )
            .unwrap()
            .version
        );
        let err = write(&wiki, "# Three\n", Some(&v1)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(err.details["reason"], "changed");
        assert_eq!(read(&wiki), "# Two\n", "a conflict writes nothing");
        assert_eq!(
            write(&wiki, "# Two\n", None).unwrap().plan.edits.len(),
            0,
            "no-op write has an empty Plan"
        );
    }

    #[test]
    fn edit_page_replaces_exact_strings_all_or_nothing() {
        let (_d, wiki) = wiki();
        CreatePage::run(
            &wiki,
            CreatePageInput {
                path: Some("eng/rust".into()),
                parent: None,
                title: None,
                content: Some("a b c b\n".into()),
                dry_run: false,
            },
        )
        .unwrap();
        assert_eq!(
            edit(&wiki, &[("c", "C"), ("a", "A")], false)
                .unwrap()
                .plan
                .edits
                .len(),
            1
        );
        assert_eq!(read(&wiki), "A b C b\n");
        assert_eq!(
            edit(&wiki, &[("A", "x"), ("zzz", "y")], false)
                .unwrap_err()
                .kind,
            ErrorKind::NoMatch
        );
        let err = edit(&wiki, &[("b", "B")], false).unwrap_err();
        assert_eq!(
            (err.kind, err.details["count"].as_u64()),
            (ErrorKind::AmbiguousMatch, Some(2))
        );
        assert_eq!(
            edit(&wiki, &[("A b", "1"), ("b C", "2")], false)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidInput
        );
        edit(&wiki, &[("A", "Z")], true).unwrap();
        assert_eq!(
            read(&wiki),
            "A b C b\n",
            "failed and dry-run edits write nothing"
        );
    }

    #[test]
    fn a_held_write_lock_times_out_as_conflict() {
        let (_d, wiki) = wiki();
        let _held = wiki.lock_writes().unwrap();
        let other = Wiki::open_isolated(wiki.root(), wiki.cache_dir().parent().unwrap()).unwrap();
        let started = std::time::Instant::now();
        let err = create(&other, "eng/rust", false).unwrap_err();
        assert_eq!(
            (err.kind, err.details["reason"].as_str()),
            (ErrorKind::Conflict, Some("lock_timeout"))
        );
        assert!(started.elapsed() >= std::time::Duration::from_secs(4));
    }

    #[test]
    fn search_text_never_exposes_fts_syntax() {
        assert_eq!(fts_query("tokio  pinning").unwrap(), r#""tokio" "pinning""#);
        assert_eq!(
            fts_query(r#""hard part" pin*"#).unwrap(),
            r#""hard part" "pin"*"#
        );
        assert_eq!(
            fts_query(r#"NEAR(a b) col:x "unclosed"#).unwrap(),
            r#""NEAR(a" "b)" "col:x" "unclosed""#
        );
        assert_eq!(
            fts_query("  \"\"  ").unwrap_err().kind,
            ErrorKind::InvalidInput
        );
    }

    #[test]
    fn an_unchanged_wiki_is_not_rewritten_on_open() {
        let (dir, wiki) = wiki();
        create(&wiki, "eng/rust", false).unwrap();
        let before = wiki.index().last_updated().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let again = Wiki::open_isolated(wiki.root(), dir.path().join("cache")).unwrap();
        assert_eq!(again.index().last_updated().unwrap(), before);
    }

    /// testing.md: after any mix of mutations and external edits, the
    /// incrementally maintained Index equals a fresh rebuild.
    #[test]
    fn incremental_index_equals_a_rebuild() {
        let (dir, wiki) = wiki();
        let reopen = || Wiki::open_isolated(wiki.root(), dir.path().join("cache")).unwrap();
        let root = wiki.root().to_path_buf();
        create(&wiki, "eng/rust", false).unwrap();
        create(&wiki, "eng/rust/async", false).unwrap();
        write(&wiki, "# Rust\n\nOwnership and borrowing.\n", None).unwrap();
        std::fs::write(root.join("eng/rust/async.md"), "# Async\n\nedited in vim\n").unwrap();
        std::fs::create_dir_all(root.join("personal")).unwrap();
        std::fs::write(root.join("personal/bread.md"), "flour\n").unwrap();
        std::fs::write(root.join("eng/diagram.png"), [0u8, 1, 2]).unwrap();
        std::fs::write(root.join("bad.md"), [0xffu8, 0xfe]).unwrap();
        let _ = reopen();
        std::fs::remove_file(root.join("personal/bread.md")).unwrap();
        edit(&wiki, &[("Ownership", "Lifetimes")], false).unwrap();
        let incremental = reopen().index().dump().unwrap();

        let rebuilt = reopen();
        rebuilt
            .index()
            .rebuild(&root, &crate::settings::Ignore::default())
            .unwrap();
        assert_eq!(incremental, rebuilt.index().dump().unwrap());
        assert!(
            incremental.iter().any(|r| r.contains("Lifetimes")),
            "the edit is indexed"
        );
        assert!(
            incremental.iter().any(|r| r.contains("edited in vim")),
            "the external edit is indexed"
        );
        assert!(
            !incremental.iter().any(|r| r.contains("bread")),
            "the external delete is indexed"
        );
    }

    #[test]
    fn list_and_search_honour_the_space_filter() {
        let (_d, wiki) = wiki();
        for path in ["eng", "eng/rust", "engine", "personal/rust"] {
            create(&wiki, path, false).unwrap();
        }
        let eng = Filter {
            space: Some("eng".into()),
            ..Filter::default()
        };
        let listed = ListPages::run(
            &wiki,
            ListPagesInput {
                filter: eng.clone(),
                sort: Sort::Path,
                limit: None,
                offset: None,
            },
        )
        .unwrap();
        let paths: Vec<_> = listed.pages.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(
            paths,
            ["eng", "eng/rust"],
            "the home Page and its subtree, not `engine`"
        );
        let found = Search::run(
            &wiki,
            SearchInput {
                text: "hi".into(),
                filter: eng,
                limit: Some(1),
                offset: None,
            },
        )
        .unwrap();
        assert_eq!(
            (found.hits.len(), found.total),
            (1, 2),
            "limit pages the hits, total doesn't"
        );
    }

    #[test]
    fn case_fallback_needs_exactly_one_case_insensitive_match() {
        let (dir, wiki) = wiki();
        std::fs::create_dir_all(wiki.root().join("eng")).unwrap();
        std::fs::write(wiki.root().join("eng/notes.md"), "# Notes\n").unwrap();
        let reopen = || Wiki::open_isolated(wiki.root(), dir.path().join("cache")).unwrap();
        let status = |wiki: &Wiki, raw: &str| {
            let input = ResolveLinkInput {
                from_page: "index".into(),
                raw: raw.into(),
            };
            ResolveLink::run(wiki, input).unwrap().status
        };
        assert_eq!(status(&reopen(), "[[eng/Notes]]"), LinkStatus::CaseFallback);
        // A second file matching ignoring case (an extensionless Attachment) makes it ambiguous.
        std::fs::write(wiki.root().join("eng/NOTES"), "x").unwrap();
        assert_eq!(status(&reopen(), "[[eng/Notes]]"), LinkStatus::Broken);
        assert_eq!(
            status(&reopen(), "[[eng/notes]]"),
            LinkStatus::Ok,
            "an exact match still wins"
        );
    }

    #[test]
    fn unknown_input_fields_are_rejected_not_ignored() {
        let (_d, wiki) = wiki();
        let call = |op: &str, input: serde_json::Value| crate::find(op).unwrap().call(&wiki, input);
        for (op, input) in [
            ("list_pages", serde_json::json!({ "tag": "lang" })),
            (
                "list_pages",
                serde_json::json!({ "filter": { "tags": "lang" } }),
            ),
            ("tag_tree", serde_json::json!({ "space": "eng" })),
            (
                "edit_page",
                serde_json::json!({ "page": "x", "edits": [{ "old": "a", "new": "b", "all": true }] }),
            ),
        ] {
            assert_eq!(
                call(op, input.clone()).unwrap_err().kind,
                ErrorKind::InvalidInput,
                "{op} {input}"
            );
        }
        assert!(
            call(
                "tag_tree",
                serde_json::json!({ "scope": { "space": "eng" } })
            )
            .is_ok()
        );
    }

    #[test]
    fn get_page_title_is_frontmatter_then_h1_then_filename() {
        let (_d, wiki) = wiki();
        let title = |content: &str| {
            std::fs::write(wiki.root().join("notes.md"), content).unwrap();
            GetPage::run(
                &wiki,
                GetPageInput {
                    page: "notes".into(),
                },
            )
            .unwrap()
            .title
        };
        assert_eq!(title("---\ntitle: From FM\n---\n# From H1\n"), "From FM");
        assert_eq!(title("# From H1\n"), "From H1");
        assert_eq!(title("no heading\n"), "notes");
        assert_eq!(
            GetPage::run(
                &wiki,
                GetPageInput {
                    page: "missing".into()
                }
            )
            .unwrap_err()
            .kind,
            ErrorKind::NotFound
        );
        assert_eq!(
            GetPage::run(
                &wiki,
                GetPageInput {
                    page: "Notes".into()
                }
            )
            .unwrap_err()
            .kind,
            ErrorKind::NotFound
        );
    }
}
