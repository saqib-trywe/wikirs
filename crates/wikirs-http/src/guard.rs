//! The one middleware in front of `/ops` and `/mcp` (http-security.md): Host,
//! Origin and CORS, bearer token, Content-Type, body size. Rejections happen
//! before any Operation runs and use the transport error kinds (errors.md).

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::token;

/// What every request is checked against.
#[derive(Debug, Clone)]
pub struct Policy {
    /// `Host` names accepted (lowercase, without port): loopback names plus
    /// `serve.allowed_hosts`.
    pub hosts: Vec<String>,
    /// The port served on: a `Host` with another port is refused.
    pub port: u16,
    /// Browser origins allowed to call (`serve.cors_origins`); others get 403.
    pub cors_origins: Vec<String>,
    /// The bearer token every request must carry, if auth is on.
    pub token: Option<String>,
    pub max_body: usize,
    /// `--read-only`: no mutations (checked by the routes, not here).
    pub read_only: bool,
}

impl Policy {
    /// The loopback-only policy: no token, no browser origins.
    #[must_use]
    pub fn loopback(port: u16) -> Self {
        Self {
            hosts: LOOPBACK.iter().map(|h| (*h).to_string()).collect(),
            port,
            cors_origins: Vec::new(),
            token: None,
            max_body: 32 << 20,
            read_only: false,
        }
    }
}

pub const LOOPBACK: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// A transport error: same body shape as an Operation error, kinds outside
/// the Operation set.
pub fn reject(status: StatusCode, kind: &str, message: impl Into<String>) -> Response {
    let body = json!({ "error": { "kind": kind, "message": message.into(), "details": {} } });
    (status, axum::Json(body)).into_response()
}

/// `Host` without its port, and the port if there is one.
fn split_host(host: &str) -> (String, Option<u16>) {
    let host = host.trim().to_ascii_lowercase();
    // `[::1]:4747`, `[::1]`, `localhost:4747`, `localhost`
    let (name, port) = match host.rfind(':') {
        Some(i) if !host[i..].contains(']') => (&host[..i], host[i + 1..].parse().ok()),
        _ => (host.as_str(), None),
    };
    (name.to_string(), port)
}

fn header(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

pub async fn guard(State(policy): State<Arc<Policy>>, request: Request, next: Next) -> Response {
    let headers = request.headers();

    // DNS rebinding: a page on another name resolving to us gets nowhere.
    let host = header(headers, header::HOST).map(split_host);
    let host_ok = host.as_ref().is_some_and(|(name, port)| {
        policy.hosts.contains(name) && port.is_none_or(|p| p == policy.port)
    });
    if !host_ok {
        return reject(StatusCode::FORBIDDEN, "forbidden", "Host is not allowed");
    }

    let origin = match header(headers, header::ORIGIN) {
        None => None,
        Some(o) if policy.cors_origins.iter().any(|a| a == o) => Some(o.to_string()),
        Some(o) => {
            return reject(
                StatusCode::FORBIDDEN,
                "forbidden",
                format!("Origin `{o}` is not in serve.cors_origins"),
            );
        }
    };
    if request.method() == Method::OPTIONS && origin.is_some() {
        return with_cors(StatusCode::NO_CONTENT.into_response(), origin.as_deref());
    }

    if let Some(token) = &policy.token {
        let given = header(headers, header::AUTHORIZATION).and_then(|v| v.strip_prefix("Bearer "));
        if !given.is_some_and(|g| token::matches(g, token)) {
            return with_cors(
                reject(
                    StatusCode::UNAUTHORIZED,
                    "unauthorized",
                    "missing or wrong bearer token",
                ),
                origin.as_deref(),
            );
        }
    }

    let is_op_call = request.method() == Method::POST && request.uri().path().starts_with("/ops/");
    if is_op_call {
        let json = header(headers, header::CONTENT_TYPE).is_some_and(|ct| {
            ct.split(';')
                .next()
                .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
        });
        if !json {
            return with_cors(
                reject(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Content-Type must be application/json",
                ),
                origin.as_deref(),
            );
        }
    }

    let declared = header(headers, header::CONTENT_LENGTH).and_then(|l| l.parse::<usize>().ok());
    if declared.is_some_and(|len| len > policy.max_body) {
        return with_cors(too_large(policy.max_body), origin.as_deref());
    }

    let response = next.run(request).await;
    // A body that turned out too large while streaming (no or chunked
    // Content-Length): axum's and rmcp's 413s get the same body as ours.
    let response = if response.status() == StatusCode::PAYLOAD_TOO_LARGE {
        too_large(policy.max_body)
    } else {
        response
    };
    with_cors(response, origin.as_deref())
}

#[must_use]
pub fn too_large(max_body: usize) -> Response {
    reject(
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
        format!("the body is over serve.max_body ({max_body} bytes)"),
    )
}

/// Adds CORS headers for an allowed origin.
fn with_cors(mut response: Response<Body>, origin: Option<&str>) -> Response<Body> {
    let Some(origin) = origin.and_then(|o| HeaderValue::from_str(o).ok()) else {
        return response;
    };
    let headers = response.headers_mut();
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, DELETE, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(
            "authorization, content-type, accept, mcp-protocol-version, mcp-session-id, last-event-id",
        ),
    );
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("mcp-session-id, retry-after"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_split_into_name_and_port() {
        let split = |h: &str| split_host(h);
        assert_eq!(split("localhost:4747"), ("localhost".into(), Some(4747)));
        assert_eq!(split("LOCALHOST"), ("localhost".into(), None));
        assert_eq!(split("[::1]:4747"), ("[::1]".into(), Some(4747)));
        assert_eq!(split("[::1]"), ("[::1]".into(), None));
        assert_eq!(split("127.0.0.1:80"), ("127.0.0.1".into(), Some(80)));
    }
}
