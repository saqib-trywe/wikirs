# Error mapping across Interfaces

Map: [wikirs map](../map.md)
Type: grilling
Status: resolved
Assignee: Saqib
Blocked by: —

## Question

How does each error kind in the closed set (NotFound, AlreadyExists, CaseConflict, InvalidPath, InvalidInput, Conflict, NoMatch/AmbiguousMatch, Io) and each `Error { kind, message, details? }` map onto each Interface?
- **CLI**: exit codes, and stderr format with and without `--json`.
- **HTTP**: status codes and the error body.
- **MCP**: tool result `isError` vs a JSON-RPC protocol error, and resource read errors.
- **GUI and TUI**: how errors are presented.

What goes in `details` for each kind (for example, the conflicting paths for Conflict)? How are warnings shown next to a successful result? See [operations.md](../../../docs/spec/operations.md#errors) and [interfaces.md](../../../docs/spec/interfaces.md).

## Answer

Resolved 2026-09-26 by grilling. The result is in [docs/spec/errors.md](../../../docs/spec/errors.md). The catalogue and `interfaces.md` were amended to match. No ADR: this is presentation and cheap to change, except the wire contract, which the golden snapshot of the catalogue covers.

1. **Wire shape**: `{error: {kind, message, details}}` with snake_case kinds. `kind` and `details` are the contract; `message` isn't.
2. **`details` per kind** is defined in the spec (for example `conflict` → `{reason: changed|lock_timeout, files}`, `invalid_input` → `{field?, reason}`).
3. **New kind `internal`**: the registry catches panics as `internal`, which takes the catalogue to 10 kinds.
4. **CLI exit codes**: 0 ok, 1 io/internal, 2 usage/invalid, 3 not_found, 4 exists/case, 5 conflict, 6 no/ambiguous match, 7 `check --fail-on-diagnostics`.
5. **CLI streams**: human errors and warnings on stderr. `--json` puts everything on stdout as one document.
6. **Warnings**: every Output on JSON Interfaces is wrapped as `{result, warnings}`.
7. **HTTP statuses**: 400 / 404 / 409 / 422 / 500, and 503 + Retry-After for a lock timeout.
8. **MCP**: Operation errors (including bad arguments) are `isError` tool results carrying `structuredContent`. An unknown tool is `-32602`. A missing resource is `-32002`.
9. **GUI and TUI**: a save conflict goes to the changed-on-disk flow, form errors appear inline by `details.field`, anything else is a notification or the banner row, and `internal` offers "copy details".
