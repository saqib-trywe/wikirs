//! The Operation catalogue (docs/spec/operations.md). Walking skeleton: `get_page`, `create_page`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Kind, Operation, Result, Wiki,
    error::ChangedFile,
    plan::{Edit, Plan, Splice, Warning, mutate, version_of},
    wiki::{PagePath, check_case_conflict, slugify},
};

crate::operations![GetPage, CreatePage, WritePage, EditPage];

// ------------------------------------------------------------------ get_page

pub struct GetPage;

/// Read one Page.
#[derive(Debug, Deserialize, JsonSchema)]
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
        Ok(GetPageOutput {
            path: page.as_str().to_string(),
            title: title_of(&content).unwrap_or_else(|| page.file_stem().to_string()),
            version: version_of(content.as_bytes()),
            content,
        })
    }
}

/// First level-1 heading, skipping a leading frontmatter block.
/// Walking skeleton: frontmatter `title` comes with the frontmatter slice.
fn title_of(content: &str) -> Option<String> {
    let mut lines = content.lines();
    let mut body: Box<dyn Iterator<Item = &str>> = Box::new(content.lines());
    if content.starts_with("---\n") {
        lines.next();
        if lines.by_ref().any(|l| l == "---") {
            body = Box::new(lines);
        }
    }
    body.find_map(|l| l.strip_prefix("# "))
        .map(|t| t.trim().to_string())
}

// --------------------------------------------------------------- create_page

pub struct CreatePage;

/// Create a Page, either at `path` or under `parent` from a `title`.
#[derive(Debug, Deserialize, JsonSchema)]
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
    /// Initial markdown.
    #[cfg_attr(feature = "clap", arg(long))]
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
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct WritePageInput {
    /// Page Path.
    pub page: String,
    /// The new markdown.
    #[cfg_attr(feature = "clap", arg(long))]
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
pub struct Replacement {
    pub old: String,
    pub new: String,
}

/// Replace exact strings in a Page. All replacements apply, or none do.
#[derive(Debug, Deserialize, JsonSchema)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    fn wiki() -> (tempfile::TempDir, Wiki) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wiki")).unwrap();
        let wiki =
            Wiki::open_with_cache(dir.path().join("wiki"), dir.path().join("cache")).unwrap();
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
        let other = Wiki::open_with_cache(wiki.root(), wiki.cache_dir().parent().unwrap()).unwrap();
        let started = std::time::Instant::now();
        let err = create(&other, "eng/rust", false).unwrap_err();
        assert_eq!(
            (err.kind, err.details["reason"].as_str()),
            (ErrorKind::Conflict, Some("lock_timeout"))
        );
        assert!(started.elapsed() >= std::time::Duration::from_secs(4));
    }

    #[test]
    fn get_page_title_falls_back_to_the_filename() {
        let (_d, wiki) = wiki();
        std::fs::write(
            wiki.root().join("notes.md"),
            "---\ntitle: x\n---\nno heading\n",
        )
        .unwrap();
        let page = GetPage::run(
            &wiki,
            GetPageInput {
                page: "notes".into(),
            },
        )
        .unwrap();
        assert_eq!(page.title, "notes");
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
