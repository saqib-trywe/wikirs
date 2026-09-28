//! The parity test (docs/spec/interfaces.md#parity-test):
//! 1. coverage: every registry Operation is exposed by every adapter;
//! 2. behaviour: one scenario gives identical JSON through core, CLI and MCP;
//! 3. contract snapshot: the catalogue JSON.

use std::process::Command;

use rmcp::{ServiceExt, model::CallToolRequestParams};
use serde_json::{Value, json};
use wikirs_core::{Kind, Wiki};

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
    let dir = tempfile::tempdir().unwrap();
    let client = mcp_client(Wiki::open(dir.path()).unwrap()).await;
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
    ]
}

fn outcome(result: wikirs_core::Result<Value>) -> Value {
    result.unwrap_or_else(|e| e.to_json())
}

fn via_core(steps: &[Step]) -> Vec<Value> {
    let dir = tempfile::tempdir().unwrap();
    let wiki = Wiki::open(dir.path()).unwrap();
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
    let dir = tempfile::tempdir().unwrap();
    steps
        .iter()
        .map(|s| {
            let out = Command::new(env!("CARGO_BIN_EXE_wikirs"))
                .arg("--wiki")
                .arg(dir.path())
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
    let dir = tempfile::tempdir().unwrap();
    let client = mcp_client(Wiki::open(dir.path()).unwrap()).await;
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
    let core = via_core(&steps);
    let mcp = via_mcp(&steps).await;
    let cli = tokio::task::spawn_blocking(move || via_cli(&scenario()))
        .await
        .unwrap();
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
    assert_eq!(
        kinds,
        [
            "not_found",
            "already_exists",
            "case_conflict",
            "invalid_path"
        ]
    );
}

// ---------------------------------------------------------- contract snapshot

#[test]
fn catalogue_snapshot() {
    insta::assert_json_snapshot!("catalogue", wikirs_core::catalogue());
}
