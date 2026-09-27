# 1. Single binary, no daemon

Status: accepted (2026-09-26)

## Context

wikirs exposes the same Operations through five Interfaces: GUI (gpui), CLI, TUI (ratatui), HTTP and MCP. The Wiki's `.md` files are the only source of truth and may also be edited by external tools (vim, git pulls, sync clients). We needed a process model for how the Interfaces reach the Wiki.

Alternatives considered:

- **Daemon owns the Wiki**, with every Interface acting as an HTTP client. This gives a single writer and an easy shared Index, but adds a daemon lifecycle (start, stop, crash recovery) and still has to handle external edits to the files.
- **Separate binaries** sharing a core crate. Same runtime model as the chosen option, with more packaging for no benefit.

## Decision

Ship one `wikirs` binary. Each Interface is a subcommand (`gui`, `tui`, `serve`, `mcp`, plus plain CLI Operations), and every one of them links the core library and operates on the files in-process. Concurrent processes (for example, the GUI open while an agent edits via MCP) are reconciled through filesystem watching and the rebuildable Index. That is the same mechanism that already has to handle external edits.

## Consequences

- There is no single writer. Handling write conflicts and sharing or locking the Index between processes must be designed explicitly (see the "Process model & concurrency" ticket).
- `wikirs serve` and `wikirs mcp` are just more Interfaces, not the core of the system.
- A daemon could be added later as an optimisation without changing the Operation contract.
