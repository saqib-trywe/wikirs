# Rust MCP SDK choice and transports — findings

Ticket: [02-rust-mcp-sdk](../issues/02-rust-mcp-sdk.md). Researched 2026-09-26 against crates.io, the GitHub source trees (shallow clones of `main`), and modelcontextprotocol.io.

## Spec baseline

- The **current** MCP revision is **2026-07-28**; `2025-11-25` and earlier are the handshake-based (`initialize`) revisions. 2026-07-28 is per-request versioned (`_meta` `io.modelcontextprotocol/protocolVersion`, plus `MCP-Protocol-Version` header on HTTP) and adds `server/discover`. Servers MAY support several versions at once. — <https://modelcontextprotocol.io/specification/versioning>
- 2026-07-28 Streamable HTTP is stateless (no `Mcp-Session-Id`, no standalone GET stream); `resources/subscribe` is replaced by `subscriptions/listen`. — rmcp README "Subscriptions" / "Stateless Streamable HTTP" sections: <https://github.com/modelcontextprotocol/rust-sdk/blob/main/README.md>

## Candidates (crates.io, 2026-09-26)

| Crate | Latest | Last publish | Recent downloads (90d) | Repo |
|---|---|---|---|---|
| `rmcp` (official) | 3.4.1 | 2026-09-23 | ~15.4M | <https://github.com/modelcontextprotocol/rust-sdk> |
| `rust-mcp-sdk` | 2.0.0 | 2026-08-27 | ~110k | <https://github.com/rust-mcp-stack/rust-mcp-sdk> |
| `mcp-server` | 0.1.0 | 2025-02-27 | ~1k | (old placeholder in the official repo; dead) |
| `mcpr` | 0.2.3 | 2025-03-16 | ~1.4k | <https://github.com/conikeec/mcpr> (stale) |

Source: `https://crates.io/api/v1/crates/<name>`. Only `rmcp` and `rust-mcp-sdk` are live options.

## rmcp (official SDK)

- **Status:** Tier 1 under SEP-1730; stable ≥1.0 since v3.0.0 (released 2026-07-28); 100% server/client conformance on both the `2025-11-25` and `2026-07-28` suites; has `VERSIONING.md` breaking-change policy. — [ROADMAP.md](https://github.com/modelcontextprotocol/rust-sdk/blob/main/ROADMAP.md), [VERSIONING.md](https://github.com/modelcontextprotocol/rust-sdk/blob/main/VERSIONING.md)
- **Spec versions:** `ProtocolVersion` consts for 2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25, 2026-07-28. Note `ProtocolVersion::LATEST` (the default) is still `V_2025_11_25`; 2026-07-28 is served when the client asks for it (stateless automatically). — `crates/rmcp/src/model.rs` lines ~170–175
- **Transports** (Cargo features): stdio (`transport-io`, `rmcp::transport::stdio()`), Streamable HTTP server (`transport-streamable-http-server`: `StreamableHttpService` is a **Tower service** mountable in axum/hyper, e.g. `axum::Router::new().nest_service("/mcp", service)`), Streamable HTTP client (reqwest), child-process client, in-process worker. Legacy 2024-11-05 HTTP+SSE is deliberately **not** provided. Options: `with_json_response(true)`, `with_legacy_session_mode(false)`. — README "Transports" section
- **Declaring tools:** macros `#[tool]`, `#[tool_router]`, `#[tool_handler]` (plus `#[tool_router(server_handler)]` shortcut); prompts `#[prompt]`, `#[prompt_router]`, `#[prompt_handler]`. **There is no resource macro** — resources are implemented by overriding `ServerHandler::list_resources` / `read_resource` / `list_resource_templates`. — `crates/rmcp-macros/src/lib.rs`; README "Resources"
- **Runtime registration (key question): yes, first-class.**
  - `ToolRouter<S>` is a runtime value: `new()`, `add_route`, `with_route`, `merge`, `remove_route`, `has_route`, `disable_route`/`enable_route`, `list_all`, `get`, `call`. — `crates/rmcp/src/handler/server/router/tool.rs`
  - `ToolRoute::new_dyn(tool: Tool, f)` takes a closure `Fn(ToolCallContext) -> BoxFuture<Result<CallToolResponse, ErrorData>>`; `ToolCallContext` exposes `name` and `arguments: Option<JsonObject>` (raw JSON). So one generic closure per Operation can deserialize/dispatch into a registry.
  - `Tool::new(name, description, input_schema: JsonObject)` accepts an arbitrary JSON Schema object; `with_input_schema::<T: JsonSchema>()` / `with_output_schema::<T>()` derive from schemars; `with_raw_output_schema` for raw. — `crates/rmcp/src/model/tool.rs`
  - Alternatively skip the router entirely and override `ServerHandler::list_tools` / `call_tool` (and `get_tool` for argument validation) by hand. — `crates/rmcp/src/handler/server.rs`
  - `ToolRouter::bind_peer_notifier` emits `notifications/tools/list_changed` when visible tools change; peers also have `notify_{tool,prompt,resource}_list_changed()`.
  - No example in `examples/` uses `new_dyn`; it is public API but less documented than the macro path.
- **JSON Schema:** `schemars` **1.x** (optional dep, enabled by default via the `server` feature). Parameter types derive `serde::Deserialize + schemars::JsonSchema` and are wrapped in `Parameters<T>`. README notes schemars quirks (e.g. enums need `#[schemars(extend("type"="string"))]`).
- **Runtime:** tokio; a `local` feature exists for non-`Send` handlers. MSRV 1.88.

## rust-mcp-sdk (rust-mcp-stack, community)

- **Spec:** 2.x implements **only 2026-07-28** (stateless); 1.x (LTS branch `release-1.x`) implements 2025-11-25. Claims 100% conformance (110/110 server, 440/440 client). — [README](https://github.com/rust-mcp-stack/rust-mcp-sdk/blob/main/README.md)
- **Transports:** stdio, Streamable HTTP (separate `rust-mcp-axum` and `rust-mcp-actix` backend crates, plus BYO-server embedding), backward-compatible SSE; POST-only under 2026-07-28 (GET/DELETE → 405).
- **Declaring:** `#[mcp_tool]` on a param struct generates a `Tool`; `tool_box!` builds an enum of tools; `mcp_resource`, `mcp_prompt`, `mcp_elicit` macros exist. Crucially, the server handler **returns `Vec<Tool>` from `handle_list_tools_request` and matches on `params.name` in `handle_call_tool_request`** — i.e. listing/dispatch is plain runtime code, so registry-driven tools are natural. `ServerHandlerCore` gives full raw dispatch.
- **JSON Schema:** its **own** `rust_mcp_macros::JsonSchema` derive, **not schemars** — nested structs must also derive it. Types come from the separate `rust-mcp-schema` crate.
- Smaller user base (~110k vs ~15M recent downloads), single-org community project.

## Pages as resources as well as tools?

Facts only (decision is the user's):

- Spec: resources are **application-driven** ("host applications determining how to incorporate context"), e.g. shown in a picker; tools are model-controlled. Resources are URI-identified, support `resources/templates/list` with RFC 6570 URI templates (e.g. `wiki://page/{slug}`), `listChanged`, and change subscriptions (via `subscriptions/listen` in 2026-07-28). — <https://modelcontextprotocol.io/specification/2025-11-25/server/resources>
- Implication for parity: a `read_page` **tool** is invokable by the model in every client; a Page **resource** gives the user/host a browsable, attachable view and change notifications, but host support and UX vary by client. Resources are read-only — writes still need tools.
- In rmcp there is no resource router/macro; exposing Pages as resources means hand-writing `list_resources` / `read_resource` / `list_resource_templates` that call the same Operations (straightforward, but outside any registry-generated tool surface). A resource is naturally a projection of an existing read Operation rather than a new Operation.

## Trade-offs summary

| | rmcp | rust-mcp-sdk |
|---|---|---|
| Governance | Official, Tier 1, versioning policy | Community |
| Spec | 2024-11-05 → 2026-07-28 (multi-version) | 2.x: 2026-07-28 only; 1.x: 2025-11-25 |
| HTTP | Tower service (fits axum shared with the HTTP interface) | axum or actix backend crates |
| Runtime tools | `ToolRouter` + `ToolRoute::new_dyn` + raw `Tool::new(schema)`, or manual `list_tools`/`call_tool` | Manual `Vec<Tool>` list + match on name |
| Schema | schemars 1.x | Own derive (not schemars) |
| Resources | Manual handler methods | `mcp_resource` macro + handler |

If wikirs' Operation registry derives schemas with schemars (likely shared with HTTP/OpenAPI), rmcp consumes them directly; rust-mcp-sdk would need schemars output converted into its `rust-mcp-schema` `Tool` input-schema type (not verified how cleanly).
