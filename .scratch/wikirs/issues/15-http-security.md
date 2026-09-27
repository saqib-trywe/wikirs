# HTTP and MCP-over-HTTP security

Map: [wikirs map](../map.md)
Type: grilling
Status: resolved
Assignee: Saqib
Blocked by: 14

## Question

`wikirs serve` exposes every Operation over `POST /ops/{name}` and MCP at `/mcp`, including destructive ones and `add_attachment` (base64). What is the default bind address (localhost only?), and how is binding to other hosts opted into? Is there authentication (a bearer token generated per Wiki or per run, and where it's stored)? What about CORS and Origin/Host checks against DNS rebinding (the MCP spec requires Origin validation for Streamable HTTP)? Is there a read-only mode? Are there request size limits? Context: [interfaces.md](../../../docs/spec/interfaces.md), and the `local_fs: false` capability for HTTP.

Input from [Selecting a Wiki](14-selecting-a-wiki.md): `serve` serves exactly one Wiki per process, so auth and binding are per-Wiki per process. Serving several Wikis means several `serve`s on different ports. There's a user-level config at `<config dir>/wikirs/config.toml` and a per-Wiki `.wikirs/local.toml` (per machine, not synced), so either could hold serve settings or a token. See [wiki-selection.md](../../../docs/spec/wiki-selection.md).

## Answer

Resolved 2026-09-26 by grilling. The result is in [docs/spec/http-security.md](../../../docs/spec/http-security.md), with `interfaces.md` and `errors.md` amended to match. No ADR: the defaults are cheap to change, and the one risky part (where the token lives) is written down in the spec.

1. **Binding**: loopback on port 4747 by default. A non-loopback bind needs `--allow-remote` plus a token. No built-in TLS (use a tunnel, Tailscale or a proxy).
2. **Auth**: a bearer token for `/ops` and `/mcp`. Optional on loopback (`require_token` turns it on), mandatory otherwise. No auth for MCP over stdio.
3. **Token storage**: outside the Wiki, in `<state dir>/wikirs/<wiki key>/serve-token` (0600). `--print-token` / `--rotate-token` are adapter flags.
4. **Host / Origin / CORS**: Host allowlist (against DNS rebinding); Origin must be in `cors_origins` (empty by default); POST needs JSON. All in one middleware in front of both routes.
5. **Transport errors**: `unauthorized` / `forbidden` / `payload_too_large` (401 / 403 / 413), kept separate from the closed set of Operation error kinds.
6. **Read-only**: `serve --read-only` and `mcp --read-only` set the capability `mutations: false` (only queries are listed, and mutation calls are refused). The parity test takes it into account.
7. **Limits**: a 32 MiB body limit by default. No rate limits.
8. **Settings**: `[serve]` in `.wikirs/local.toml`, with flags overriding it.
