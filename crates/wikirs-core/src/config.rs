//! The Wiki Operations (operations.md#wiki): `init`, `get_config`, `set_config`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Error, Kind, Operation, Result, Wiki,
    plan::{Edit, Plan, Warning, mutate},
    settings::{self, ConfigScope, Setting, WIKI_FILE},
};

// ---------------------------------------------------------------------- init

pub struct Init;

/// Make this folder a Wiki: create `.wikirs/config.toml`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct InitInput {
    /// Return the Plan without applying it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct InitOutput {
    /// The settings file created, relative to the Wiki root.
    pub path: String,
    pub plan: Plan,
    pub applied: bool,
}

const WIKI_FILE_HEADER: &str = "# wikirs settings for this Wiki, shared by every machine (commit this file).\n# `wikirs get-config` lists every setting; machine-only ones live outside the Wiki.\n";

impl Operation for Init {
    const NAME: &'static str = "init";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str =
        "Make a folder a Wiki by creating `.wikirs/config.toml`, which also marks its root.";
    type Input = InitInput;
    type Output = InitOutput;

    fn run(wiki: &Wiki, input: InitInput) -> Result<InitOutput> {
        let (plan, ()) = mutate(wiki, input.dry_run, |tx| {
            if tx.stat(WIKI_FILE)? {
                return Err(Error::already_exists(WIKI_FILE));
            }
            tx.edit(Edit::Create {
                path: WIKI_FILE.into(),
                content: WIKI_FILE_HEADER.into(),
            });
            Ok(())
        })?;
        Ok(InitOutput {
            path: WIKI_FILE.into(),
            plan,
            applied: !input.dry_run,
        })
    }
}

// ---------------------------------------------------------------- get_config

pub struct GetConfig;

/// Read one setting, or all of them, with where each value comes from.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct GetConfigInput {
    /// A setting, e.g. `links.syntax`; every setting if absent.
    pub key: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GetConfigOutput {
    pub settings: Vec<Setting>,
    /// Unreadable files and invalid values (which fall back).
    #[serde(skip)]
    #[schemars(skip)]
    warnings: Vec<Warning>,
}

impl Operation for GetConfig {
    const NAME: &'static str = "get_config";
    const KIND: Kind = Kind::Query;
    const DESCRIPTION: &'static str = "Read one setting or all of them, each with its value and source (`machine`, `wiki` or `default`).";
    type Input = GetConfigInput;
    type Output = GetConfigOutput;

    fn run(wiki: &Wiki, input: GetConfigInput) -> Result<GetConfigOutput> {
        let found = wiki.settings();
        let settings = found
            .report(input.key.as_deref())
            .ok_or_else(|| Error::not_found("setting", input.key.as_deref().unwrap_or("")))?;
        Ok(GetConfigOutput {
            settings,
            warnings: found.warnings,
        })
    }

    fn warnings(output: &GetConfigOutput) -> Vec<Warning> {
        output.warnings.clone()
    }
}

// ---------------------------------------------------------------- set_config

pub struct SetConfig;

/// Set a setting in the Wiki's file or this machine's; `null` removes it.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct SetConfigInput {
    /// A setting, e.g. `links.syntax`.
    pub key: String,
    /// The new value; `null` removes the setting from that file. On the CLI:
    /// JSON if it parses as JSON, else a string.
    #[cfg_attr(feature = "clap", arg(value_parser = json_or_string, allow_hyphen_values = true))]
    pub value: Value,
    /// `wiki` (the committed `.wikirs/config.toml`) or `machine` (this machine only).
    #[cfg_attr(feature = "clap", arg(long, value_enum))]
    pub scope: ConfigScope,
    /// Return the Plan without applying it.
    #[serde(default)]
    #[cfg_attr(feature = "clap", arg(long))]
    pub dry_run: bool,
}

#[cfg(feature = "clap")]
#[allow(clippy::unnecessary_wraps)] // clap's value_parser signature
fn json_or_string(raw: &str) -> std::result::Result<Value, String> {
    Ok(serde_json::from_str(raw).unwrap_or_else(|_| raw.into()))
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SetConfigOutput {
    pub key: String,
    pub scope: ConfigScope,
    pub plan: Plan,
    pub applied: bool,
}

/// Keys whose change the open Index has to follow.
const INDEX_KEYS: [&str; 2] = ["ignore", "search.stemming"];

impl Operation for SetConfig {
    const NAME: &'static str = "set_config";
    const KIND: Kind = Kind::Mutation;
    const DESCRIPTION: &'static str = "Set a setting in the Wiki's committed file or this machine's (`null` removes it). Machine settings override the Wiki's.";
    type Input = SetConfigInput;
    type Output = SetConfigOutput;

    fn run(wiki: &Wiki, input: SetConfigInput) -> Result<SetConfigOutput> {
        settings::check(&input.key, input.scope, &input.value).map_err(|e| match e {
            None => Error::not_found("setting", &input.key),
            Some(reason) => Error::invalid_input(Some("value"), reason),
        })?;
        // The machine file lives outside the Wiki, so its edit carries an absolute path.
        let file = match input.scope {
            ConfigScope::Wiki => WIKI_FILE.to_string(),
            ConfigScope::Machine => wiki.machine_file().display().to_string(),
        };
        let (plan, ()) = mutate(wiki, input.dry_run, |tx| {
            let current = tx.read(&file)?;
            let bad_file = |reason: String| Error::invalid_input(None, format!("{file}: {reason}"));
            match current {
                Some(current) => {
                    let new = settings::edit(&current.content, &input.key, &input.value)
                        .map_err(bad_file)?;
                    if let Some(splice) = settings::diff_splice(&current.content, &new) {
                        tx.edit(Edit::Modify {
                            path: file.clone(),
                            base_version: current.version,
                            splices: vec![splice],
                        });
                    }
                }
                None if input.value.is_null() => {}
                None => {
                    let start = match input.scope {
                        ConfigScope::Wiki => WIKI_FILE_HEADER.to_string(),
                        ConfigScope::Machine => settings::edit(
                            MACHINE_FILE_HEADER,
                            "root",
                            &Value::from(wiki.root().display().to_string()),
                        )
                        .map_err(bad_file)?,
                    };
                    let content =
                        settings::edit(&start, &input.key, &input.value).map_err(bad_file)?;
                    tx.edit(Edit::Create {
                        path: file.clone(),
                        content,
                    });
                }
            }
            Ok(())
        })?;
        if !input.dry_run && !plan.edits.is_empty() && INDEX_KEYS.contains(&input.key.as_str()) {
            wiki.reload_index_settings()?;
        }
        Ok(SetConfigOutput {
            key: input.key,
            scope: input.scope,
            plan,
            applied: !input.dry_run,
        })
    }
}

// ------------------------------------------------------------------- adopt

/// Machine settings left by a moved Wiki: a settings file whose recorded
/// `root` no longer exists (wiki-selection.md#machine-settings).
#[derive(Debug, Clone, Serialize)]
pub struct Adoptable {
    pub file: String,
    pub root: String,
}

/// What [`adopt`] would take over or took over.
#[derive(Debug, Serialize)]
pub struct Adopted {
    pub from: Adoptable,
    pub plan: Plan,
    pub applied: bool,
}

/// Settings files this Wiki could adopt; none once it has its own.
#[must_use]
pub fn adoptable(wiki: &Wiki) -> Vec<Adoptable> {
    let own = wiki.machine_file();
    let Some(dir) = own.parent().filter(|_| !own.exists()) else {
        return Vec::new();
    };
    let mut found: Vec<Adoptable> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|file| file.extension().is_some_and(|e| e == "toml"))
        .filter_map(|file| {
            let text = std::fs::read_to_string(&file).ok()?;
            let doc: toml_edit::DocumentMut = text.parse().ok()?;
            let root = doc.get("root")?.as_str()?.to_string();
            (!std::path::Path::new(&root).exists()).then(|| Adoptable {
                file: file.display().to_string(),
                root,
            })
        })
        .collect();
    found.sort_by(|a, b| a.root.cmp(&b.root));
    found
}

/// Takes over the settings of a moved Wiki: records this Wiki's root in the
/// file and renames it to this Wiki's key. `from` (a recorded root) picks one
/// when several could be adopted.
pub fn adopt(wiki: &Wiki, from: Option<&str>, dry_run: bool) -> Result<Adopted> {
    let candidates = adoptable(wiki);
    let chosen = match from {
        Some(root) => candidates
            .into_iter()
            .find(|c| c.root == root)
            .ok_or_else(|| {
                Error::invalid_input(
                    Some("from"),
                    format!("no settings of a moved Wiki at `{root}` to adopt"),
                )
            })?,
        None if candidates.len() > 1 => {
            let roots: Vec<&str> = candidates.iter().map(|c| c.root.as_str()).collect();
            return Err(Error::invalid_input(
                Some("from"),
                format!(
                    "several moved Wikis' settings could be adopted; pick one: {}",
                    roots.join(", ")
                ),
            ));
        }
        None => candidates.into_iter().next().ok_or_else(|| {
            let reason = if wiki.machine_file().exists() {
                "this Wiki already has machine settings"
            } else {
                "no settings of a moved Wiki to adopt"
            };
            Error::invalid_input(None, reason)
        })?,
    };
    let to = wiki.machine_file().display().to_string();
    let (plan, ()) = mutate(wiki, dry_run, |tx| {
        let current = tx
            .read(&chosen.file)?
            .ok_or_else(|| Error::not_found("config_key", &chosen.file))?;
        if tx.read(&to)?.is_some() {
            return Err(Error::already_exists(&to));
        }
        let root = Value::from(wiki.root().display().to_string());
        let new = settings::edit(&current.content, "root", &root)
            .map_err(|reason| Error::invalid_input(None, format!("{}: {reason}", chosen.file)))?;
        if let Some(splice) = settings::diff_splice(&current.content, &new) {
            tx.edit(Edit::Modify {
                path: chosen.file.clone(),
                base_version: current.version,
                splices: vec![splice],
            });
        }
        tx.edit(Edit::Move {
            from: chosen.file.clone(),
            to: to.clone(),
        });
        Ok(())
    })?;
    if !dry_run {
        wiki.reload_index_settings()?;
    }
    Ok(Adopted {
        from: chosen,
        plan,
        applied: !dry_run,
    })
}

const MACHINE_FILE_HEADER: &str = "# wikirs settings for one Wiki on this machine only (never synced).\n# `root` records which Wiki, so a moved Wiki's settings can be adopted.\n";

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{ErrorKind, find};

    fn wiki() -> (tempfile::TempDir, Wiki) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wiki")).unwrap();
        let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("base")).unwrap();
        (dir, wiki)
    }

    fn call(wiki: &Wiki, op: &str, input: Value) -> Result<Value> {
        find(op).unwrap().call(wiki, input)
    }

    #[allow(clippy::needless_pass_by_value)] // reads better at the call sites
    fn set(wiki: &Wiki, key: &str, value: Value, scope: &str) -> Result<Value> {
        call(
            wiki,
            "set_config",
            json!({ "key": key, "value": value, "scope": scope }),
        )
    }

    /// `(value, source)` of one setting.
    fn get(wiki: &Wiki, key: &str) -> (Value, String) {
        let out = call(wiki, "get_config", json!({ "key": key })).unwrap();
        let s = &out["result"]["settings"][0];
        (
            s["value"].clone(),
            s["source"].as_str().unwrap().to_string(),
        )
    }

    fn read(wiki: &Wiki, rel: &str) -> String {
        std::fs::read_to_string(wiki.root().join(rel)).unwrap()
    }

    #[test]
    fn init_creates_the_settings_file_once() {
        let (_dir, wiki) = wiki();
        let dry = call(&wiki, "init", json!({ "dry_run": true })).unwrap();
        assert!(
            !wiki.root().join(WIKI_FILE).exists(),
            "a dry run writes nothing"
        );
        let done = call(&wiki, "init", json!({})).unwrap();
        assert_eq!(dry["result"]["plan"], done["result"]["plan"]);
        assert_eq!(read(&wiki, WIKI_FILE), WIKI_FILE_HEADER);
        let err = call(&wiki, "init", json!({})).unwrap_err();
        assert_eq!(err.kind, ErrorKind::AlreadyExists);
        set(&wiki, "links.syntax", json!("wikilink"), "wiki").unwrap();
        assert_eq!(
            read(&wiki, WIKI_FILE),
            format!("{WIKI_FILE_HEADER}\n[links]\nsyntax = \"wikilink\"\n"),
            "the header stays on top"
        );
        let indexed = wiki.index().dump().unwrap();
        assert!(
            indexed.iter().all(|row| !row.contains(".wikirs")),
            "the hidden file isn't indexed: {indexed:?}"
        );
    }

    #[test]
    fn set_config_splices_the_wiki_file_and_machine_values_override_it() {
        let (_dir, wiki) = wiki();
        assert_eq!(
            get(&wiki, "links.syntax"),
            (json!("standard"), "default".into())
        );

        set(&wiki, "links.syntax", json!("wikilink"), "wiki").unwrap();
        assert_eq!(
            read(&wiki, WIKI_FILE),
            format!("{WIKI_FILE_HEADER}\n[links]\nsyntax = \"wikilink\"\n"),
            "created with its header"
        );
        assert_eq!(
            get(&wiki, "links.syntax"),
            (json!("wikilink"), "wiki".into())
        );

        // Hand-written comments and layout survive an edit.
        let hand = "# mine\nignore = [\"a\"] # keep\n\n[links]\nsyntax = \"wikilink\"\n";
        std::fs::write(wiki.root().join(WIKI_FILE), hand).unwrap();
        let out = set(&wiki, "search.stemming", json!("english"), "wiki").unwrap();
        let splices = &out["result"]["plan"]["edits"][0]["splices"];
        assert_eq!(splices.as_array().unwrap().len(), 1);
        assert_eq!(splices[0]["old"], "", "a pure insertion");
        assert!(
            read(&wiki, WIKI_FILE).starts_with(hand),
            "{}",
            read(&wiki, WIKI_FILE)
        );

        let out = set(&wiki, "links.syntax", json!("standard"), "machine").unwrap();
        let machine = wiki.machine_file().display().to_string();
        assert_eq!(out["result"]["plan"]["edits"][0]["path"], machine.as_str());
        let text = std::fs::read_to_string(wiki.machine_file()).unwrap();
        let recorded: toml_edit::DocumentMut = text.parse().unwrap();
        assert_eq!(
            recorded["root"].as_str(),
            Some(wiki.root().display().to_string().as_str()),
            "{text}"
        );
        assert_eq!(
            get(&wiki, "links.syntax"),
            (json!("standard"), "machine".into())
        );

        set(&wiki, "links.syntax", Value::Null, "machine").unwrap();
        assert_eq!(
            get(&wiki, "links.syntax"),
            (json!("wikilink"), "wiki".into())
        );
        set(&wiki, "links.syntax", Value::Null, "wiki").unwrap();
        assert!(
            !read(&wiki, WIKI_FILE).contains("[links]"),
            "an emptied table goes too"
        );
        let out = set(&wiki, "links.syntax", Value::Null, "wiki").unwrap();
        assert!(
            out["result"]["plan"]["edits"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn set_config_checks_key_scope_and_type() {
        let (_dir, wiki) = wiki();
        for (key, value, scope, kind) in [
            ("nope", json!(1), "wiki", ErrorKind::NotFound),
            ("serve.port", json!(80), "wiki", ErrorKind::InvalidInput),
            (
                "serve.port",
                json!(70000),
                "machine",
                ErrorKind::InvalidInput,
            ),
            (
                "serve.port",
                json!("80"),
                "machine",
                ErrorKind::InvalidInput,
            ),
            (
                "links.syntax",
                json!("html"),
                "wiki",
                ErrorKind::InvalidInput,
            ),
            ("ignore", json!(["a[b"]), "wiki", ErrorKind::InvalidInput),
            ("ignore", json!("a"), "wiki", ErrorKind::InvalidInput),
            (
                "serve.read_only",
                json!("yes"),
                "machine",
                ErrorKind::InvalidInput,
            ),
            (
                "serve.max_body",
                json!(0),
                "machine",
                ErrorKind::InvalidInput,
            ),
        ] {
            let err = set(&wiki, key, value, scope).unwrap_err();
            assert_eq!(err.kind, kind, "{key}");
        }
        let out = set(&wiki, "serve.port", Value::Null, "machine").unwrap();
        assert!(
            out["result"]["plan"]["edits"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            !wiki.machine_file().exists(),
            "removing from no file creates none"
        );
        set(&wiki, "serve.port", json!(8080), "machine").unwrap();
        assert_eq!(get(&wiki, "serve.port"), (json!(8080), "machine".into()));
        let err = call(&wiki, "get_config", json!({ "key": "nope" })).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        let all = call(&wiki, "get_config", json!({})).unwrap();
        assert_eq!(all["result"]["settings"].as_array().unwrap().len(), 12);
    }

    #[test]
    fn bad_files_and_values_fall_back_with_a_warning() {
        let (_dir, wiki) = wiki();
        std::fs::create_dir_all(wiki.root().join(".wikirs")).unwrap();
        std::fs::write(
            wiki.root().join(WIKI_FILE),
            "ignore = [\"ok\", 3]\nwatcher = \"poll\"\n[search]\nstemming = \"latin\"\n",
        )
        .unwrap();
        let out = call(&wiki, "get_config", json!({})).unwrap();
        let messages: Vec<&str> = out["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["message"].as_str().unwrap())
            .collect();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert_eq!(
            get(&wiki, "search.stemming"),
            (json!("none"), "default".into())
        );
        assert_eq!(
            get(&wiki, "watcher"),
            (json!("native"), "default".into()),
            "a machine key in the Wiki file is not read"
        );
        std::fs::write(wiki.root().join(WIKI_FILE), "ignore = [\"a[\"]\n").unwrap();
        let out = call(&wiki, "get_config", json!({})).unwrap();
        assert_eq!(
            out["warnings"][0]["kind"], "bad_setting",
            "a bad glob is reported"
        );

        std::fs::write(wiki.root().join(WIKI_FILE), "not = [toml").unwrap();
        let out = call(&wiki, "get_config", json!({})).unwrap();
        assert_eq!(out["warnings"][0]["kind"], "bad_setting");
        let err = set(&wiki, "ignore", json!([]), "wiki").unwrap_err();
        assert_eq!(
            err.kind,
            ErrorKind::InvalidInput,
            "never overwrite a broken file"
        );
    }

    fn paths(wiki: &Wiki) -> Vec<String> {
        let out = call(wiki, "list_pages", json!({})).unwrap();
        out["result"]["pages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["path"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn ignore_hides_files_from_the_index_at_once() {
        let (dir, wiki) = wiki();
        for f in [
            "a.md",
            "drafts/b.md",
            "eng/drafts/c.md",
            "eng/d.tmp.md",
            "eng/e.md",
        ] {
            let p = wiki.root().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "# x\n").unwrap();
        }
        let wiki = Wiki::open_isolated(wiki.root(), dir.path().join("base")).unwrap();
        assert_eq!(paths(&wiki).len(), 5);
        set(&wiki, "ignore", json!(["drafts", "*.tmp.md"]), "wiki").unwrap();
        assert_eq!(paths(&wiki), ["a", "eng/e"], "names match at any depth");

        call(&wiki, "create_page", json!({ "path": "drafts/new" })).unwrap();
        assert_eq!(
            paths(&wiki),
            ["a", "eng/e"],
            "a write into an ignored folder stays out"
        );
        let reopened = Wiki::open_isolated(wiki.root(), dir.path().join("base")).unwrap();
        assert_eq!(paths(&reopened), ["a", "eng/e"]);

        set(&wiki, "ignore", json!(["eng/drafts/**"]), "wiki").unwrap();
        assert_eq!(
            paths(&wiki),
            ["a", "drafts/b", "drafts/new", "eng/d.tmp", "eng/e"],
            "a pattern with `/` is anchored at the root"
        );
    }

    #[test]
    fn stemming_switches_the_tokenizer_and_reindexes() {
        let (dir, wiki) = wiki();
        std::fs::write(wiki.root().join("a.md"), "# A\n\nThe dogs were running.\n").unwrap();
        let wiki = Wiki::open_isolated(wiki.root(), dir.path().join("base")).unwrap();
        let hits = |wiki: &Wiki| {
            call(wiki, "search", json!({ "text": "runs" })).unwrap()["result"]["total"].clone()
        };
        assert_eq!(hits(&wiki), 0);
        set(&wiki, "search.stemming", json!("english"), "wiki").unwrap();
        assert_eq!(hits(&wiki), 1, "`runs` and `running` share a stem");
        let reopened = Wiki::open_isolated(wiki.root(), dir.path().join("base")).unwrap();
        assert_eq!(hits(&reopened), 1);
        set(&wiki, "search.stemming", json!("none"), "machine").unwrap();
        assert_eq!(
            hits(&wiki),
            0,
            "a machine value overrides, and applies at once"
        );
    }

    #[test]
    fn cache_dir_moves_the_index_at_next_open() {
        let (dir, wiki) = wiki();
        let moved = dir.path().join("elsewhere");
        set(
            &wiki,
            "cache_dir",
            json!(moved.display().to_string()),
            "machine",
        )
        .unwrap();
        let reopened = Wiki::open_isolated(wiki.root(), dir.path().join("base")).unwrap();
        assert_eq!(reopened.cache_dir(), moved);
        assert!(moved.join("index.db").exists());
    }

    #[test]
    fn a_moved_wiki_adopts_its_machine_settings() {
        let (dir, wiki) = wiki();
        set(&wiki, "links.syntax", json!("wikilink"), "machine").unwrap();
        set(&wiki, "ignore", json!(["secret.md"]), "machine").unwrap();
        std::fs::write(wiki.root().join("secret.md"), "# Secret\n").unwrap();
        let old_file = wiki.machine_file().to_path_buf();
        drop(wiki);
        let base = dir.path().join("base");
        std::fs::rename(dir.path().join("wiki"), dir.path().join("moved")).unwrap();
        let moved = Wiki::open_isolated(dir.path().join("moved"), &base).unwrap();
        assert_eq!(get(&moved, "links.syntax").1, "default", "the key changed");
        let pages =
            |wiki: &Wiki| call(wiki, "list_pages", json!({})).unwrap()["result"]["total"].clone();
        assert_eq!(pages(&moved), 1, "nothing ignored without the settings");

        let found = adoptable(&moved);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].file, old_file.display().to_string());

        let dry = adopt(&moved, None, true).unwrap();
        assert!(old_file.exists(), "a dry run moves nothing");
        let done = adopt(&moved, None, false).unwrap();
        assert_eq!(dry.plan, done.plan);
        assert!(!old_file.exists());
        assert_eq!(
            get(&moved, "links.syntax"),
            (json!("wikilink"), "machine".into())
        );
        assert_eq!(pages(&moved), 0, "adopted Index settings apply at once");
        let text = std::fs::read_to_string(moved.machine_file()).unwrap();
        let recorded: toml_edit::DocumentMut = text.parse().unwrap();
        assert_eq!(
            recorded["root"].as_str(),
            Some(moved.root().display().to_string().as_str())
        );
        assert!(adoptable(&moved).is_empty(), "it has its own settings now");
        let err = adopt(&moved, None, false).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
    }

    #[test]
    fn several_adoptable_settings_need_a_choice() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base");
        for name in ["a", "b"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
            let wiki = Wiki::open_isolated(dir.path().join(name), &base).unwrap();
            set(&wiki, "links.syntax", json!("wikilink"), "machine").unwrap();
            drop(wiki);
            std::fs::remove_dir_all(dir.path().join(name)).unwrap();
        }
        std::fs::create_dir(dir.path().join("c")).unwrap();
        let wiki = Wiki::open_isolated(dir.path().join("c"), &base).unwrap();
        let roots: Vec<String> = adoptable(&wiki).into_iter().map(|a| a.root).collect();
        assert_eq!(roots.len(), 2, "{roots:?}");
        let err = adopt(&wiki, None, false).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        assert!(err.message.contains(&roots[1]), "{}", err.message);
        let err = adopt(&wiki, Some("/nowhere"), false).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        adopt(&wiki, Some(&roots[1]), false).unwrap();
        assert_eq!(get(&wiki, "links.syntax").1, "machine");
        assert!(adoptable(&wiki).is_empty());
    }
}
