# Rust MCP SDK choice and transports

Map: [wikirs map](../map.md)
Type: research
Status: resolved
Blocked by: —

## Question

Which Rust MCP server SDK should wikirs use? Compare the official `rmcp` crate with the alternatives on: spec version supported, stdio vs streamable HTTP transports, how tools, resources and prompts are declared (macros vs registration at runtime), and generating JSON Schema from serde/schemars types. The key question for parity: **can MCP tools be registered at runtime from an Operation registry**, or only with static macros? Also: should Pages be exposed as MCP *resources* as well as tools?

## Answer

- Only two live options: official `rmcp` 3.4.1 (Tier 1, stable since 3.0.0 on 2026-07-28, ~15M recent downloads) and community `rust-mcp-sdk` 2.0.0 (~110k).
- Current MCP spec is 2026-07-28 (stateless, per-request versioning). `rmcp` supports 2024-11-05 through 2026-07-28 and passes 100% conformance on 2025-11-25 and 2026-07-28; `rust-mcp-sdk` 2.x covers only 2026-07-28 (1.x LTS covers 2025-11-25).
- Transports: both do stdio + Streamable HTTP. `rmcp`'s HTTP server is a Tower service you mount on axum; it has no legacy HTTP+SSE.
- **Runtime registration works in both.** `rmcp`: `ToolRouter::add_route` plus `ToolRoute::new_dyn(Tool::new(name, desc, json_schema), closure)` where the closure gets raw JSON `arguments`, or you override `list_tools`/`call_tool` by hand. Macros (`#[tool_router]`) are optional. `rust-mcp-sdk`: the handler returns `Vec<Tool>` and matches on the name.
- Schema: `rmcp` uses schemars 1.x (`with_input_schema::<T>()` or a raw JSON object). `rust-mcp-sdk` uses its own `JsonSchema` derive, not schemars.
- Resources: `rmcp` has tool and prompt macros but no resource macro, so resources are hand-written `list_resources`/`read_resource`/`list_resource_templates`. The spec calls resources application-driven (host/user picks them) and tools model-controlled, and resources are read-only. Exposing Pages as resources too would be an extra read-only projection of existing read Operations, and how clients handle it varies.

[findings](../research/02-rust-mcp-sdk.md)
