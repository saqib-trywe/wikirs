//! The MCP Interface beyond tool parity (docs/spec/interfaces.md#mcp):
//! read-only mode, tool hints, resources, and `watch` as notifications.

use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use rmcp::{
    RoleClient, ServiceExt,
    model::{
        CallToolRequestParams, PaginatedRequestParams, ReadResourceRequestParams, ResourceContents,
        ServerNotification, SubscriptionFilter,
    },
    service::{RunningService, ServiceError},
};
use serde_json::{Value, json};
use wikirs_core::Wiki;
use wikirs_mcp::WikiServer;

fn temp_wiki() -> (tempfile::TempDir, Wiki) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("wiki")).unwrap();
    let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("cache")).unwrap();
    (dir, wiki)
}

async fn client(server: WikiServer) -> RunningService<RoleClient, ()> {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let server = server.serve(server_io).await.unwrap();
        server.waiting().await.unwrap();
    });
    ().serve(client_io).await.unwrap()
}

/// A client using the 2026-07-28 lifecycle (`server/discover`), which has
/// `subscriptions/listen`.
async fn modern_client(server: WikiServer) -> RunningService<RoleClient, ()> {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let server = server.serve(server_io).await.unwrap();
        server.waiting().await.unwrap();
    });
    rmcp::service::serve_client_with_lifecycle(
        (),
        client_io,
        rmcp::service::ClientLifecycleMode::Discover {
            preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
        },
    )
    .await
    .unwrap()
}

async fn call<S: rmcp::Service<RoleClient>>(
    client: &RunningService<RoleClient, S>,
    tool: &str,
    args: Value,
) -> Value {
    let Value::Object(args) = args else {
        unreachable!()
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args))
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true), "{tool}: {result:?}");
    result.structured_content.unwrap()
}

/// An `McpError`'s code and `data`.
fn mcp_error(err: ServiceError) -> (i32, Option<Value>) {
    match err {
        ServiceError::McpError(e) => (e.code.0, e.data),
        other => panic!("not an MCP error: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn read_only_lists_only_queries_and_maintenance() {
    let (_dir, wiki) = temp_wiki();
    let client = client(WikiServer::new(wiki).read_only()).await;
    let mut tools: Vec<String> = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    tools.sort();
    let mut expected: Vec<String> = wikirs_core::registry()
        .iter()
        .filter(|op| {
            matches!(
                op.kind,
                wikirs_core::Kind::Query | wikirs_core::Kind::Maintenance
            )
        })
        .map(|op| op.name.to_string())
        .collect();
    expected.sort();
    assert_eq!(tools, expected);
    assert!(tools.contains(&"rebuild_index".to_string()));

    let err = client
        .call_tool(
            CallToolRequestParams::new("create_page")
                .with_arguments(json!({ "path": "x" }).as_object().unwrap().clone()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        mcp_error(err).0,
        -32602,
        "a hidden mutation is an unknown tool"
    );
    assert!(
        client.list_all_resources().await.is_ok(),
        "resources stay available"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_carry_the_spec_hints() {
    let (_dir, wiki) = temp_wiki();
    let client = client(WikiServer::new(wiki)).await;
    let hints: Vec<(String, Option<bool>, Option<bool>)> = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| {
            let a = t.annotations.unwrap_or_default();
            (t.name.to_string(), a.read_only_hint, a.destructive_hint)
        })
        .collect();
    let hint = |name: &str| {
        let (_, read_only, destructive) = hints.iter().find(|(n, ..)| n == name).unwrap();
        (*read_only, *destructive)
    };
    assert_eq!(hint("get_page"), (Some(true), None));
    assert_eq!(hint("rebuild_index"), (None, None), "neither hint");
    assert_eq!(hint("write_page"), (Some(false), Some(false)));
    for name in [
        "delete_page",
        "delete_attachment",
        "move_page",
        "move_attachment",
        "rename_tag",
    ] {
        assert_eq!(hint(name), (Some(false), Some(true)), "{name}");
    }
    let destructive = hints.iter().filter(|(.., d)| *d == Some(true)).count();
    assert_eq!(destructive, 5, "no other tool is destructive");
}

#[tokio::test(flavor = "multi_thread")]
async fn pages_and_attachments_are_resources() {
    let (_dir, wiki) = temp_wiki();
    let client = client(WikiServer::new(wiki)).await;
    // More Pages than one listing page holds, then Attachments after them.
    for i in 0..105 {
        call(
            &client,
            "create_page",
            json!({ "path": format!("p/{i:03}") }),
        )
        .await;
    }
    call(&client, "create_page", json!({ "path": "eng" })).await;
    call(
        &client,
        "create_page",
        json!({ "path": "eng/rust notes", "content": "# Rust\n\nBody\n" }),
    )
    .await;
    for name in ["a.png", "b.bin"] {
        let bytes = STANDARD.encode([0u8, 1, 255]);
        call(
            &client,
            "add_attachment",
            json!({ "page": "eng", "name": name, "source": { "base64": bytes } }),
        )
        .await;
    }

    let mut uris = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let result = client
            .list_resources(Some(PaginatedRequestParams::default().with_cursor(cursor)))
            .await
            .unwrap();
        pages += 1;
        uris.extend(result.resources.iter().map(|r| r.uri.clone()));
        cursor = result.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(pages, 2);
    assert_eq!(uris.len(), 109, "every Page and Attachment exactly once");
    let mut unique = uris.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 109);
    assert_eq!(
        uris[..2],
        ["wiki://page/eng", "wiki://page/eng/rust%20notes"]
    );
    assert_eq!(
        uris[106..],
        [
            "wiki://page/p/104",
            "wiki://attachment/eng/a.png",
            "wiki://attachment/eng/b.bin"
        ],
        "Pages by path, then Attachments"
    );

    let templates = client.list_all_resource_templates().await.unwrap();
    let templates: Vec<&str> = templates.iter().map(|t| t.uri_template.as_str()).collect();
    assert_eq!(
        templates,
        ["wiki://page/{+path}", "wiki://attachment/{+path}"]
    );

    let read = |uri: &str| client.read_resource(ReadResourceRequestParams::new(uri));
    let page = read("wiki://page/eng/rust%20notes").await.unwrap();
    let ResourceContents::TextResourceContents {
        text, mime_type, ..
    } = &page.contents[0]
    else {
        panic!("a Page is text: {page:?}")
    };
    assert_eq!(
        (text.as_str(), mime_type.as_deref()),
        ("# Rust\n\nBody\n", Some("text/markdown"))
    );
    let file = read("wiki://attachment/eng/b.bin").await.unwrap();
    let ResourceContents::BlobResourceContents { blob, .. } = &file.contents[0] else {
        panic!("an Attachment is a blob: {file:?}")
    };
    assert_eq!(STANDARD.decode(blob).unwrap(), [0u8, 1, 255]);

    let (code, data) = mcp_error(read("wiki://page/missing").await.unwrap_err());
    // -32002, which rmcp reports as -32602 to 2026-07-28 peers (SEP-2164).
    assert!([-32002, -32602].contains(&code), "{code}");
    assert_eq!(data.unwrap()["kind"], "not_found");
    let (code, _) = mcp_error(read("wiki://tag/rust").await.unwrap_err());
    assert!([-32002, -32602].contains(&code), "{code}");
}

/// Collects notifications until none arrive for a short while.
async fn drain(sub: &mut rmcp::service::Subscription) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(Ok(Some(n))) = tokio::time::timeout(Duration::from_millis(500), sub.next()).await {
        out.push(match n {
            ServerNotification::ResourceUpdatedNotification(u) => {
                format!("updated {}", u.params.uri)
            }
            ServerNotification::ResourceListChangedNotification(_) => "list_changed".into(),
            other => format!("{other:?}"),
        });
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_drives_listen_notifications() {
    let (_dir, wiki) = temp_wiki();
    let client = modern_client(WikiServer::new(wiki)).await;
    call(&client, "create_page", json!({ "path": "a" })).await;
    call(&client, "create_page", json!({ "path": "c" })).await;

    let filter = SubscriptionFilter::builder()
        .resources_list_changed()
        .resource_subscriptions(["wiki://page/a", "wiki://page/b", "wiki://tag/x"])
        .build();
    let mut sub = client.peer().listen(filter).await.unwrap();
    assert_eq!(
        sub.acknowledged().resource_subscriptions.as_deref(),
        Some(&["wiki://page/a".to_string(), "wiki://page/b".to_string()][..]),
        "only wiki:// Page and Attachment URIs are accepted"
    );

    call(
        &client,
        "write_page",
        json!({ "page": "a", "content": "# A2\n" }),
    )
    .await;
    call(
        &client,
        "write_page",
        json!({ "page": "c", "content": "# C2\n" }),
    )
    .await;
    assert_eq!(
        drain(&mut sub).await,
        ["updated wiki://page/a"],
        "edits: only subscribed resources, and the listing is unchanged"
    );

    call(&client, "move_page", json!({ "from": "c", "to": "b" })).await;
    assert_eq!(
        drain(&mut sub).await,
        ["updated wiki://page/b", "list_changed"],
        "a move updates both paths; one list_changed per batch"
    );
}

/// A legacy client that records the notifications it receives.
#[derive(Clone)]
struct Recorder(tokio::sync::mpsc::UnboundedSender<String>);

impl rmcp::ClientHandler for Recorder {
    async fn on_resource_updated(
        &self,
        params: rmcp::model::ResourceUpdatedNotificationParam,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        let _ = self.0.send(format!("updated {}", params.uri));
    }

    async fn on_resource_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        let _ = self.0.send("list_changed".into());
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(deprecated)] // resources/subscribe is how legacy clients subscribe
async fn legacy_clients_subscribe_per_resource() {
    let (_dir, wiki) = temp_wiki();
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let server = WikiServer::new(wiki).serve(server_io).await.unwrap();
        server.waiting().await.unwrap();
    });
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let client = Recorder(tx).serve(client_io).await.unwrap();
    let mut drain = async || {
        let mut out = Vec::new();
        while let Ok(Some(n)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            out.push(n);
        }
        out
    };

    call(&client, "create_page", json!({ "path": "a" })).await;
    call(&client, "create_page", json!({ "path": "c" })).await;
    assert_eq!(
        drain().await,
        ["list_changed", "list_changed"],
        "no subscriptions yet"
    );

    let subscribe = |uri: &str| client.subscribe(rmcp::model::SubscribeRequestParams::new(uri));
    subscribe("wiki://page/a").await.unwrap();
    let (code, _) = mcp_error(subscribe("wiki://tag/x").await.unwrap_err());
    assert!([-32002, -32602].contains(&code), "{code}");

    call(
        &client,
        "write_page",
        json!({ "page": "a", "content": "# A2\n" }),
    )
    .await;
    call(
        &client,
        "write_page",
        json!({ "page": "c", "content": "# C2\n" }),
    )
    .await;
    assert_eq!(drain().await, ["updated wiki://page/a"]);

    client
        .unsubscribe(rmcp::model::UnsubscribeRequestParams::new("wiki://page/a"))
        .await
        .unwrap();
    call(
        &client,
        "write_page",
        json!({ "page": "a", "content": "# A3\n" }),
    )
    .await;
    assert_eq!(drain().await, [] as [&str; 0], "unsubscribed");
}
