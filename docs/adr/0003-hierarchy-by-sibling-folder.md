# 3. Page hierarchy is the folder tree, with the parent Page beside its folder

Status: accepted (2026-09-26)

## Context

Pages form a hierarchy (Confluence-style parent and Child Pages). The hierarchy must be visible and meaningful outside the app (plain editors, GitHub) and merge-friendly, and it has to fit path identity ([ADR 0002](0002-path-identity-with-rewrite.md)).

Alternatives considered:

- **An index file** (`parent/index.md` + `parent/child.md`). Everything sits in one folder, but it muddies identity: is the parent's Page Path `parent` or `parent/index`? And many editor tabs end up titled `index.md`.
- **A frontmatter `parent:` field** with flat folders. The hierarchy is invisible outside the app, and moving a parent means rewriting its children's frontmatter.

## Decision

A Page's Child Pages live in a folder next to it with the same name: `parent.md` + `parent/child.md`. The Page Path is the page's position in the tree, and there are no exceptions: a Space's home Page is `<space>.md` at the Wiki root. A folder with no matching `.md` is a **Placeholder**. The Page's own Attachments go in the same folder.

## Consequences

- Moving a Page moves its folder, and every Child Page and Attachment with it, so a single move can rewrite Links to a whole subtree.
- A parent and its children are two entries on disk (`parent.md` and `parent/`), which tools that aren't hierarchy-aware show separately.
- Deleting a parent without `recursive` leaves a Placeholder rather than orphaning or deleting its children.
