# Errors and warnings across Interfaces

How the closed set of error kinds in the [Operation catalogue](operations.md#errors) and warnings reach each Interface. Decided in [Error mapping across Interfaces](../../.scratch/wikirs/issues/13-error-mapping.md).

## The error value

Every Operation failure crosses the registry as one value:

```json
{ "error": { "kind": "conflict", "message": "eng/rust.md changed since you read it", "details": { … } } }
```

- `kind` is the snake_case form of the core enum. **`kind` and `details` are the stable contract.**
- `message` is English prose for humans and may change at any time.
- The registry catches panics inside an Operation and returns `internal`, so one bug doesn't take down `serve`, `mcp` or the GUI.

| kind | details |
|---|---|
| `not_found` | `{ what: page \| attachment \| tag \| config_key \| wiki, path }` |
| `already_exists` | `{ path }` |
| `case_conflict` | `{ path, existing }` |
| `invalid_path` | `{ path, reason }` (e.g. `malformed`, `into_own_subtree`) |
| `invalid_input` | `{ field?, reason }` |
| `conflict` | `{ reason: changed \| lock_timeout, files: [{ path, expected, actual }] }` |
| `no_match` | `{ edit_index, count: 0 }` |
| `ambiguous_match` | `{ edit_index, count }` |
| `io` | `{ path?, os_error }` |
| `internal` | `{ backtrace_id? }` |

## Transport errors

`serve` rejects some requests before any Operation runs ([http-security.md](http-security.md)). These use the same body shape, but their kinds are **not** part of the closed set of Operation error kinds:

| kind | Status | When |
|---|---|---|
| `unauthorized` | 401 | missing or wrong bearer token |
| `forbidden` | 403 | bad `Host` / `Origin` / `Content-Type`, or a mutation on a `--read-only` server |
| `payload_too_large` | 413 | body over `serve.max_body` |

## Success and warnings

On JSON Interfaces (HTTP, MCP, CLI `--json`) every Output is wrapped:

```json
{ "result": { … }, "warnings": [ … ] }
```

A Plan keeps its own `warnings`, and the envelope repeats them. The GUI and TUI receive the warnings in the typed result.

## CLI

| Exit code | Meaning |
|---|---|
| 0 | success (warnings don't change it) |
| 1 | `io`, `internal` |
| 2 | usage: clap errors, `invalid_input`, `invalid_path` |
| 3 | `not_found` |
| 4 | `already_exists`, `case_conflict` |
| 5 | `conflict` |
| 6 | `no_match`, `ambiguous_match` |
| 7 | `check --fail-on-diagnostics` found diagnostics |

- **Human mode**:
  - Errors print to stderr as `error[<kind>]: <message>`, with a `hint:` line where one helps.
  - Warnings print to stderr as `warning: …`.
  - The result goes to stdout.
- **`--json`**: the envelope or the error object goes to **stdout** as one document, and the exit code as above.
- `check` returns diagnostics as data and exits 0 unless `--fail-on-diagnostics` is given.

## HTTP

The body's `kind` is authoritative. The status is:

| Status | kinds |
|---|---|
| 400 | `invalid_input`, `invalid_path`, malformed JSON |
| 404 | `not_found`, and unknown Operation names |
| 409 | `already_exists`, `case_conflict`, `conflict` |
| 422 | `no_match`, `ambiguous_match` |
| 500 | `io`, `internal` |
| 503 + `Retry-After: 1` | `conflict` with `reason: lock_timeout` |

## MCP

- **Operation errors, including invalid arguments**: a tool result with `isError: true`. The content is a text message, and `structuredContent` is the error object, so the model can see what went wrong and correct itself.
- **Unknown tool**: JSON-RPC error `-32602`.
- **Resources**: `not_found` → JSON-RPC `-32002` (resource not found), which rmcp reports as `-32602` to 2026-07-28 peers (SEP-2164). Any other kind → `-32603`. Both carry the error object (`{ kind, message, details }`) in `data`. A URI that isn't a `wiki://` Page or Attachment is `not_found`.
- **Read-only** (`mcp --read-only`): a mutation is an unknown tool (`-32602`).

## GUI and TUI

| Situation | Presentation |
|---|---|
| `conflict` from saving a Page | the changed-on-disk flow (banner, Compare); never a generic error |
| `invalid_input` from a palette form | inline under the field named by `details.field` |
| any other error from a user action | GUI notification / TUI banner row: `message` plus the kind |
| `internal` | as above, plus a "copy details" action for bug reports |
| warnings | in the Plan preview (mutations), or as a muted line under the result |
