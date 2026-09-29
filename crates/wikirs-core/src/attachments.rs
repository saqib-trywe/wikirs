//! Attachment Operations (operations.md#attachments). An Attachment is any
//! file that isn't a Page, hidden or ignored; new ones go in their Page's own
//! folder (`eng/rust.md` → `eng/rust/diagram.png`).

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Kind, Operation, Result, Wiki,
    index::{AttachmentRow, Scope},
    mutations::{rewrite_links_for, warn_breaking_links},
    plan::{Blob, Edit, Plan, Warning, mutate, version_of_file},
    wiki::{AttachmentPath, PagePath, check_attachment_case, check_case_conflict},
};

/// An existing Attachment at exactly `path` (case included), or `NotFound`.
fn existing(wiki: &Wiki, path: &AttachmentPath) -> Result<()> {
    let missing = || Error::not_found("attachment", path.as_str());
    if check_attachment_case(wiki, path).is_err()
        || !wiki.root().join(path.as_str()).is_file()
        || wiki.settings().ignore().matches_path(path.as_str())
    {
        return Err(missing());
    }
    Ok(())
}

fn no_local_fs(field: &str) -> Error {
    Error::invalid_input(
        Some(field),
        "this Interface has no access to local files: `local_path` is unavailable",
    )
}

// ------------------------------------------------------------ add_attachment

pub struct AddAttachment;

/// Where the new Attachment's content comes from: exactly one of the two.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
#[cfg_attr(feature = "clap", group(required = true, multiple = false))]
pub struct AttachmentSource {
    /// A file on this machine to copy in (Interfaces with local file access only).
    #[cfg_attr(feature = "clap", arg(long))]
    pub local_path: Option<String>,
    /// The content, base64-encoded.
    #[cfg_attr(feature = "clap", arg(long))]
    pub base64: Option<String>,
}

/// Add an Attachment to a Page's folder; a taken name gets a numeric suffix.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct AddAttachmentInput {
    /// Page Path of the Page it belongs to.
    pub page: String,
    /// File name, e.g. `diagram.png`.
    pub name: String,
    #[cfg_attr(feature = "clap", command(flatten))]
    pub source: AttachmentSource,
    /// Return the Plan without applying it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AddAttachmentOutput {
    /// The final path, from the Wiki root.
    pub path: String,
    pub plan: Plan,
    pub applied: bool,
    pub size: u64,
    pub version: String,
}

/// `diagram.png` → `diagram-2.png`; `notes` → `notes-2`.
fn with_suffix(name: &str, n: usize) -> String {
    match name.rsplit_once('.') {
        // Hidden names never get here, so the stem isn't empty.
        Some((stem, ext)) => format!("{stem}-{n}.{ext}"),
        _ => format!("{name}-{n}"),
    }
}

fn check_name(name: &str) -> Result<()> {
    let bad = |reason: &str| Err(Error::invalid_input(Some("name"), reason.to_string()));
    if name.contains(['/', '\\']) {
        return bad("a file name, without folders");
    }
    // Hidden names, `..` and `.md` are rejected as paths.
    AttachmentPath::parse(name)
        .map(|_| ())
        .or_else(|e| bad(&e.message))
}

fn blob_of(wiki: &Wiki, source: AttachmentSource) -> Result<Blob> {
    let bad = |reason: String| Error::invalid_input(Some("source"), reason);
    match (source.local_path, source.base64) {
        (Some(path), None) => {
            if !wiki.local_fs() {
                return Err(no_local_fs("source"));
            }
            let path = PathBuf::from(path);
            if !path.is_file() {
                return Err(bad(format!("`{}` is not a file", path.display())));
            }
            Ok(Blob::File(path))
        }
        (None, Some(text)) => STANDARD
            .decode(text.trim())
            .map(Blob::Bytes)
            .map_err(|e| bad(format!("not valid base64: {e}"))),
        _ => Err(bad("give exactly one of `local_path` or `base64`".into())),
    }
}

impl Operation for AddAttachment {
    const NAME: &'static str = "add_attachment";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str = "Add an Attachment to a Page's folder, from a local file or base64. A taken name gets a numeric suffix.";
    type Input = AddAttachmentInput;
    type Output = AddAttachmentOutput;

    fn run(wiki: &Wiki, input: AddAttachmentInput) -> Result<AddAttachmentOutput> {
        let page = PagePath::parse(&input.page)?;
        check_name(&input.name)?;
        let blob = blob_of(wiki, input.source)?;
        let (plan, (path, size, version)) = mutate(wiki, input.dry_run, |tx| {
            let missing = || Error::not_found("page", page.as_str());
            if check_case_conflict(tx.wiki(), &page).is_err()
                || !tx.stat(&format!("{}.md", page.as_str()))?
            {
                return Err(missing());
            }
            // Names in the folder, ignoring case: a case variant is taken too.
            let taken: BTreeSet<String> = std::fs::read_dir(tx.wiki().root().join(page.as_str()))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_lowercase())
                .collect();
            // With `n` names taken, one of the first `n + 1` candidates is free.
            let name = (1..=taken.len() + 1)
                .map(|n| {
                    if n == 1 {
                        input.name.clone()
                    } else {
                        with_suffix(&input.name, n)
                    }
                })
                .find(|n| !taken.contains(&n.to_lowercase()))
                .expect("pigeonhole: a candidate is free");
            let path = format!("{}/{name}", page.as_str());
            if name != input.name {
                tx.warn(
                    "renamed",
                    format!("`{}` is taken, so it's saved as `{name}`", input.name),
                );
            }
            if tx.wiki().settings().ignore().matches_path(&path) {
                tx.warn(
                    "ignored",
                    format!("`{path}` matches `ignore`, so it won't be listed or linked"),
                );
            }
            // Records "absent", so a file appearing before apply is a Conflict.
            tx.stat(&path)?;
            let (size, version) = tx.create_binary(&path, blob)?;
            Ok((path, size, version))
        })?;
        Ok(AddAttachmentOutput {
            path,
            plan,
            applied: !input.dry_run,
            size,
            version,
        })
    }

    fn warnings(output: &AddAttachmentOutput) -> Vec<Warning> {
        output.plan.warnings.clone()
    }
}

// ---------------------------------------------------------- list_attachments

pub struct ListAttachments;

/// List Attachments: one Page's own, or everything in a part of the Wiki.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct ListAttachmentsInput {
    /// Only this Page's own Attachments (its folder, not its Child Pages').
    #[cfg_attr(feature = "clap", arg(long))]
    pub page: Option<String>,
    /// Everything in this part of the Wiki (not with `page`).
    #[serde(default)]
    #[cfg_attr(feature = "clap", command(flatten))]
    pub scope: Scope,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListAttachmentsOutput {
    pub attachments: Vec<AttachmentRow>,
}

impl Operation for ListAttachments {
    const NAME: &'static str = "list_attachments";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str =
        "List Attachments: a Page's own, or all of them in a Space, path prefix or the Wiki.";
    type Input = ListAttachmentsInput;
    type Output = ListAttachmentsOutput;

    fn run(wiki: &Wiki, input: ListAttachmentsInput) -> Result<ListAttachmentsOutput> {
        let Some(raw) = input.page else {
            return Ok(ListAttachmentsOutput {
                attachments: wiki.index().attachments(&input.scope)?,
            });
        };
        if input.scope.space.is_some() || input.scope.path_prefix.is_some() {
            return Err(Error::invalid_input(
                Some("page"),
                "give `page` or a scope, not both",
            ));
        }
        let page = PagePath::parse(&raw)?;
        // A Placeholder's folder can hold Attachments too.
        if check_case_conflict(wiki, &page).is_err()
            || !(wiki.page_file(&page).is_file() || wiki.root().join(page.as_str()).is_dir())
        {
            return Err(Error::not_found("page", page.as_str()));
        }
        let folder = format!("{}/", page.as_str());
        let scope = Scope {
            space: None,
            path_prefix: Some(folder.clone()),
        };
        let attachments = wiki
            .index()
            .attachments(&scope)?
            .into_iter()
            .filter(|a| {
                a.path
                    .strip_prefix(&folder)
                    .is_some_and(|rest| !rest.contains('/'))
            })
            .collect();
        Ok(ListAttachmentsOutput { attachments })
    }
}

// ----------------------------------------------------------- read_attachment

pub struct ReadAttachment;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum ReadAs {
    /// The content, base64-encoded.
    #[default]
    Bytes,
    /// The file's absolute path (Interfaces with local file access only).
    LocalPath,
}

/// Read an Attachment's content, or where it is on disk.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct ReadAttachmentInput {
    /// Path from the Wiki root, e.g. `eng/rust/diagram.png`.
    pub path: String,
    /// `bytes` (default) or `local_path`.
    #[serde(default, rename = "as")]
    #[cfg_attr(feature = "clap", arg(long = "as", value_enum, default_value_t))]
    pub read_as: ReadAs,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ReadAttachmentOutput {
    pub path: String,
    pub size: u64,
    pub version: String,
    /// With `as: bytes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
    /// With `as: local_path`: the absolute path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
}

impl Operation for ReadAttachment {
    const NAME: &'static str = "read_attachment";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str =
        "Read an Attachment: its content as base64, or its absolute local path.";
    type Input = ReadAttachmentInput;
    type Output = ReadAttachmentOutput;

    fn run(wiki: &Wiki, input: ReadAttachmentInput) -> Result<ReadAttachmentOutput> {
        let path = AttachmentPath::parse(&input.path)?;
        if input.read_as == ReadAs::LocalPath && !wiki.local_fs() {
            return Err(no_local_fs("as"));
        }
        existing(wiki, &path)?;
        let file = wiki.root().join(path.as_str());
        let io = |e: &std::io::Error| Error::io(Some(path.as_str()), e);
        let (base64, local_path, version, size) = match input.read_as {
            ReadAs::Bytes => {
                let bytes = std::fs::read(&file).map_err(|e| io(&e))?;
                let version = crate::plan::version_of(&bytes);
                (
                    Some(STANDARD.encode(&bytes)),
                    None,
                    version,
                    bytes.len() as u64,
                )
            }
            ReadAs::LocalPath => {
                let (version, size) = version_of_file(&file).map_err(|e| io(&e))?;
                (None, Some(file.display().to_string()), version, size)
            }
        };
        Ok(ReadAttachmentOutput {
            path: path.as_str().to_string(),
            size,
            version,
            base64,
            local_path,
        })
    }
}

// ----------------------------------------------------------- move_attachment

pub struct MoveAttachment;

/// Move or rename an Attachment, rewriting every Link to it.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct MoveAttachmentInput {
    /// Current path, e.g. `eng/rust/diagram.png`.
    pub from: String,
    /// New path.
    pub to: String,
    /// Return the Plan without applying it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MoveAttachmentOutput {
    pub from: String,
    pub to: String,
    pub plan: Plan,
    pub applied: bool,
    /// Links rewritten across the Wiki.
    pub links_rewritten: usize,
}

impl Operation for MoveAttachment {
    const NAME: &'static str = "move_attachment";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str = "Move or rename an Attachment, rewriting every Link to it.";
    type Input = MoveAttachmentInput;
    type Output = MoveAttachmentOutput;

    fn run(wiki: &Wiki, input: MoveAttachmentInput) -> Result<MoveAttachmentOutput> {
        let from = AttachmentPath::parse(&input.from)?;
        let to = AttachmentPath::parse(&input.to)?;
        if from == to {
            return Err(Error::invalid_input(
                Some("to"),
                "`to` is the same as `from`",
            ));
        }
        let (plan, links_rewritten) = mutate(wiki, input.dry_run, |tx| {
            existing(tx.wiki(), &from)?;
            check_attachment_case(tx.wiki(), &to)?;
            if tx.wiki().root().join(to.as_str()).is_dir() || tx.stat(to.as_str())? {
                return Err(Error::already_exists(to.as_str()));
            }
            tx.stat(from.as_str())?;
            let moved = BTreeMap::from([(from.as_str().to_string(), to.as_str().to_string())]);
            let links_rewritten = rewrite_links_for(tx, &moved)?;
            tx.edit(Edit::Move {
                from: from.as_str().to_string(),
                to: to.as_str().to_string(),
            });
            Ok(links_rewritten)
        })?;
        Ok(MoveAttachmentOutput {
            from: from.as_str().to_string(),
            to: to.as_str().to_string(),
            plan,
            applied: !input.dry_run,
            links_rewritten,
        })
    }

    fn warnings(output: &MoveAttachmentOutput) -> Vec<Warning> {
        output.plan.warnings.clone()
    }
}

// --------------------------------------------------------- delete_attachment

pub struct DeleteAttachment;

/// Delete an Attachment. The Plan warns about Links that will break.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct DeleteAttachmentInput {
    /// Path from the Wiki root.
    pub path: String,
    /// Return the Plan without applying it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeleteAttachmentOutput {
    pub path: String,
    pub plan: Plan,
    pub applied: bool,
}

impl Operation for DeleteAttachment {
    const NAME: &'static str = "delete_attachment";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str =
        "Delete an Attachment. The Plan warns about Links that will break.";
    type Input = DeleteAttachmentInput;
    type Output = DeleteAttachmentOutput;

    fn run(wiki: &Wiki, input: DeleteAttachmentInput) -> Result<DeleteAttachmentOutput> {
        let path = AttachmentPath::parse(&input.path)?;
        let (plan, ()) = mutate(wiki, input.dry_run, |tx| {
            existing(tx.wiki(), &path)?;
            tx.stat(path.as_str())?;
            warn_breaking_links(tx, &[path.as_str().to_string()])?;
            tx.edit(Edit::Delete {
                path: path.as_str().to_string(),
            });
            Ok(())
        })?;
        Ok(DeleteAttachmentOutput {
            path: path.as_str().to_string(),
            plan,
            applied: !input.dry_run,
        })
    }

    fn warnings(output: &DeleteAttachmentOutput) -> Vec<Warning> {
        output.plan.warnings.clone()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::{ErrorKind, find};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\xff\xfe";

    fn wiki() -> (tempfile::TempDir, Wiki) {
        let dir = tempfile::tempdir().unwrap();
        let files: &[(&str, &[u8])] = &[
            (
                "eng/rust.md",
                b"# Rust\n\n![d](rust/d.png) [spec](rust/spec.pdf)\n",
            ),
            ("eng/rust/d.png", PNG),
            ("eng/rust/async.md", b"# Async\n"),
            ("eng/rust/async/deep.png", b"deep"),
            ("eng/ideas/loose.txt", b"loose"),
            ("notes.md", b"# Notes\n\n![[eng/rust/d.png]]\n"),
        ];
        for (rel, content) in files {
            let p = dir.path().join("wiki").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
        let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("base")).unwrap();
        (dir, wiki)
    }

    fn call(wiki: &Wiki, op: &str, input: Value) -> Result<Value> {
        find(op).unwrap().call(wiki, input)
    }

    fn add(wiki: &Wiki, page: &str, name: &str, bytes: &[u8]) -> Result<Value> {
        call(
            wiki,
            "add_attachment",
            json!({ "page": page, "name": name, "source": { "base64": STANDARD.encode(bytes) } }),
        )
    }

    fn listed(wiki: &Wiki, input: Value) -> Vec<String> {
        call(wiki, "list_attachments", input).unwrap()["result"]["attachments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["path"].as_str().unwrap().to_string())
            .collect()
    }

    fn read(wiki: &Wiki, rel: &str) -> Vec<u8> {
        std::fs::read(wiki.root().join(rel)).unwrap()
    }

    #[test]
    fn added_bytes_land_in_the_page_folder_exactly() {
        let (_dir, wiki) = wiki();
        let dry = call(
            &wiki,
            "add_attachment",
            json!({ "page": "notes", "name": "logo.png",
                    "source": { "base64": STANDARD.encode(PNG) }, "dry_run": true }),
        )
        .unwrap();
        assert!(
            !wiki.root().join("notes").exists(),
            "a dry run writes nothing"
        );
        let done = add(&wiki, "notes", "logo.png", PNG).unwrap();
        assert_eq!(dry["result"]["plan"], done["result"]["plan"]);
        assert_eq!(
            done["result"]["plan"]["edits"],
            json!([{ "op": "create_binary", "path": "notes/logo.png",
                     "size": PNG.len(), "version": crate::plan::version_of(PNG) }])
        );
        assert_eq!(read(&wiki, "notes/logo.png"), PNG);
        assert!(
            !wiki.cache_dir().join("staged").exists(),
            "staging is cleared"
        );

        let got = call(
            &wiki,
            "read_attachment",
            json!({ "path": "notes/logo.png" }),
        )
        .unwrap();
        assert_eq!(
            STANDARD
                .decode(got["result"]["base64"].as_str().unwrap())
                .unwrap(),
            PNG
        );
        assert_eq!(got["result"]["version"], done["result"]["version"]);
        let local = call(
            &wiki,
            "read_attachment",
            json!({ "path": "notes/logo.png", "as": "local_path" }),
        )
        .unwrap();
        assert_eq!(
            local["result"]["local_path"],
            wiki.root().join("notes/logo.png").display().to_string()
        );
        assert_eq!(local["result"]["version"], done["result"]["version"]);
        assert_eq!(
            listed(&wiki, json!({ "page": "notes" })),
            ["notes/logo.png"]
        );
    }

    #[test]
    fn a_taken_name_gets_a_numeric_suffix() {
        let (_dir, wiki) = wiki();
        let out = add(&wiki, "eng/rust", "D.png", b"new").unwrap();
        assert_eq!(
            out["result"]["path"], "eng/rust/D-2.png",
            "a case variant is taken"
        );
        assert_eq!(out["warnings"][0]["kind"], "renamed");
        let out = add(&wiki, "eng/rust", "d.png", b"newer").unwrap();
        assert_eq!(out["result"]["path"], "eng/rust/d-3.png");
        let out = add(&wiki, "eng/rust", "async", b"x").unwrap();
        assert_eq!(
            out["result"]["path"], "eng/rust/async-2",
            "a Child Page's folder is taken"
        );
        assert_eq!(read(&wiki, "eng/rust/d.png"), PNG, "never overwritten");
        let out = add(&wiki, "eng/rust", "fresh.bin", b"x").unwrap();
        assert_eq!(out["warnings"], json!([]));
        call(
            &wiki,
            "set_config",
            json!({ "key": "ignore", "value": ["*.tmp"], "scope": "wiki" }),
        )
        .unwrap();
        let out = add(&wiki, "eng/rust", "scratch.tmp", b"x").unwrap();
        assert_eq!(out["warnings"][0]["kind"], "ignored");
    }

    #[test]
    fn add_attachment_rejects_bad_input() {
        let (_dir, wiki) = wiki();
        for (page, name, kind) in [
            ("nope", "a.png", ErrorKind::NotFound),
            ("eng/Rust", "a.png", ErrorKind::NotFound),
            ("eng/ideas", "a.png", ErrorKind::NotFound),
            ("eng/rust", "sub/a.png", ErrorKind::InvalidInput),
            ("eng/rust", ".hidden", ErrorKind::InvalidInput),
            ("eng/rust", "page.md", ErrorKind::InvalidInput),
            ("eng/rust", "..", ErrorKind::InvalidInput),
        ] {
            let err = add(&wiki, page, name, b"x").unwrap_err();
            assert_eq!(err.kind, kind, "{page} {name}");
        }
        for source in [
            json!({ "base64": "AAE=!" }),
            json!({}),
            json!({ "base64": "", "local_path": "/x" }),
            json!({ "local_path": "/definitely/not/here" }),
        ] {
            let err = call(
                &wiki,
                "add_attachment",
                json!({ "page": "notes", "name": "a", "source": source }),
            )
            .unwrap_err();
            assert_eq!(err.kind, ErrorKind::InvalidInput, "{source}");
        }
    }

    #[test]
    fn local_paths_need_local_file_access() {
        let (dir, wiki) = wiki();
        let source = dir.path().join("outside.pdf");
        std::fs::write(&source, b"%PDF").unwrap();
        let input = json!({ "page": "notes", "name": "spec.pdf",
                            "source": { "local_path": source.display().to_string() } });
        let remote = wiki.clone().without_local_fs();
        let err = call(&remote, "add_attachment", input.clone()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        let err = call(
            &remote,
            "read_attachment",
            json!({ "path": "eng/rust/d.png", "as": "local_path" }),
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        assert!(
            call(
                &remote,
                "read_attachment",
                json!({ "path": "eng/rust/d.png" })
            )
            .is_ok()
        );

        call(&wiki, "add_attachment", input).unwrap();
        assert_eq!(read(&wiki, "notes/spec.pdf"), b"%PDF");
        assert!(source.exists(), "copied, not moved");
    }

    #[test]
    fn listing_is_per_page_or_per_scope() {
        let (_dir, wiki) = wiki();
        assert_eq!(
            listed(&wiki, json!({ "page": "eng/rust" })),
            ["eng/rust/d.png"],
            "not a Child Page's"
        );
        assert_eq!(
            listed(&wiki, json!({ "page": "eng/ideas" })),
            ["eng/ideas/loose.txt"],
            "a Placeholder's folder"
        );
        assert_eq!(
            listed(&wiki, json!({ "scope": { "path_prefix": "eng/rust" } })),
            ["eng/rust/async/deep.png", "eng/rust/d.png"]
        );
        assert_eq!(listed(&wiki, json!({})).len(), 3);
        for (input, kind) in [
            (json!({ "page": "nope" }), ErrorKind::NotFound),
            (json!({ "page": "Eng/rust" }), ErrorKind::NotFound),
            (
                json!({ "page": "eng", "scope": { "space": "eng" } }),
                ErrorKind::InvalidInput,
            ),
        ] {
            let err = call(&wiki, "list_attachments", input).unwrap_err();
            assert_eq!(err.kind, kind);
        }
    }

    #[test]
    fn only_existing_visible_attachments_can_be_read() {
        let (_dir, wiki) = wiki();
        call(
            &wiki,
            "set_config",
            json!({ "key": "ignore", "value": ["ideas"], "scope": "wiki" }),
        )
        .unwrap();
        for (path, kind) in [
            ("eng/rust/nope.png", ErrorKind::NotFound),
            ("eng/rust/D.png", ErrorKind::NotFound),
            ("eng/rust", ErrorKind::NotFound),
            ("eng/ideas/loose.txt", ErrorKind::NotFound),
            ("eng/rust.md", ErrorKind::InvalidPath),
            (".wikirs/config.toml", ErrorKind::InvalidPath),
        ] {
            let err = call(&wiki, "read_attachment", json!({ "path": path })).unwrap_err();
            assert_eq!(err.kind, kind, "{path}");
        }
    }

    #[test]
    fn move_rewrites_links_and_moving_back_restores_every_byte() {
        let (_dir, wiki) = wiki();
        let (rust, notes) = (read(&wiki, "eng/rust.md"), read(&wiki, "notes.md"));
        let out = call(
            &wiki,
            "move_attachment",
            json!({ "from": "eng/rust/d.png", "to": "img/diagram.png" }),
        )
        .unwrap();
        assert_eq!(out["result"]["links_rewritten"], 2);
        assert_eq!(read(&wiki, "img/diagram.png"), PNG);
        assert_eq!(
            String::from_utf8(read(&wiki, "eng/rust.md")).unwrap(),
            "# Rust\n\n![d](../img/diagram.png) [spec](rust/spec.pdf)\n"
        );
        assert_eq!(
            String::from_utf8(read(&wiki, "notes.md")).unwrap(),
            "# Notes\n\n![[img/diagram.png]]\n"
        );
        let back = call(
            &wiki,
            "move_attachment",
            json!({ "from": "img/diagram.png", "to": "eng/rust/d.png" }),
        )
        .unwrap();
        assert_eq!(back["result"]["links_rewritten"], 2);
        assert_eq!(
            (read(&wiki, "eng/rust.md"), read(&wiki, "notes.md")),
            (rust, notes)
        );
        assert!(!wiki.root().join("img").exists(), "an emptied folder goes");
    }

    #[test]
    fn move_attachment_refuses_taken_and_bad_targets() {
        let (_dir, wiki) = wiki();
        for (from, to, kind) in [
            ("eng/rust/nope.png", "x.png", ErrorKind::NotFound),
            ("eng/rust/d.png", "eng/rust/d.png", ErrorKind::InvalidInput),
            (
                "eng/rust/d.png",
                "eng/ideas/loose.txt",
                ErrorKind::AlreadyExists,
            ),
            ("eng/rust/d.png", "eng/rust/async", ErrorKind::AlreadyExists),
            (
                "eng/rust/d.png",
                "eng/ideas/LOOSE.txt",
                ErrorKind::CaseConflict,
            ),
            ("eng/rust/d.png", "eng/rust/d.md", ErrorKind::InvalidPath),
            ("eng/rust/d.png", "../out.png", ErrorKind::InvalidPath),
        ] {
            let err =
                call(&wiki, "move_attachment", json!({ "from": from, "to": to })).unwrap_err();
            assert_eq!(err.kind, kind, "{from} → {to}");
        }
        assert_eq!(read(&wiki, "eng/rust/d.png"), PNG);
        let out = call(
            &wiki,
            "move_attachment",
            json!({ "from": "eng/rust/d.png", "to": "Notes", "dry_run": true }),
        );
        assert!(
            out.is_ok(),
            "`Notes` is a different file from `notes.md`: {out:?}"
        );
    }

    #[test]
    fn delete_warns_about_links_that_break() {
        let (_dir, wiki) = wiki();
        let broken = |wiki: &Wiki| {
            call(wiki, "check", json!({ "kinds": ["broken_link"] })).unwrap()["result"]
                ["diagnostics"]
                .as_array()
                .unwrap()
                .len()
        };
        let before = broken(&wiki);
        let dry = call(
            &wiki,
            "delete_attachment",
            json!({ "path": "eng/rust/d.png", "dry_run": true }),
        )
        .unwrap();
        let kinds: Vec<&str> = dry["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["link_will_break", "link_will_break"]);
        assert!(wiki.root().join("eng/rust/d.png").exists());
        call(
            &wiki,
            "delete_attachment",
            json!({ "path": "eng/rust/d.png" }),
        )
        .unwrap();
        assert!(!wiki.root().join("eng/rust/d.png").exists());
        assert!(
            wiki.root().join("eng/rust").is_dir(),
            "the Child Page's folder stays"
        );
        let err = call(
            &wiki,
            "delete_attachment",
            json!({ "path": "eng/rust/d.png" }),
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert_eq!(
            broken(&wiki),
            before + 2,
            "exactly the Links it warned about"
        );
    }
}
