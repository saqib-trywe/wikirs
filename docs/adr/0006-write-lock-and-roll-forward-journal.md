# 6. Wiki-wide write lock with a roll-forward journal

Status: accepted (2026-09-26)

## Context

A mutation computes a Plan (possibly dozens of file edits, e.g. a move that rewrites every Link) and applies it. With no daemon, two wikirs processes can plan and apply at the same time: an agent's `rename_tag` over MCP while the GUI saves a Page. A process can also die in the middle of a Plan and leave half the Links rewritten. External editors (vim, git, sync clients) change files too, but wikirs can't coordinate with them.

Alternatives considered:

- **Rely only on hash checks.** Each file is checked against its expected hash before it's written. That catches conflicts per file, but two wikirs Plans can interleave and each can pass its own checks, leaving a mix of both.
- **Undo journal / rollback.** This needs the original contents stored and risks overwriting edits made after the crash.
- **No recovery.** Leave partial Plans for `check` to report as Broken Links. That's simple, but a crash mid-move silently corrupts the Wiki's Links.

## Decision

- **Write lock.** Every mutation takes one advisory OS file lock per Wiki (`write.lock` in the Wiki's cache dir) from computing the Plan until the Index update finishes. Other wikirs processes wait, and time out with `Conflict`. Queries never take the lock.
- **Hash checks** under the lock catch external editors. Each touched file is re-read and compared against the caller's `base_version` and the hash the Plan was computed from, and any mismatch aborts the whole Plan before a single write.
- **Roll-forward journal.** Before applying, the Plan (each edit with the hash before and after it) is written to `journal.json` in the cache dir. Edits apply in the order creates/modifies → moves → deletes, so a crash never loses content. The journal is deleted once the Plan completes. If a journal is found at open or when the lock is taken, each edit is finished if its file still has the "before" hash, skipped if it has the "after" hash, and otherwise left alone and reported.

## Consequences

- Plans are atomic with respect to other wikirs processes, and a crashed Plan is completed without prompting, never overwriting an external edit.
- All mutations across all wikirs processes on one Wiki are serialised. They take milliseconds, so waits are short, but one slow Plan (a huge `rename_tag`) blocks every other writer for its duration.
- Against external editors there is still a small window between the hash check and the rename. There's no portable way to close it.
- The lock and journal live in the local cache dir, so they coordinate processes on one machine only. Coordination across machines is left to sync, as the rest of the design already does.
