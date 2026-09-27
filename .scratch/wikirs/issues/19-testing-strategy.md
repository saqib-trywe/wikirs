# Testing strategy

Map: [wikirs map](../map.md)
Type: grilling
Status: resolved
Assignee: Saqib
Blocked by: —

## Question

Beyond the three-layer parity test ([interfaces.md](../../../docs/spec/interfaces.md#parity-test)), how is wikirs tested?

- **Fixtures**: fixture Wikis (a directory tree checked into `tests/fixtures`, or generated per test in a tempdir?).
- **Golden files**: for Link and Tag splicing, where every other byte must be unchanged. How are they reviewed (`insta`?)
- **Plans and diffs**: property tests for Plans (for example, "apply then re-plan is a no-op", "move then move back restores the bytes").
- **Concurrency**: the write lock across two real processes, journal recovery after a simulated crash between edits, and watcher events from external edits.
- **Index**: consistency of the reconcile scan against a full rebuild.
- **GUI**: how far it can be tested beyond its palette (gpui-kit claims headless UI tests; the TUI can use ratatui's `TestBackend` snapshots, as the prototype did).
- **CI**: what runs where (macOS/Linux/Windows, and whether the gpui build runs in CI or only `--no-default-features`).

Context: [workspace.md](../../../docs/spec/workspace.md) (crates, where tests live), [process-model.md](../../../docs/spec/process-model.md), and the [TUI prototype](../prototypes/12-tui-layout/README.md) (`--snapshot`).

## Answer

Resolved 2026-09-27 by grilling. The result is in [docs/spec/testing.md](../../../docs/spec/testing.md).

1. **Fixtures**: checked-in fixture Wikis (always copied into a tempdir first) plus a `WikiBuilder` for fixtures built inside tests.
2. **Splicing**: `insta` snapshots of (input, Plan, output), plus a byte-level check on every splice test.
3. **Plan properties**: proptest for idempotence, round trip, no lost Links, and pure dry runs.
4. **Concurrency**: real child processes: two writers, a crash in the middle of a Plan through a `test-hooks` env var, and watcher latency and echoes.
5. **Index**: incremental updates plus a reconcile scan must equal a fresh rebuild, as a proptest and on every fixture.
6. **UIs**: TUI `TestBackend` snapshots. For the GUI, the logic is tested outside gpui, with a few headless smoke tests. No pixel tests.
7. **CLI**: `assert_cmd` checks exit codes, streams, `--json` shape and human-output snapshots.
8. **CI**: full tests on Linux; macOS and Windows test without the GUI and build with it; long proptests run nightly.
