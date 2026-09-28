//! The parity test (docs/spec/interfaces.md#parity-test):
//! 1. coverage: every registry Operation is exposed by every adapter;
//! 2. behaviour: one scenario gives identical JSON through core, CLI and MCP;
//! 3. contract snapshot: the catalogue JSON.

use std::process::Command;

use rmcp::{ServiceExt, model::CallToolRequestParams};
use serde_json::{Value, json};
use wikirs_core::{Kind, Wiki};

/// A fresh Wiki whose cache dir (lock, journal) also lives in the tempdir.
fn temp_wiki() -> (tempfile::TempDir, Wiki) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("wiki")).unwrap();
    let wiki = Wiki::open_with_cache(dir.path().join("wiki"), dir.path().join("cache")).unwrap();
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
            op: "index_status",
            input: json!({}),
            cli: &["index-status"],
        },
        Step {
            op: "rebuild_index",
            input: json!({}),
            cli: &["rebuild-index"],
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

fn outcome(result: wikirs_core::Result<Value>) -> Value {
    result.unwrap_or_else(|e| e.to_json())
}

fn via_core(steps: &[Step]) -> Vec<Value> {
    let (_dir, wiki) = temp_wiki();
    steps
        .iter()
        .map(|s| {
            outcome(
                wikirs_core::find(s.op)
                    .unwrap()
                    .call(&wiki, s.input.clone()),
            )
        })
        .collect()
}

fn via_cli(steps: &[Step]) -> Vec<Value> {
    let (dir, _wiki) = temp_wiki();
    steps
        .iter()
        .map(|s| {
            let out = Command::new(env!("CARGO_BIN_EXE_wikirs"))
                .env("WIKIRS_CACHE_DIR", dir.path().join("cache"))
                .arg("--wiki")
                .arg(dir.path().join("wiki"))
                .arg("--json")
                .args(s.cli)
                .output()
                .unwrap();
            serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
                panic!(
                    "{}: bad JSON ({e}): {}",
                    s.op,
                    String::from_utf8_lossy(&out.stdout)
                )
            })
        })
        .collect()
}

async fn via_mcp(steps: &[Step]) -> Vec<Value> {
    let (_dir, wiki) = temp_wiki();
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
            result
                .structured_content
                .expect("tool results carry structured content"),
        );
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
    let cli: Vec<Value> = tokio::task::spawn_blocking(move || via_cli(&scenario()))
        .await
        .unwrap()
        .into_iter()
        .map(normalize)
        .collect();
    for (i, step) in steps.iter().enumerate() {
        assert_eq!(
            cli[i], core[i],
            "step {i} ({}): CLI differs from core",
            step.op
        );
        assert_eq!(
            mcp[i], core[i],
            "step {i} ({}): MCP differs from core",
            step.op
        );
    }
    // The scenario really exercises the error kinds it claims to.
    let kinds: Vec<&str> = core
        .iter()
        .filter_map(|v| v["error"]["kind"].as_str())
        .collect();
    let content = core
        .iter()
        .rev()
        .find_map(|v| v["result"]["content"].as_str())
        .unwrap();
    assert_eq!(
        content, "# Notes\n\nv3\n",
        "writes and edits landed; the dry run didn't"
    );
    let status = &core[core.len() - 2]["result"];
    assert_eq!(
        (status["pages"].as_i64(), status["attachments"].as_i64()),
        (Some(2), Some(0))
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
            "invalid_input"
        ]
    );
}

// ---------------------------------------------------------- contract snapshot

#[test]
fn catalogue_snapshot() {
    insta::assert_json_snapshot!("catalogue", wikirs_core::catalogue());
}
