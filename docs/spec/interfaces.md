# Interfaces: how each one exposes the Operation registry

How every Interface maps onto the [Operation catalogue](operations.md). The mechanism and its rationale are in [ADR 0004](../adr/0004-operation-registry-generated-adapters.md). Decided in [Parity architecture](../../.scratch/wikirs/issues/09-parity-architecture.md).

## Core

- `Wiki::open(root) -> Wiki` holds the root, the merged config and the Index connection. How each Interface finds `root` is in [wiki-selection.md](wiki-selection.md).
- Every Operation is `run(&Wiki, Input) -> Result<Output, Error>`. The core is synchronous.
- The erased registry returns errors as `Error { kind, message, details }`, with `kind` from the closed set in the catalogue, and catches panics as `internal`. Outputs are wrapped as `{ result, warnings }`. The mapping onto each Interface is in [errors.md](errors.md).
- The catalogue (names, kinds, descriptions, input and output schemas, binary version) can be read as JSON.
- There's no per-Operation versioning. A golden snapshot shows when a change breaks the contract.

## Naming

Each Operation keeps its catalogue name on every Interface. The CLI only changes the case to kebab:

| Interface | `move_page` appears as |
|---|---|
| CLI | `wikirs move-page` |
| HTTP | `POST /ops/move_page` |
| MCP | tool `move_page` |
| GUI / TUI palette | "move_page" entry (display label may be humanised) |

## Capabilities

Every adapter declares `local_fs: bool`:
- true for CLI, GUI, TUI and MCP over stdio
- false for HTTP and MCP over Streamable HTTP

A `local_path` input (`add_attachment.source`, `read_attachment.as`) to an adapter without `local_fs` fails with `InvalidInput`.

`serve` and `mcp` also take `--read-only`, which sets `mutations: false`. Their catalogue then lists only queries, maintenance (`rebuild_index`, which never touches a Wiki file) and `watch`, and HTTP mutation calls get 403 `forbidden`. See [http-security.md](http-security.md#read-only-mode).

## CLI

- Input structs derive `clap::Args`, and the subcommand enum is generated.
- Every subcommand also accepts `--input <json | ->`: the whole Input as JSON (`-` reads stdin), in place of the arguments it conflicts with. Its errors are `invalid_input` with `field: "input"`.
- Output: `--json` prints the `{ result, warnings }` envelope (or the error object) on stdout. Otherwise the CLI renders the result for humans, with one renderer per Output type (picked by the type's schema title) and pretty JSON for the rest. Any result with a Plan renders it as a unified diff on a dry run, and as one line per edit once applied.
- `check --fail-on-diagnostics` exits 7 if there are any ([errors.md](errors.md#cli)).
- `watch` prints JSON Lines.
- `wikirs config adopt [--from <old root>] [--dry-run]` takes over a moved Wiki's machine settings ([wiki-selection.md](wiki-selection.md#machine-settings)). It's CLI-only, not an Operation. In human mode, every Operation subcommand prints a `hint:` pointing at it while there are settings to adopt.

## HTTP (`wikirs serve`, axum)

| Route | Meaning |
|---|---|
| `POST /ops/{name}` | JSON Input → JSON Output (queries, mutations and maintenance) |
| `GET /ops` | the catalogue |
| `GET /ops/watch?space=…&path_prefix=…` | `watch` as Server-Sent Events: one `data:` line per event, the event as JSON (the Scope fields as query parameters) |
| `/mcp` | MCP Streamable HTTP, mounted on the same router |

- An empty body is the input `{}`. `POST /ops/watch` is an Operation call like any other, and fails with `invalid_input` as `watch` does on every Interface.
- HTTP calls run without `local_fs`, so `local_path` inputs are `invalid_input` (the parity test expects that).
- `/mcp` gives each session its own server: its own legacy `resources/subscribe` set.

Binding, auth, Origin/Host checks and read-only mode are in [http-security.md](http-security.md).

## MCP (`wikirs mcp` over stdio; `/mcp` under `serve`), rmcp

- **Tools**: every query, mutation and maintenance Operation, registered at runtime from the registry (`ToolRoute::new_dyn`, `Tool::new` with the registry schema).
  - Queries carry `readOnlyHint`.
  - `delete_page`, `delete_attachment`, `move_page`, `move_attachment` and `rename_tag` carry `destructiveHint`.
  - `rebuild_index` carries neither hint: it rewrites only the disposable Index.
- **Resources**: a hand-written, logic-free projection of the read Operations.

  | Resource | URI template | Backed by |
  |---|---|---|
  | Page | `wiki://page/{+path}` (`text/markdown`) | `get_page` |
  | Attachment | `wiki://attachment/{+path}` (blob) | `read_attachment` |
  | listing | `list_resources`, paginated | `list_pages` + `list_attachments` |

  Tag listings are not resources; `tag_tree` covers them.
- **`watch`** isn't a tool. Its MCP form is resource notifications driven by `watch`:
  - 2026-07-28 clients open `subscriptions/listen`. The server accepts `resourcesListChanged` and any `wiki://` Page or Attachment URIs among the requested `resourceSubscriptions`.
  - Legacy clients (the `initialize` handshake) use `resources/subscribe` / `unsubscribe`, and get `list_changed` from the start.
  - A resource gets `resources/updated` when its file changes, including both paths of a move. `resources/list_changed` is sent once per batch that created, deleted or moved something, and never for edits alone.

## GUI and TUI

- They call typed Operations in-process: the GUI on gpui's background executor, the TUI directly or on a worker thread.
- `watch` is a channel receiver, bridged into each UI's event loop.
- **Command palette**, generated from the registry:
  - It lists every Operation and renders a generic form from the Input schema (strings, bools, enums, arrays, optional fields).
  - Mutations get a dry-run toggle that shows the Plan.
  - This is the parity floor. Bespoke UI (tree, editor, backlinks) calls the same Operations on top of it.

## Parity test

1. **Coverage**: registry names vs each adapter's exposed names:
   - CLI subcommands
   - HTTP `/ops`
   - MCP tools, plus resources/notifications for `watch`
   - GUI and TUI palette entries

   Adapters with `mutations: false` are checked against the subset of queries, maintenance and `watch`. The form renderer must support every schema shape used by any Input. A failure names the Interface and the Operation.
2. **Behaviour**: one scenario suite (create → link → move → backlinks → tag → rename_tag → delete, with dry runs) runs against a fixture Wiki through the core, CLI (in-process, `--json`), HTTP (tower `oneshot`) and MCP (in-process transport). It asserts identical JSON Outputs and error kinds.
3. **Contract snapshot**: a golden file of the catalogue JSON.

The Reference section of `operations.md` is generated from the registry by `tests/docs.rs`, which fails if the committed copy is stale (`UPDATE_DOCS=1` rewrites it).
