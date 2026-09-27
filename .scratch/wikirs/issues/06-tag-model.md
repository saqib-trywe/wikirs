# Tag model: syntax, canonical source, hierarchy semantics

Map: [wikirs map](../map.md)
Type: grilling
Status: resolved
Assignee: Saqib
Blocked by: —

## Question

How do Tags work? Where are they written: frontmatter `tags: [a/b]`, inline `#a/b/c`, or both, and if both, which one is canonical? What is the hierarchy separator, and are there case and character rules? What do the hierarchy semantics mean: does querying `lang` include `lang/rust/async`? Do Tags exist independently of Pages, for example with descriptions or as empty Tags? What do renaming, merging and moving a Tag subtree do to the files?

Input from [markdown parser research](../research/04-markdown-parser.md): no candidate parser recognises inline `#tag`, so supporting it means a custom scanner over text events (skipping code, links and headings).

## Answer

Resolved 2026-09-26 by grilling. The terms (Tag, Inline Tag) are updated in `CONTEXT.md`. There's no ADR, because all of this is plain text and cheap to reverse.

1. **Where Tags live**: in frontmatter `tags:` and inline as `#tag` (an **Inline Tag**). A Page's Tags are the union of both, and neither place is canonical. The app writes new Tags to frontmatter.
2. **Syntax**:
   - The separator is `/`.
   - Allowed characters are Unicode letters and digits, `-`, `_` and `/`. There's no leading, trailing or doubled `/`.
   - Every segment needs at least one non-digit, so `#123` and `#2026` aren't Tags.
   - Inline, a Tag is `#` at the start of a line or after whitespace or punctuation, immediately followed by a Tag character. Frontmatter values are written without `#`.
3. **Case**: matching is case-insensitive. The app writes lowercase, and a mixed-case Tag in a file is accepted with a warning.
4. **No Tag registry**: a Tag exists only while some Page carries it or one of its descendants. There are no descriptions, colours or aliases (out of scope).
5. **Hierarchy queries**:
   - By default, a query for `lang` includes descendant Tags. An `exact` flag limits it to Pages that carry the Tag itself.
   - The Tag tree returns both a direct and an inclusive count per node.
6. **Frontmatter**:
   - Only the `tags:` key is read, as a YAML list (flow or block) or a single string. There's no `tag:` alias and no comma-splitting.
   - Writes splice in just the item, matching the existing style, and leave every other byte unchanged.
   - A newly created `tags:` key uses block form (one item per line), because it merges cleanly.
7. **Rename Tag**:
   - One Operation, run as plan then apply with a dry run, like Page rename.
   - It rewrites frontmatter entries and Inline Tags in place and always moves the subtree (`lang/rust/async` → `rust/async`).
   - Renaming onto an existing Tag *is* the merge, so there's no separate merge Operation.
   - Duplicate frontmatter entries within a Page are collapsed. Inline occurrences are rewritten, never deleted, and mixed case comes out lowercase.
8. **Untag**: removes the frontmatter entry and strips the `#` from each Inline Tag, leaving the word in the prose. This runs as a planned edit.
9. **What gets tagged**: Pages only, not Attachments. Redundant ancestor Tags (`lang` + `lang/rust`) are allowed, with no warning.
10. **Inline scanner**:
    - Skips code spans and blocks, link destinations and URLs, and frontmatter.
    - Counts Tags in headings and in link text.
