//! The parity test (docs/spec/interfaces.md#parity-test):
//! 1. coverage: every registry Operation is exposed by every adapter (the TUI's
//!    palette included, each with a form);
//! 2. behaviour: one scenario gives identical JSON through core, CLI, MCP and HTTP;
//! 3. contract snapshot: the catalogue JSON.

use std::process::Command;

use rmcp::{ServiceExt, model::CallToolRequestParams};
use serde_json::{Value, json};
use wikirs_core::{Kind, Wiki};

/// A fresh Wiki whose cache dir (lock, journal) also lives in the tempdir.
fn temp_wiki() -> (tempfile::TempDir, Wiki) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("wiki")).unwrap();
    let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("cache")).unwrap();
    (dir, wiki)
}

fn registry_names(include: impl Fn(Kind) -> bool) -> Vec<String> {
    let mut names: Vec<String> = wikirs_core::registry()
        .iter()
        .filter(|op| include(op.kind))
        .map(|op| op.name.to_string())
        .collect();
    names.sort();
    names
}

// ----------------------------------------------------------------- coverage

#[test]
fn cli_exposes_every_operation() {
    let mut cli: Vec<String> = wikirs_cli::operation_subcommands()
        .iter()
        .map(|n| n.replace('-', "_"))
        .collect();
    cli.sort();
    assert_eq!(
        cli,
        registry_names(|_| true),
        "CLI subcommands differ from the registry"
    );
}

#[test]
fn tui_palette_lists_every_operation_with_a_form() {
    let mut palette = wikirs_tui::palette_entries();
    palette.sort();
    assert_eq!(
        palette,
        registry_names(|_| true),
        "TUI palette entries differ from the registry"
    );
    // The form renderer supports every Input's schema (interfaces.md#parity-test).
    for name in &palette {
        if let Err(err) = wikirs_forms::Form::for_op(name).expect("in the registry") {
            panic!("TUI palette: {err}");
        }
    }
}

#[tokio::test]
async fn mcp_exposes_every_operation_as_a_tool() {
    let (_dir, wiki) = temp_wiki();
    let client = mcp_client(wiki).await;
    let mut tools: Vec<String> = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    tools.sort();
    assert_eq!(
        tools,
        registry_names(|k| k != Kind::Subscription),
        "MCP tools differ from the registry"
    );
}

/// One HTTP request through the router (no socket), as status and JSON body.
async fn http(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let body = body.map_or_else(String::new, Value::to_string);
    let response = router
        .clone()
        .oneshot(request.body(axum::body::Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn http_lists_every_operation() {
    let (_dir, wiki) = temp_wiki();
    for read_only in [false, true] {
        let mut policy = wikirs_http::Policy::loopback(0);
        policy.read_only = read_only;
        let router = wikirs_http::router(&wiki, policy);
        let (status, catalogue) = http(&router, "GET", "/ops", None).await;
        assert_eq!(status, 200);
        let mut names: Vec<String> = catalogue["operations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|op| op["name"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            registry_names(|k| !read_only || k != Kind::Mutation),
            "GET /ops differs from the registry (read_only: {read_only})"
        );
    }
}

// ---------------------------------------------------------------- behaviour

/// One step: Operation, JSON input (core, MCP), and the same input as CLI args.
struct Step {
    op: &'static str,
    input: Value,
    cli: &'static [&'static str],
}

#[allow(clippy::too_many_lines)] // one flat list of steps
fn scenario() -> Vec<Step> {
    vec![
        Step {
            op: "create_page",
            input: json!({ "title": "Async Notes", "parent": "eng", "dry_run": true }),
            cli: &[
                "create-page",
                "--title",
                "Async Notes",
                "--parent",
                "eng",
                "--dry-run",
            ],
        },
        Step {
            op: "get_page",
            input: json!({ "page": "eng/async-notes" }),
            cli: &["get-page", "eng/async-notes"],
        },
        Step {
            op: "create_page",
            input: json!({ "title": "Async Notes", "parent": "eng" }),
            cli: &["create-page", "--title", "Async Notes", "--parent", "eng"],
        },
        Step {
            op: "get_page",
            input: json!({ "page": "eng/async-notes" }),
            cli: &["get-page", "eng/async-notes"],
        },
        Step {
            op: "create_page",
            input: json!({ "path": "eng/async-notes" }),
            cli: &["create-page", "--path", "eng/async-notes"],
        },
        Step {
            op: "create_page",
            input: json!({ "path": "eng/Async-Notes" }),
            cli: &["create-page", "--path", "eng/Async-Notes"],
        },
        Step {
            op: "create_page",
            input: json!({ "path": "eng/../x" }),
            cli: &["create-page", "--path", "eng/../x"],
        },
        Step {
            op: "create_page",
            input: json!({ "path": "notes", "content": "no heading\n" }),
            cli: &[
                "create-page",
                "--path",
                "notes",
                "--content",
                "no heading\n",
            ],
        },
        Step {
            op: "get_page",
            input: json!({ "page": "notes" }),
            cli: &["get-page", "notes"],
        },
        Step {
            op: "write_page",
            input: json!({ "page": "notes", "content": "# Notes\n\nv2\n", "base_version": "stale" }),
            cli: &[
                "write-page",
                "notes",
                "--content",
                "# Notes\n\nv2\n",
                "--base-version",
                "stale",
            ],
        },
        Step {
            op: "write_page",
            input: json!({ "page": "notes", "content": "# Notes\n\nv2\n" }),
            cli: &["write-page", "notes", "--content", "# Notes\n\nv2\n"],
        },
        Step {
            op: "edit_page",
            input: json!({ "page": "notes", "edits": [{ "old": "v2", "new": "v3" }] }),
            cli: &["edit-page", "notes", "--edit", r#"{"old":"v2","new":"v3"}"#],
        },
        Step {
            op: "edit_page",
            input: json!({ "page": "notes", "edits": [{ "old": "missing", "new": "x" }] }),
            cli: &[
                "edit-page",
                "notes",
                "--edit",
                r#"{"old":"missing","new":"x"}"#,
            ],
        },
        Step {
            op: "edit_page",
            input: json!({ "page": "notes", "edits": [{ "old": "\n", "new": " " }] }),
            cli: &["edit-page", "notes", "--edit", r#"{"old":"\n","new":" "}"#],
        },
        Step {
            op: "edit_page",
            input: json!({ "page": "notes", "edits": [{ "old": "v3", "new": "v4" }], "dry_run": true }),
            cli: &[
                "edit-page",
                "notes",
                "--edit",
                r#"{"old":"v3","new":"v4"}"#,
                "--dry-run",
            ],
        },
        Step {
            op: "get_page",
            input: json!({ "page": "notes" }),
            cli: &["get-page", "notes"],
        },
        Step {
            op: "list_pages",
            input: json!({ "filter": { "space": "eng" }, "sort": "title" }),
            cli: &["list-pages", "--space", "eng", "--sort", "title"],
        },
        Step {
            op: "search",
            input: json!({ "text": "v3" }),
            cli: &["search", "v3"],
        },
        Step {
            op: "search",
            input: json!({ "text": "   " }),
            cli: &["search", "   "],
        },
        Step {
            op: "create_page",
            input: json!({ "path": "eng/rust", "content": "---\ntags: [lang/rust]\n---\n# Rust\n\n## Pinning\n\n[[eng/async-notes]] [[eng/async-notes#nope]] [[Eng/Async-Notes]] [[eng/missing]] [n](../notes.md) #Lang/Unsafe\n" }),
            cli: &[
                "create-page",
                "--path",
                "eng/rust",
                "--content",
                "---\ntags: [lang/rust]\n---\n# Rust\n\n## Pinning\n\n[[eng/async-notes]] [[eng/async-notes#nope]] [[Eng/Async-Notes]] [[eng/missing]] [n](../notes.md) #Lang/Unsafe\n",
            ],
        },
        Step {
            op: "links",
            input: json!({ "page": "eng/rust" }),
            cli: &["links", "eng/rust"],
        },
        Step {
            op: "backlinks",
            input: json!({ "target": "eng/async-notes" }),
            cli: &["backlinks", "eng/async-notes"],
        },
        Step {
            op: "resolve_link",
            input: json!({ "from_page": "notes", "raw": "[[eng/rust#pinning]]" }),
            cli: &["resolve-link", "notes", "[[eng/rust#pinning]]"],
        },
        Step {
            op: "resolve_link",
            input: json!({ "from_page": "notes", "raw": "plain text" }),
            cli: &["resolve-link", "notes", "plain text"],
        },
        Step {
            op: "outline",
            input: json!({ "page": "eng/rust" }),
            cli: &["outline", "eng/rust"],
        },
        Step {
            op: "tag_tree",
            input: json!({}),
            cli: &["tag-tree"],
        },
        Step {
            op: "list_pages",
            input: json!({ "filter": { "tag": "lang" }, "sort": "path" }),
            cli: &["list-pages", "--tag", "lang"],
        },
        Step {
            op: "check",
            input: json!({ "kinds": ["broken_link", "case_fallback", "heading_missing", "mixed_case_tag"] }),
            cli: &[
                "check",
                "--kind",
                "broken-link",
                "--kind",
                "case-fallback",
                "--kind",
                "heading-missing",
                "--kind",
                "mixed-case-tag",
            ],
        },
        Step {
            op: "index_status",
            input: json!({}),
            cli: &["index-status"],
        },
        Step {
            op: "rebuild_index",
            input: json!({}),
            cli: &["rebuild-index"],
        },
        Step {
            op: "tag_page",
            input: json!({ "page": "eng/rust", "tags": ["project/wikirs", "lang/rust"] }),
            cli: &["tag-page", "eng/rust", "project/wikirs", "lang/rust"],
        },
        Step {
            op: "rename_tag",
            input: json!({ "from": "lang", "to": "language" }),
            cli: &["rename-tag", "lang", "language"],
        },
        Step {
            op: "untag_page",
            input: json!({ "page": "eng/rust", "tags": ["project/wikirs"] }),
            cli: &["untag-page", "eng/rust", "project/wikirs"],
        },
        Step {
            op: "move_page",
            input: json!({ "from": "eng/rust", "to": "lang/rust", "dry_run": true }),
            cli: &["move-page", "eng/rust", "lang/rust", "--dry-run"],
        },
        Step {
            op: "move_page",
            input: json!({ "from": "eng/rust", "to": "eng/rust/inside" }),
            cli: &["move-page", "eng/rust", "eng/rust/inside"],
        },
        Step {
            op: "move_page",
            input: json!({ "from": "eng/async-notes", "to": "lang/async" }),
            cli: &["move-page", "eng/async-notes", "lang/async"],
        },
        Step {
            op: "get_page",
            input: json!({ "page": "eng/rust" }),
            cli: &["get-page", "eng/rust"],
        },
        Step {
            op: "delete_page",
            input: json!({ "page": "lang/async" }),
            cli: &["delete-page", "lang/async"],
        },
        Step {
            op: "delete_page",
            input: json!({ "page": "nope", "recursive": true }),
            cli: &["delete-page", "nope", "--recursive"],
        },
        Step {
            op: "set_page_meta",
            input: json!({ "page": "eng/rust", "meta": { "order": 5, "status": "draft" } }),
            cli: &[
                "set-page-meta",
                "eng/rust",
                "--set",
                "status=draft",
                "--set",
                "order=5",
            ],
        },
        Step {
            op: "set_page_meta",
            input: json!({ "page": "eng/rust", "meta": { "tags": ["x"] } }),
            cli: &["set-page-meta", "eng/rust", "--set", r#"tags=["x"]"#],
        },
        Step {
            op: "create_page",
            input: json!({ "path": "eng/zeta" }),
            cli: &["create-page", "--path", "eng/zeta"],
        },
        Step {
            op: "reorder_page",
            input: json!({ "page": "eng/zeta", "before": "eng/rust" }),
            cli: &["reorder-page", "eng/zeta", "--before", "eng/rust"],
        },
        Step {
            op: "reorder_page",
            input: json!({ "page": "eng/zeta", "after": "notes" }),
            cli: &["reorder-page", "eng/zeta", "--after", "notes"],
        },
        Step {
            op: "children",
            input: json!({}),
            cli: &["children"],
        },
        Step {
            op: "children",
            input: json!({ "parent": "eng", "depth": 2 }),
            cli: &["children", "eng", "--depth", "2"],
        },
        Step {
            op: "list_spaces",
            input: json!({}),
            cli: &["list-spaces"],
        },
        Step {
            op: "init",
            input: json!({}),
            cli: &["init"],
        },
        Step {
            op: "init",
            input: json!({}),
            cli: &["init"],
        },
        Step {
            op: "set_config",
            input: json!({ "key": "links.syntax", "value": "wikilink", "scope": "wiki" }),
            cli: &["set-config", "links.syntax", "wikilink", "--scope", "wiki"],
        },
        Step {
            op: "set_config",
            input: json!({ "key": "serve.port", "value": 8080, "scope": "machine" }),
            cli: &["set-config", "serve.port", "8080", "--scope", "machine"],
        },
        Step {
            op: "set_config",
            input: json!({ "key": "serve.port", "value": 80, "scope": "wiki" }),
            cli: &["set-config", "serve.port", "80", "--scope", "wiki"],
        },
        Step {
            op: "set_config",
            input: json!({ "key": "nope", "value": null, "scope": "machine" }),
            cli: &["set-config", "nope", "null", "--scope", "machine"],
        },
        Step {
            op: "set_config",
            input: json!({ "key": "ignore", "value": ["zeta.md"], "scope": "wiki" }),
            cli: &["set-config", "ignore", r#"["zeta.md"]"#, "--scope", "wiki"],
        },
        Step {
            op: "list_pages",
            input: json!({}),
            cli: &["list-pages"],
        },
        Step {
            op: "get_config",
            input: json!({}),
            cli: &["get-config"],
        },
        Step {
            op: "add_attachment",
            input: json!({ "page": "eng/rust", "name": "d.png",
                           "source": { "base64": "iVBORw0K" }, "dry_run": true }),
            cli: &[
                "add-attachment",
                "eng/rust",
                "d.png",
                "--base64",
                "iVBORw0K",
                "--dry-run",
            ],
        },
        Step {
            op: "add_attachment",
            input: json!({ "page": "eng/rust", "name": "d.png", "source": { "base64": "iVBORw0K" } }),
            cli: &[
                "add-attachment",
                "eng/rust",
                "d.png",
                "--base64",
                "iVBORw0K",
            ],
        },
        Step {
            op: "add_attachment",
            input: json!({ "page": "eng/rust", "name": "d.png", "source": { "base64": "AAE=" } }),
            cli: &["add-attachment", "eng/rust", "d.png", "--base64", "AAE="],
        },
        Step {
            op: "add_attachment",
            input: json!({ "page": "nope", "name": "d.png", "source": { "base64": "AAE=" } }),
            cli: &["add-attachment", "nope", "d.png", "--base64", "AAE="],
        },
        Step {
            op: "list_attachments",
            input: json!({ "page": "eng/rust" }),
            cli: &["list-attachments", "--page", "eng/rust"],
        },
        Step {
            op: "read_attachment",
            input: json!({ "path": "eng/rust/d.png" }),
            cli: &["read-attachment", "eng/rust/d.png"],
        },
        Step {
            op: "read_attachment",
            input: json!({ "path": "eng/rust/d-2.png", "as": "local_path" }),
            cli: &["read-attachment", "eng/rust/d-2.png", "--as", "local-path"],
        },
        Step {
            op: "create_page",
            input: json!({ "path": "gallery", "content": "![d](eng/rust/d.png)\n" }),
            cli: &[
                "create-page",
                "--path",
                "gallery",
                "--content",
                "![d](eng/rust/d.png)\n",
            ],
        },
        Step {
            op: "move_attachment",
            input: json!({ "from": "eng/rust/d.png", "to": "img/d.png" }),
            cli: &["move-attachment", "eng/rust/d.png", "img/d.png"],
        },
        Step {
            op: "move_attachment",
            input: json!({ "from": "eng/rust/d-2.png", "to": "img/d.png" }),
            cli: &["move-attachment", "eng/rust/d-2.png", "img/d.png"],
        },
        Step {
            op: "get_page",
            input: json!({ "page": "gallery" }),
            cli: &["get-page", "gallery"],
        },
        Step {
            op: "delete_attachment",
            input: json!({ "path": "img/d.png" }),
            cli: &["delete-attachment", "img/d.png"],
        },
        Step {
            op: "delete_attachment",
            input: json!({ "path": "img/d.png" }),
            cli: &["delete-attachment", "img/d.png"],
        },
    ]
}

/// Drops the fields that legitimately differ between runs: each Interface
/// works on its own temp Wiki (paths) at its own moment (timestamps).
fn normalize(mut value: Value) -> Value {
    match &mut value {
        Value::Object(map) => {
            for key in ["root", "cache_dir", "last_updated", "modified"] {
                map.remove(key);
            }
            for v in map.values_mut() {
                *v = normalize(v.take());
            }
        }
        Value::Array(items) => {
            for v in items {
                *v = normalize(v.take());
            }
        }
        _ => {}
    }
    value
}

/// Replaces what differs between each Interface's temp Wiki inside strings:
/// its temp dir (raw and canonical) and its Wiki key (machine settings path).
struct Scrub(Vec<(String, &'static str)>);

impl Scrub {
    fn of(dir: &tempfile::TempDir, wiki: &Wiki) -> Self {
        let key = wiki.machine_file().file_stem().unwrap().to_string_lossy();
        let canonical = wiki.root().parent().unwrap().display().to_string();
        Self(vec![
            (key.to_string(), "<key>"),
            (canonical, "<tmp>"),
            (dir.path().display().to_string(), "<tmp>"),
        ])
    }

    fn apply(&self, value: Value) -> Value {
        match value {
            Value::String(mut text) => {
                for (from, to) in &self.0 {
                    text = text.replace(from, to);
                }
                Value::String(text)
            }
            Value::Array(items) => Value::Array(items.into_iter().map(|v| self.apply(v)).collect()),
            Value::Object(map) => {
                Value::Object(map.into_iter().map(|(k, v)| (k, self.apply(v))).collect())
            }
            other => other,
        }
    }
}

fn outcome(result: wikirs_core::Result<Value>) -> Value {
    result.unwrap_or_else(|e| e.to_json())
}

fn via_core(steps: &[Step]) -> Vec<Value> {
    let (dir, wiki) = temp_wiki();
    let scrub = Scrub::of(&dir, &wiki);
    steps
        .iter()
        .map(|s| {
            scrub.apply(outcome(
                wikirs_core::find(s.op)
                    .unwrap()
                    .call(&wiki, s.input.clone()),
            ))
        })
        .collect()
}

/// Through the binary, with each step's CLI arguments or, with `as_json`, its
/// JSON Input as `--input`.
fn via_cli(steps: &[Step], as_json: bool) -> Vec<Value> {
    let (dir, wiki) = temp_wiki();
    let scrub = Scrub::of(&dir, &wiki);
    steps
        .iter()
        .map(|s| {
            let out = Command::new(env!("CARGO_BIN_EXE_wikirs"))
                .env("WIKIRS_CACHE_DIR", dir.path().join("cache"))
                // Where `open_isolated` puts machine settings for the same base.
                .env("WIKIRS_CONFIG_DIR", dir.path().join("cache").join("config"))
                .arg("--wiki")
                .arg(dir.path().join("wiki"))
                .arg("--json")
                .args(if as_json {
                    vec![
                        s.op.replace('_', "-"),
                        "--input".into(),
                        s.input.to_string(),
                    ]
                } else {
                    s.cli.iter().map(ToString::to_string).collect()
                })
                .output()
                .unwrap();
            scrub.apply(serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
                panic!(
                    "{}: bad JSON ({e}): {}",
                    s.op,
                    String::from_utf8_lossy(&out.stdout)
                )
            }))
        })
        .collect()
}

async fn via_mcp(steps: &[Step]) -> Vec<Value> {
    let (dir, wiki) = temp_wiki();
    let scrub = Scrub::of(&dir, &wiki);
    let client = mcp_client(wiki).await;
    let mut out = Vec::new();
    for s in steps {
        let Value::Object(args) = s.input.clone() else {
            unreachable!()
        };
        let result = client
            .call_tool(CallToolRequestParams::new(s.op).with_arguments(args))
            .await
            .unwrap();
        out.push(
            scrub.apply(
                result
                    .structured_content
                    .expect("tool results carry structured content"),
            ),
        );
    }
    out
}

async fn via_http(steps: &[Step]) -> Vec<Value> {
    let (dir, wiki) = temp_wiki();
    let scrub = Scrub::of(&dir, &wiki);
    let router = wikirs_http::router(&wiki, wikirs_http::Policy::loopback(0));
    let mut out = Vec::new();
    for s in steps {
        let (_, body) = http(&router, "POST", &format!("/ops/{}", s.op), Some(&s.input)).await;
        out.push(scrub.apply(body));
    }
    out
}

async fn mcp_client(wiki: Wiki) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let server = wikirs_mcp::WikiServer::new(wiki)
            .serve(server_io)
            .await
            .unwrap();
        server.waiting().await.unwrap();
    });
    ().serve(client_io).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn every_interface_behaves_identically() {
    let steps = scenario();
    let core: Vec<Value> = via_core(&steps).into_iter().map(normalize).collect();
    let mcp: Vec<Value> = via_mcp(&steps).await.into_iter().map(normalize).collect();
    let http: Vec<Value> = via_http(&steps).await.into_iter().map(normalize).collect();
    let [cli, cli_input] = [false, true].map(|as_json| {
        std::thread::spawn(move || via_cli(&scenario(), as_json))
            .join()
            .unwrap()
            .into_iter()
            .map(normalize)
            .collect::<Vec<Value>>()
    });
    for (i, step) in steps.iter().enumerate() {
        assert_eq!(
            cli[i], core[i],
            "step {i} ({}): CLI differs from core",
            step.op
        );
        assert_eq!(
            cli_input[i], core[i],
            "step {i} ({}): CLI with --input differs from core",
            step.op
        );
        assert_eq!(
            mcp[i], core[i],
            "step {i} ({}): MCP differs from core",
            step.op
        );
        // HTTP has no local file access (capability `local_fs: false`).
        if step.input.to_string().contains("local_path") {
            assert_eq!(
                http[i]["error"]["kind"], "invalid_input",
                "step {i} ({}): HTTP must refuse local paths",
                step.op
            );
        } else {
            assert_eq!(
                http[i], core[i],
                "step {i} ({}): HTTP differs from core",
                step.op
            );
        }
    }
    check_scenario(&steps, &core);
}

/// What the scenario's core results must show, beyond the Interfaces agreeing.
#[allow(clippy::too_many_lines)] // one flat list of assertions
fn check_scenario(steps: &[Step], core: &[Value]) {
    // The scenario really exercises the error kinds it claims to.
    let kinds: Vec<&str> = core
        .iter()
        .filter_map(|v| v["error"]["kind"].as_str())
        .collect();
    let at = |op: &str, page: Option<&str>| {
        steps
            .iter()
            .rposition(|s| s.op == op && page.is_none_or(|p| s.input["page"] == p))
            .unwrap()
    };
    assert_eq!(
        core[at("get_page", Some("notes"))]["result"]["content"],
        "# Notes\n\nv3\n",
        "writes and edits landed; the dry run didn't"
    );
    // After the moves: eng/rust's Links to the moved Page were rewritten, its Tags renamed.
    let rust = &core[at("get_page", Some("eng/rust"))]["result"];
    let rust_content = rust["content"].as_str().unwrap();
    assert!(
        rust_content.contains("[[lang/async]] [[lang/async#nope]] [[lang/async]]"),
        "{rust_content}"
    );
    assert_eq!(rust["tags"], json!(["language/rust", "language/unsafe"]));
    let status = &core[at("index_status", None)]["result"];
    assert_eq!(
        (
            status["pages"].as_i64(),
            status["links"].as_i64(),
            status["tags"].as_i64()
        ),
        (Some(3), Some(5), Some(2)),
        "3 Pages, 5 Links written, Tags lang/rust and lang/unsafe"
    );
    let check = &core[at("check", None)]["result"]["diagnostics"];
    let found: Vec<&str> = check
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["kind"].as_str())
        .collect();
    assert_eq!(
        found,
        // `[n](../notes.md)` from eng/rust.md is notes.md, which exists.
        [
            "heading_missing",
            "case_fallback",
            "broken_link",
            "mixed_case_tag"
        ]
    );
    assert_eq!(
        kinds,
        [
            "not_found",
            "already_exists",
            "case_conflict",
            "invalid_path",
            "conflict",
            "no_match",
            "ambiguous_match",
            "invalid_input",
            "invalid_input",
            "invalid_path",
            "not_found",
            "invalid_input",
            "invalid_input",
            "already_exists",
            "invalid_input",
            "not_found",
            "not_found",
            "already_exists",
            "not_found"
        ]
    );
    let gallery = &core[at("get_page", Some("gallery"))]["result"]["content"];
    assert_eq!(gallery, "![d](img/d.png)\n", "the Link followed the move");
    let deleted = &core[at("delete_attachment", None) - 1]["warnings"];
    assert_eq!(deleted[0]["kind"], "link_will_break", "{deleted}");
    let config = &core[at("get_config", None)]["result"]["settings"];
    let sources: Vec<(&str, &str)> = config
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["source"] != "default")
        .map(|s| (s["key"].as_str().unwrap(), s["source"].as_str().unwrap()))
        .collect();
    assert_eq!(
        sources,
        [
            ("links.syntax", "wiki"),
            ("ignore", "wiki"),
            ("serve.port", "machine")
        ]
    );
    let listed = &core[at("list_pages", None)]["result"]["pages"];
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["path"] != "eng/zeta"),
        "ignored at once: {listed}"
    );
    // eng/zeta was placed before eng/rust (`order: 5`) in the gap below it.
    let eng = &core[at("children", None)]["result"]["children"];
    assert_eq!(eng[0]["path"], "eng/zeta", "{eng}");
}

// ---------------------------------------------------------- contract snapshot

#[test]
fn catalogue_snapshot() {
    insta::assert_json_snapshot!("catalogue", wikirs_core::catalogue());
}
