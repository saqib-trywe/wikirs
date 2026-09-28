//! The Operation catalogue (docs/spec/operations.md). Walking skeleton: `get_page`, `create_page`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Kind, Operation, Result, Wiki,
    plan::{Edit, Plan, Warning},
    wiki::{PagePath, check_case_conflict, slugify},
};

crate::operations![GetPage, CreatePage];

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

fn version_of(bytes: &[u8]) -> String {
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(bytes))
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

        // Case first: on case-insensitive filesystems (macOS, Windows) a case-only
        // clash would otherwise look like the exact path already existing.
        check_case_conflict(wiki, &page)?;
        if wiki.page_file(&page).exists() {
            return Err(Error::already_exists(page.as_str()));
        }

        let plan = Plan {
            edits: vec![Edit::Create {
                path: format!("{}.md", page.as_str()),
                content,
            }],
            warnings: Vec::new(),
        };
        if !input.dry_run {
            plan.apply(wiki)?;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    fn wiki() -> (tempfile::TempDir, Wiki) {
        let dir = tempfile::tempdir().unwrap();
        let wiki = Wiki::open(dir.path()).unwrap();
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
