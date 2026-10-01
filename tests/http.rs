//! `wikirs serve` (docs/spec/http-security.md, interfaces.md#http, errors.md#http):
//! the security middleware, statuses, read-only mode, `watch` over SSE and
//! `/mcp`, through the router; plus binding in a real process.

use std::time::Duration;

use axum::{Router, body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use wikirs_core::Wiki;
use wikirs_http::{Policy, router};

fn temp_wiki() -> (tempfile::TempDir, Wiki) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("wiki")).unwrap();
    let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("base")).unwrap();
    (dir, wiki)
}

/// A request with a loopback Host; `headers` add to or override it.
fn request(method: &str, uri: &str, headers: &[(&str, &str)], body: &str) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if !headers.iter().any(|(h, _)| h.eq_ignore_ascii_case("host")) {
        builder = builder.header("host", "localhost:4747");
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

const JSON: (&str, &str) = ("content-type", "application/json");

/// Status, the headers asked for, and the body as JSON (or null).
async fn send(router: &Router, request: Request<Body>) -> (u16, Value, axum::http::HeaderMap) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        headers,
    )
}

fn kind(body: &Value) -> &str {
    body["error"]["kind"].as_str().unwrap_or("")
}

#[tokio::test]
async fn ops_calls_map_errors_to_statuses() {
    let (_dir, wiki) = temp_wiki();
    let app = router(&wiki, Policy::loopback(4747));
    let post = |op: &str, body: &str| request("POST", &format!("/ops/{op}"), &[JSON], body);

    let (status, body, _) = send(&app, post("create_page", r#"{"path":"a"}"#)).await;
    assert_eq!(status, 200);
    assert!(
        body["result"].is_object() && body["warnings"].is_array(),
        "{body}"
    );
    let (status, body, _) = send(&app, post("index_status", "")).await;
    assert_eq!(
        (status, body["result"]["pages"].clone()),
        (200, json!(1)),
        "an empty body is `{{}}`"
    );

    for (op, input, want, want_kind) in [
        ("get_page", r#"{"page":"missing"}"#, 404, "not_found"),
        ("no_such_op", "{}", 404, "not_found"),
        ("create_page", r#"{"path":"a"}"#, 409, "already_exists"),
        ("create_page", r#"{"path":"a/../b"}"#, 400, "invalid_path"),
        ("get_page", "{not json", 400, "invalid_input"),
        ("get_page", r#"{"pgae":"a"}"#, 400, "invalid_input"),
        ("watch", "{}", 400, "invalid_input"),
        (
            "read_attachment",
            r#"{"path":"a/x.png","as":"local_path"}"#,
            400,
            "invalid_input",
        ),
    ] {
        let (status, body, _) = send(&app, post(op, input)).await;
        assert_eq!(
            (status, kind(&body)),
            (want, want_kind),
            "{op} {input}: {body}"
        );
    }
}

#[tokio::test]
async fn read_only_refuses_mutations_with_403() {
    let (_dir, wiki) = temp_wiki();
    let mut policy = Policy::loopback(4747);
    policy.read_only = true;
    let app = router(&wiki, policy);
    let (status, body, _) = send(
        &app,
        request("POST", "/ops/create_page", &[JSON], r#"{"path":"a"}"#),
    )
    .await;
    assert_eq!((status, kind(&body)), (403, "forbidden"));
    let (status, ..) = send(&app, request("POST", "/ops/list_pages", &[JSON], "{}")).await;
    assert_eq!(status, 200, "queries still work");
    let (status, ..) = send(&app, request("POST", "/ops/rebuild_index", &[JSON], "{}")).await;
    assert_eq!(status, 200, "so does maintenance");
}

#[tokio::test]
async fn the_guard_checks_host_origin_content_type_and_size() {
    let (_dir, wiki) = temp_wiki();
    let mut policy = Policy::loopback(4747);
    policy.cors_origins = vec!["http://localhost:3000".into()];
    policy.max_body = 64;
    policy.hosts.push("wiki.lan".into());
    let app = router(&wiki, policy);
    let list = |headers: &[(&str, &str)]| request("POST", "/ops/list_pages", headers, "{}");

    for host in [
        "localhost:4747",
        "127.0.0.1:4747",
        "[::1]:4747",
        "localhost",
        "WIKI.LAN:4747",
    ] {
        let (status, ..) = send(&app, list(&[JSON, ("host", host)])).await;
        assert_eq!(status, 200, "Host {host}");
    }
    for host in [
        "evil.example",
        "evil.example:4747",
        "localhost:9999",
        "localhost.evil.example",
    ] {
        let (status, body, _) = send(&app, list(&[JSON, ("host", host)])).await;
        assert_eq!((status, kind(&body)), (403, "forbidden"), "Host {host}");
    }

    let (status, body, _) = send(&app, list(&[JSON, ("origin", "https://evil.example")])).await;
    assert_eq!(
        (status, kind(&body)),
        (403, "forbidden"),
        "a page on another site"
    );
    let (status, _, headers) = send(&app, list(&[JSON, ("origin", "http://localhost:3000")])).await;
    assert_eq!(status, 200);
    assert_eq!(
        headers["access-control-allow-origin"],
        "http://localhost:3000"
    );
    let (status, _, headers) = send(
        &app,
        request(
            "OPTIONS",
            "/ops/list_pages",
            &[("origin", "http://localhost:3000")],
            "",
        ),
    )
    .await;
    assert_eq!(status, 204, "a preflight from an allowed origin");
    assert!(headers.contains_key("access-control-allow-headers"));

    for ct in [
        None,
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
    ] {
        let headers: Vec<(&str, &str)> = ct.map(|c| ("content-type", c)).into_iter().collect();
        let (status, body, _) = send(&app, list(&headers)).await;
        assert_eq!(
            (status, kind(&body)),
            (403, "forbidden"),
            "Content-Type {ct:?}"
        );
    }
    let (status, ..) = send(
        &app,
        list(&[("content-type", "application/json; charset=utf-8")]),
    )
    .await;
    assert_eq!(status, 200);

    let big = format!(r#"{{"page":"{}"}}"#, "x".repeat(100));
    let (status, body, _) = send(&app, request("POST", "/ops/get_page", &[JSON], &big)).await;
    assert_eq!((status, kind(&body)), (413, "payload_too_large"));
    let (status, body, _) = send(
        &app,
        request(
            "POST",
            "/mcp",
            &[JSON, ("accept", "application/json, text/event-stream")],
            &big,
        ),
    )
    .await;
    assert_eq!(
        (status, kind(&body)),
        (413, "payload_too_large"),
        "/mcp too, same shape"
    );
    let declared = request(
        "POST",
        "/ops/list_pages",
        &[JSON, ("content-length", "1000000")],
        "{}",
    );
    let (status, body, _) = send(&app, declared).await;
    assert_eq!(
        (status, kind(&body)),
        (413, "payload_too_large"),
        "a declared length over the limit is refused before the body is read"
    );
}

#[tokio::test]
async fn a_token_guards_ops_and_mcp_alike() {
    let (_dir, wiki) = temp_wiki();
    let mut policy = Policy::loopback(4747);
    policy.token = Some("s3cret".into());
    let app = router(&wiki, policy);
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": { "name": "t", "version": "0" } }
    })
    .to_string();
    let mcp = |auth: Option<&'static str>| {
        let mut headers = vec![JSON, ("accept", "application/json, text/event-stream")];
        headers.extend(auth.map(|a| ("authorization", a)));
        request("POST", "/mcp", &headers, &initialize)
    };
    let ops = |auth: Option<&'static str>| {
        let mut headers = vec![JSON];
        headers.extend(auth.map(|a| ("authorization", a)));
        request("POST", "/ops/list_pages", &headers, "{}")
    };

    for auth in [
        None,
        Some("Bearer wrong"),
        Some("s3cret"),
        Some("Basic s3cret"),
    ] {
        let (status, body, _) = send(&app, ops(auth)).await;
        assert_eq!(
            (status, kind(&body)),
            (401, "unauthorized"),
            "/ops with {auth:?}"
        );
        let (status, ..) = send(&app, mcp(auth)).await;
        assert_eq!(status, 401, "/mcp with {auth:?}");
    }
    let (status, ..) = send(&app, ops(Some("Bearer s3cret"))).await;
    assert_eq!(status, 200);
    let (status, ..) = send(&app, mcp(Some("Bearer s3cret"))).await;
    assert_eq!(status, 200, "/mcp is served once authorized");
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_streams_server_sent_events() {
    let (_dir, wiki) = temp_wiki();
    let app = router(&wiki, Policy::loopback(4747));
    let response = app
        .clone()
        .oneshot(request("GET", "/ops/watch?space=eng", &[], ""))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut body = response.into_body();

    let core = wiki.clone();
    tokio::task::spawn_blocking(move || {
        for path in ["notes", "eng/rust"] {
            let input = json!({ "path": path });
            wikirs_core::find("create_page")
                .unwrap()
                .call(&core, input)
                .unwrap();
        }
    })
    .await
    .unwrap();

    let mut text = String::new();
    while !text.contains("index_updated") {
        let frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
            .await
            .expect("an event in time")
            .unwrap()
            .unwrap();
        if let Ok(data) = frame.into_data() {
            text.push_str(&String::from_utf8_lossy(&data));
        }
    }
    let events: Vec<Value> = text
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .map(|d| serde_json::from_str(d).unwrap())
        .collect();
    assert_eq!(
        events,
        [
            json!({ "kind": "page_created", "path": "eng/rust",
                    "version": events[0]["version"] }),
            json!({ "kind": "index_updated" }),
        ],
        "only the scope's events, as JSON"
    );
}

// ------------------------------------------------------------- real process

/// A child process that's killed when this drops, so a failing assertion
/// can't leave it running (holding the test runner's output pipes open).
struct Reaped(std::process::Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `wikirs serve` on a free port: prints its URL, answers with the policy
/// it was given, and refuses a remote bind without `--allow-remote`.
#[test]
fn serve_binds_loopback_and_refuses_remote_without_the_flag() {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpStream,
        process::{Command, Stdio},
    };

    let (dir, _wiki) = temp_wiki();
    let cmd = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wikirs"));
        cmd.env("WIKIRS_CACHE_DIR", dir.path().join("base"))
            .env("WIKIRS_CONFIG_DIR", dir.path().join("base/config"))
            .env("WIKIRS_STATE_DIR", dir.path().join("base/state"))
            .arg("--wiki")
            .arg(dir.path().join("wiki"))
            .args(args);
        cmd
    };

    // If it didn't refuse, it would serve forever: give it a deadline.
    let mut refusing = Reaped(
        cmd(&["serve", "--bind", "0.0.0.0", "--port", "0"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = refusing.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "a non-loopback bind without --allow-remote must not serve"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(!status.success());
    let mut stderr = String::new();
    refusing
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let refused = std::process::Output {
        status,
        stdout: Vec::new(),
        stderr: stderr.into_bytes(),
    };
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--allow-remote"));

    let token = cmd(&["serve", "--print-token"]).output().unwrap();
    let token = String::from_utf8(token.stdout).unwrap().trim().to_string();
    assert_eq!(token.len(), 64);
    let again = cmd(&["serve", "--print-token"]).output().unwrap();
    assert_eq!(String::from_utf8(again.stdout).unwrap().trim(), token);
    let rotated = cmd(&["serve", "--rotate-token"]).output().unwrap();
    assert_ne!(String::from_utf8(rotated.stdout).unwrap().trim(), token);

    let mut child = Reaped(
        cmd(&["serve", "--port", "0"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut banner = String::new();
    BufReader::new(child.0.stdout.take().unwrap())
        .read_line(&mut banner)
        .unwrap();
    let url = banner
        .split_whitespace()
        .find(|w| w.starts_with("http://"))
        .unwrap_or_else(|| panic!("no URL in {banner:?}"));
    let addr = url.trim_start_matches("http://");
    assert!(
        addr.starts_with("127.0.0.1:"),
        "loopback by default: {addr}"
    );
    let port = addr.rsplit(':').next().unwrap();

    let get = |host: &str| {
        let mut stream = TcpStream::connect(addr).unwrap();
        write!(
            stream,
            "GET /ops HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    let ok = get(&format!("localhost:{port}"));
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.contains("\"operations\""));
    let rebound = get("attacker.example");
    assert!(rebound.starts_with("HTTP/1.1 403"), "{rebound}");
}
