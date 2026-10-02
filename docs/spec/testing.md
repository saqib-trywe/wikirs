# Testing strategy

How wikirs is tested, beyond the three-layer parity test in [interfaces.md](interfaces.md#parity-test). Decided in [Testing strategy](../../.scratch/wikirs/issues/19-testing-strategy.md). Where the tests live is in [workspace.md](workspace.md#tests-and-ci).

Tools: `insta` (snapshots), `proptest` (properties), `assert_cmd` (CLI), `tempfile` (scratch Wikis).

## Fixtures

- **Checked-in fixture Wikis** in `tests/fixtures/<name>/`: `basic`, `links-edge-cases`, `tags`, `placeholders`, `case-and-unicode`, `skipped-files` (symlinks, non-UTF-8).
- **Fixtures built inside a test**: `WikiBuilder::new().page("eng/rust", "…")…` for focused unit tests.
- **Tests never modify a checked-in fixture**: they copy it into a `tempfile` dir first.

## Splicing (golden files)

- `insta` snapshots of *(input file, Plan, output file)* for each case:
  - wikilinks with alias or `#heading`
  - relative links from different folders, `%20`
  - frontmatter `tags` in flow and block style
  - inline Tags next to code spans and URLs
  - CRLF files, and files with a BOM
- A byte-level assertion on every splice test: the output equals the input with only the planned ranges replaced.
- Snapshot changes show up as review diffs (`cargo insta review`).

## Plan properties (`proptest` over generated Wikis)

| Property | Statement |
|---|---|
| Idempotence | apply a Plan, then plan the same Operation again → an empty Plan |
| Round trip | `move_page a→b` then `b→a` restores every file byte for byte (same for `rename_tag`) |
| No lost Links | after any move, `check` shows no new Broken Links that weren't in the Plan's warnings |
| Dry run is pure | a dry run writes nothing, and its Plan equals the one that gets applied |

## Index consistency

**For any sequence of mutations and external edits, the Index after incremental updates plus a reconcile scan equals a fresh rebuild**, compared as a canonical dump of the Links, Tags and FTS rows. It runs as a proptest and on every fixture, and covers the watcher, the reconcile scan and applying mutations together.

## Concurrency and crash recovery (real processes)

The test binary launches itself as child processes.

- **Two writers**: two processes each apply 50 conflicting mutations. Every result is either applied or `conflict`, and the final state equals some serial order.
- **Crash in the middle of a Plan**:
  - `WIKIRS_TEST_CRASH_AFTER_EDITS=n` aborts after n edits. It's compiled only with a `test-hooks` feature.
  - The next `open` recovers the journal, and each file ends up as the roll-forward rules say ([process-model.md](process-model.md#mutations)), including a file edited externally in between.
- **Watcher**: an external write reaches a long-lived handle within 1 s, and the process's own writes aren't echoed as extra events.

## CLI

`assert_cmd` runs the built binary against fixtures, checking:
- exit codes 0–7 ([errors.md](errors.md#cli))
- that stdout and stderr stay separate
- the `--json` output shape
- `insta` snapshots of the human output for the main commands

Behaviour itself is covered by the parity suite.

## TUI and GUI

- **TUI**: `insta` snapshots of ratatui `TestBackend` frames for each view and popup, plus key-sequence tests (keys in, frame out).
- **GUI**:
  - The logic lives outside rendering and is tested directly: `wikirs-ui` (forms and the open-Page session) and the core's document model.
  - The gpui layer gets a few **headless smoke tests** through gpui-kit's `test-support` feature: open a Wiki, open a Page, run a palette Operation, trigger the conflict banner.
  - No pixel tests.
- **Palette coverage**: the parity test checks that every Operation appears in both palettes.

## CI

| When | Where | What |
|---|---|---|
| every push | Linux | `cargo test --all-features` (with gpui's Linux deps), `clippy`, `fmt`, `cargo check --no-default-features` |
| every push | macOS, Windows | `cargo test --no-default-features --features tui,serve,mcp` (paths, case, rename and file-lock behaviour differ there), plus `cargo build --all-features` so the gpui build can't break unnoticed |
| nightly | Linux | long proptest runs (more cases) |
