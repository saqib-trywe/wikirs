# 4. Operation registry with generated adapters

Status: accepted (2026-09-26)

## Context

Parity is a hard requirement: all 32 Operations in the [catalogue](../spec/operations.md) must be available, and must behave the same, on the GUI, CLI, TUI, HTTP and MCP. With five hand-written adapters, parity would erode Operation by Operation. We needed a mechanism that makes a missing or divergent Operation a failing build or test, not a code-review catch.

Alternatives considered for the representation:

- **One `Request`/`Response` enum**: serde dispatch comes free, and an exhaustive `match` checks coverage at compile time. But every adapter matches 32 arms, and typed Outputs collapse into one union, which hurts the in-process GUI and TUI.
- **A proc-macro on plain fns** (`#[operation]`): the most pleasant to write, but it hides the registry inside a proc-macro crate that is hard to debug.
- **REST routes for HTTP**: they would be a second, hand-kept naming of the contract.

## Decision

- **Registry.** Each Operation is a struct implementing `trait Operation { NAME, KIND, Input, Output, run(&Wiki, Input) }`. One `operations![...]` declarative macro lists them all and generates the erased registry: name → kind, description, input and output schemas, and a JSON-in/JSON-out dispatch. Registering an Operation happens in exactly one place.
- **One schema.** Input and Output types derive `serde` and `schemars` (plus `clap::Args` on Inputs). Doc comments are the descriptions. Nothing else describes the contract.
- **Generated adapters.** The CLI subcommands, the HTTP RPC routes (`POST /ops/{name}`), and the MCP tools (added at runtime) are all generated from the registry, and each Operation's name is used unchanged on every Interface. The GUI and TUI call typed Operations in-process, and each has a command palette generated from the registry, which gives them a floor of parity. Bespoke UI sits on top of it.
- **Thin adapters.** An adapter may only parse its transport into an Input, enforce capabilities (`local_fs`, and `mutations` for read-only mode), render an Output or Error, and bridge `watch`. Any behaviour a user could notice belongs in an Operation.
- **Parity test** in three layers:
  1. **Coverage**: every registry Operation is exposed by every adapter, and the schema-form renderer supports every schema shape in use.
  2. **Behaviour**: one scenario suite runs through the core, CLI, HTTP and MCP and asserts identical JSON and error kinds.
  3. **Contract snapshot**: a golden snapshot of the catalogue JSON.

## Consequences

- Adding an Operation means one struct plus one line in `operations![]`. The CLI, HTTP and MCP get it for free, and the GUI and TUI get it through the palette.
- The core is synchronous. HTTP and MCP call it through `spawn_blocking`, the GUI through gpui's background executor, and `watch` returns a plain channel that each adapter bridges. This keeps tokio out of the core and the GUI, and it's cheap to revisit.
- The generated palette forms constrain Input types: any schema shape the form renderer can't handle fails the coverage test, which pushes Inputs toward simple shapes.
- Once code exists, the registry is the source of truth, and the reference tables in `docs/spec/operations.md` are generated from it and checked for staleness.
- HTTP is RPC, not REST. Clients that expect resource-shaped URLs don't get them.
