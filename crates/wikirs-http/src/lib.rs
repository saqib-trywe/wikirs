//! The HTTP Interface, `wikirs serve` (docs/spec/interfaces.md#http,
//! http-security.md): RPC routes generated from the registry, `watch` as
//! Server-Sent Events, and MCP Streamable HTTP at `/mcp`, all behind one
//! security middleware.

use std::{
    convert::Infallible,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State, rejection::BytesRejection},
    http::{HeaderValue, StatusCode, header},
    middleware,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::Value;
use tokio_stream::{StreamExt, wrappers::UnboundedReceiverStream};
use wikirs_core::{Error, ErrorKind, Kind, Wiki, index::Scope};
use wikirs_mcp::WikiServer;

pub mod guard;
pub mod token;

pub use guard::Policy;

struct App {
    wiki: Wiki,
    read_only: bool,
    max_body: usize,
}

/// The whole HTTP surface for `wiki` under `policy`. `wiki` should already
/// be watching (long-lived process) for `watch` to see other writers.
pub fn router(wiki: &Wiki, policy: Policy) -> Router {
    // Remote callers can't name paths on this machine (interfaces.md#capabilities).
    let wiki = wiki.clone().without_local_fs();
    let app = Arc::new(App {
        wiki: wiki.clone(),
        read_only: policy.read_only,
        max_body: policy.max_body,
    });

    let mcp_server = WikiServer::new(wiki);
    let mcp_server = if policy.read_only {
        mcp_server.read_only()
    } else {
        mcp_server
    };
    // rmcp checks Host and Origin too; give it the same lists.
    let mut mcp_config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(policy.hosts.iter().map(|h| h.trim_matches(['[', ']'])));
    mcp_config.allowed_origins.clone_from(&policy.cors_origins);
    mcp_config.max_request_body_bytes = policy.max_body;
    let mcp = StreamableHttpService::new(
        move || Ok(mcp_server.session()),
        Arc::new(LocalSessionManager::default()),
        mcp_config,
    );

    let policy = Arc::new(policy);
    Router::new()
        .route("/ops", get(catalogue))
        // `POST /ops/watch` is still an Operation call (that fails, as on every Interface).
        .route(
            "/ops/watch",
            get(watch).post(|app, body| call(app, Path("watch".to_string()), body)),
        )
        .route("/ops/{name}", post(call))
        .with_state(app)
        .nest_service("/mcp", mcp)
        .layer(DefaultBodyLimit::max(policy.max_body))
        .layer(middleware::from_fn_with_state(policy.clone(), guard::guard))
}

/// `GET /ops`: the catalogue, without mutations on a read-only server.
async fn catalogue(State(app): State<Arc<App>>) -> Json<Value> {
    let mut catalogue = wikirs_core::catalogue();
    if app.read_only
        && let Some(ops) = catalogue["operations"].as_array_mut()
    {
        ops.retain(|op| op["kind"] != "mutation");
    }
    Json(catalogue)
}

/// `POST /ops/{name}`: JSON Input in, `{ result, warnings }` or an error out.
async fn call(
    State(app): State<Arc<App>>,
    Path(name): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let body = match body {
        Ok(body) => body,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return guard::too_large(app.max_body);
        }
        Err(rejection) => return op_error(&Error::invalid_input(None, rejection.body_text())),
    };
    let Some(op) = wikirs_core::find(&name) else {
        return op_error(&Error::not_found("operation", &name));
    };
    if app.read_only && op.kind == Kind::Mutation {
        return guard::reject(
            StatusCode::FORBIDDEN,
            "forbidden",
            format!("`{name}` changes the Wiki, and this server is read-only"),
        );
    }
    let input: Value = if body.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        match serde_json::from_slice(&body) {
            Ok(input) => input,
            Err(e) => return op_error(&Error::invalid_input(None, format!("malformed JSON: {e}"))),
        }
    };
    let app = app.clone();
    let outcome = tokio::task::spawn_blocking(move || op.call(&app.wiki, input)).await;
    match outcome {
        Ok(Ok(envelope)) => Json(envelope).into_response(),
        Ok(Err(err)) => op_error(&err),
        Err(join) => op_error(&Error::internal(join.to_string())),
    }
}

/// An Operation error with its status (errors.md#http).
fn op_error(err: &Error) -> Response {
    let lock_timeout = err.kind == ErrorKind::Conflict && err.details["reason"] == "lock_timeout";
    let status = match err.kind {
        _ if lock_timeout => StatusCode::SERVICE_UNAVAILABLE,
        ErrorKind::InvalidInput | ErrorKind::InvalidPath => StatusCode::BAD_REQUEST,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::AlreadyExists | ErrorKind::CaseConflict | ErrorKind::Conflict => {
            StatusCode::CONFLICT
        }
        ErrorKind::NoMatch | ErrorKind::AmbiguousMatch => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::Io | ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut response = (status, Json(err.to_json())).into_response();
    if lock_timeout {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    response
}

/// `GET /ops/watch?space=…&path_prefix=…`: one SSE `data:` line (a JSON
/// event) per change, until the client disconnects.
async fn watch(
    State(app): State<Arc<App>>,
    Query(scope): Query<Scope>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let events = app.wiki.watch(scope);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    // The core's channel blocks: forward it from a thread, which ends at the
    // first event after the client is gone.
    let _ = std::thread::Builder::new()
        .name("wikirs-http-watch".into())
        .spawn(move || {
            for event in events {
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
    let stream = UnboundedReceiverStream::new(rx).map(|event| {
        Ok(Event::default()
            .json_data(&event)
            .unwrap_or_else(|_| Event::default()))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

// --------------------------------------------------------------------- serve

/// `wikirs serve`'s flags; each overrides its `[serve]` setting.
#[derive(Debug, Default, Clone)]
pub struct ServeOptions {
    pub bind: Option<String>,
    pub port: Option<u16>,
    pub allow_remote: bool,
    pub read_only: bool,
}

/// An IP as it appears in a URL or `Host` (`[::1]` for IPv6).
fn url_host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

/// Where to listen, and whether that's loopback only.
fn addresses(bind: Option<&str>, port: u16) -> anyhow::Result<(Vec<SocketAddr>, bool)> {
    let Some(bind) = bind.filter(|b| *b != "localhost") else {
        return Ok((
            vec![
                SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
                SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port),
            ],
            true,
        ));
    };
    let ip: IpAddr = bind
        .trim_matches(['[', ']'])
        .parse()
        .map_err(|_| anyhow::anyhow!("--bind `{bind}` is not an IP address"))?;
    Ok((vec![SocketAddr::new(ip, port)], ip.is_loopback()))
}

/// The checks and listening addresses for `wiki` under `options` and its
/// `[serve]` settings. Fails if a non-loopback bind lacks `--allow-remote`.
/// Returns the policy, addresses, and a newly generated token to print.
pub fn plan(
    wiki: &Wiki,
    options: &ServeOptions,
) -> anyhow::Result<(Policy, Vec<SocketAddr>, Option<String>)> {
    let settings = wiki.settings().serve();
    let bind = options.bind.clone().or(settings.bind);
    let port = options.port.unwrap_or(settings.port);
    let (addrs, loopback) = addresses(bind.as_deref(), port)?;
    if !loopback && !options.allow_remote {
        anyhow::bail!(
            "refusing to listen on non-loopback `{}` without --allow-remote",
            bind.unwrap_or_default()
        );
    }
    let mut policy = Policy::loopback(port);
    policy.hosts.extend(
        settings
            .allowed_hosts
            .iter()
            .map(|h| h.to_ascii_lowercase()),
    );
    if !loopback && let Some(addr) = addrs.first().filter(|a| !a.ip().is_unspecified()) {
        policy.hosts.push(url_host(addr.ip()));
    }
    policy.cors_origins = settings.cors_origins;
    policy.max_body = settings.max_body;
    policy.read_only = options.read_only || settings.read_only;
    let mut fresh = None;
    if !loopback || settings.require_token {
        let (token, new) = token::get_or_create(wiki)?;
        if new {
            fresh = Some(token.clone());
        }
        policy.token = Some(token);
    }
    Ok((policy, addrs, fresh))
}

/// Serves `wiki` until Ctrl-C. Prints the URL (and a newly generated token)
/// on stdout.
pub fn serve(wiki: &Wiki, options: &ServeOptions) -> anyhow::Result<()> {
    let (mut policy, addrs, fresh) = plan(wiki, options)?;
    // A long-lived process watches, so `watch` and queries see every writer.
    let _ = wiki.start_watcher();
    tokio::runtime::Runtime::new()?.block_on(async move {
        // Bind the first address (port 0 picks one), then the rest on that port.
        let first = tokio::net::TcpListener::bind(addrs[0]).await?;
        let port = first.local_addr()?.port();
        let mut listeners = vec![first];
        for addr in &addrs[1..] {
            // `[::1]` is best effort: a host without IPv6 still serves IPv4.
            if let Ok(l) = tokio::net::TcpListener::bind(SocketAddr::new(addr.ip(), port)).await {
                listeners.push(l);
            }
        }
        policy.port = port;
        let remote = !addrs[0].ip().is_loopback();
        let router = router(wiki, policy);

        let shown = url_host(addrs[0].ip());
        println!(
            "wikirs: serving {} at http://{shown}:{port} (MCP at /mcp)",
            wiki.root().display()
        );
        if let Some(token) = fresh {
            println!("token: {token}");
        }
        if remote {
            eprintln!(
                "warning: traffic is unencrypted; use an SSH tunnel, Tailscale or a TLS proxy"
            );
        }

        let mut tasks = tokio::task::JoinSet::new();
        for listener in listeners {
            let router = router.clone();
            tasks.spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async {
                        let _ = tokio::signal::ctrl_c().await;
                    })
                    .await
            });
        }
        while let Some(done) = tasks.join_next().await {
            done??;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_follow_the_spec() {
        let status = |e: Error| op_error(&e).status().as_u16();
        assert_eq!(status(Error::invalid_input(None, "x")), 400);
        assert_eq!(status(Error::not_found("page", "x")), 404);
        assert_eq!(status(Error::already_exists("x")), 409);
        assert_eq!(status(Error::internal("x")), 500);
        let busy = op_error(&Error::conflict_lock_timeout());
        assert_eq!(busy.status().as_u16(), 503);
        assert_eq!(busy.headers()[header::RETRY_AFTER], "1");
    }

    #[test]
    fn binding_is_loopback_unless_named() {
        let (addrs, loopback) = addresses(None, 4747).unwrap();
        assert!(loopback);
        assert_eq!(addrs.len(), 2, "127.0.0.1 and [::1]");
        assert!(addresses(Some("localhost"), 1).unwrap().1);
        assert!(addresses(Some("::1"), 1).unwrap().1);
        assert!(!addresses(Some("0.0.0.0"), 1).unwrap().1);
        assert!(!addresses(Some("192.168.1.5"), 1).unwrap().1);
        assert!(addresses(Some("my-host"), 1).is_err());
    }
}
