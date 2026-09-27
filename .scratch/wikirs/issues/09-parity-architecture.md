# Parity architecture: Operation registry, adapters, parity test

Map: [wikirs map](../map.md)
Type: grilling
Status: resolved
Assignee: Saqib
Blocked by: 02, 08

## Question

How is parity enforced mechanically? Should each Operation be a trait impl, an enum variant, or a macro-generated fn? Should serde and schemars types serve as the single schema? How does each Interface consume the registry: clap subcommands (generated or hand-written?), HTTP routes (REST vs RPC), MCP tools (registered at runtime from the registry), and GUI/TUI actions? What does the parity test look like, and what fails when an Operation is missing from an Interface? Record the result as an ADR.

Settled input (user, 2026-09-26): Pages **are** exposed as MCP *resources* as well as tools. Still to decide here: the resource URI scheme (for example `wiki://<space>/<path>`), whether they're backed by the existing read Operations (so no Operation exists only in MCP), whether Attachments and Tag listings are resources too, and whether change notifications are sent. See [MCP SDK findings](../research/02-rust-mcp-sdk.md).

Inputs from [Operation catalogue](08-operation-catalogue.md) ([spec](../../../docs/spec/operations.md)): 32 Operations of three kinds: query, mutation and subscription (`watch`). Every mutation takes `dry_run` and returns a Plan. The set of error kinds is closed. The registry has to model a streaming subscription (SSE over HTTP, notifications over MCP), and `add_attachment`/`read_attachment` accept and return either a local path or base64 bytes, depending on the Interface.

## Answer

Resolved 2026-09-26 by grilling. The mechanism is recorded in [ADR 0004](../../../docs/adr/0004-operation-registry-generated-adapters.md), and the mapping for each Interface is in [docs/spec/interfaces.md](../../../docs/spec/interfaces.md).

1. **Registry**: a trait per Operation (`NAME`, `KIND`, `Input`, `Output`, `run(&Wiki, Input)`), with one `operations![]` macro generating the erased registry (metadata + JSON dispatch). The core is sync, and `watch` returns a channel.
2. **Schema**: serde + schemars 1.x (+ `clap::Args` on Inputs) from one set of types. The catalogue JSON also carries the binary version.
3. **Naming**: an Operation's name is the same everywhere (CLI in kebab case).
4. **CLI**: derived clap commands, `--input <json>` as an escape hatch, human output by default or `--json`, dry-run Plans shown as a unified diff.
5. **HTTP**: RPC. `POST /ops/{name}`, `GET /ops` (the catalogue), `GET /ops/watch` (SSE), and MCP mounted at `/mcp`.
6. **MCP**: every query and mutation becomes a runtime tool, with read-only and destructive hints. Pages and Attachments are resources (`wiki://page/{+path}`, `wiki://attachment/{+path}`) projected from `get_page`/`read_attachment`/`list_*`. `watch` becomes resource notifications.
7. **GUI and TUI**: typed in-process calls, plus a command palette generated from the registry (schema forms, dry-run toggle), which sets the minimum parity. Bespoke UI sits on top.
8. **Capabilities**: each adapter declares `local_fs`, and a `local_path` input without it fails with `InvalidInput`.
9. **Thin-adapter rule**: an adapter only parses input, checks capabilities, renders output and bridges `watch`.
10. **Parity test**: coverage per Interface (including the form renderer's supported shapes), a behaviour suite checking identical JSON across core, CLI, HTTP and MCP, and a golden snapshot of the catalogue. Once code exists, the registry is the source of truth, and the reference tables in the spec are generated from it.
11. **Errors**: `Error { kind, message, details? }` crosses the registry. How each kind maps to each Interface stays Not yet specified.
