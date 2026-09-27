# 2. Pages are identified by path; renames rewrite Links

Status: accepted (2026-09-26)

## Context

Every Link needs a stable way to name its target. The Wiki's `.md` files are the only source of truth. They must stay readable in plain editors and on GitHub, and they're synced between one person's machines by external tools (git, Dropbox).

Alternatives considered:

- **A stable id in frontmatter** (e.g. a ULID), with Links targeting the id. Links would survive renames with no rewriting. But ids in Link text are unreadable outside the app, every Page needs one (Pages created in vim have none), and plain-markdown tools can't resolve them.
- **Path identity plus an optional id** kept only to recover broken Links. This keeps most of the cost of ids without their main benefit.

## Decision

A Page is identified by its **Page Path**: its path relative to the Wiki root, without `.md`. Links refer to that path: wikilinks use the full path from the Wiki root, and standard links use a path relative to the file they're in. Renaming or moving a Page is a plan-then-apply Operation that rewrites every Link pointing at it, plus the moved Page's own relative Links. Each Link is spliced at its byte offsets, so nothing else in the file changes.

## Consequences

- The files stay plain markdown, and every Link can be resolved by humans and other tools.
- Rename-driven rewriting has to be reliable, which depends on source-preserving offset splicing (see the markdown parser research).
- A single rename can touch many files, which raises the chance of merge conflicts with edits made on another machine. That's acceptable for a single-user Wiki.
- A Link written or edited outside the app while a rename is underway can end up as a Broken Link. Broken Links are therefore a normal, reported state.
