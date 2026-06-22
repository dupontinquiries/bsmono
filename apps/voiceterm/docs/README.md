# web2qml docs

Documentation for the web2qml project. This folder is the entry point.

## What's here

- **[vision.md](vision.md)** — long-term goals; what we're building toward
- **[architecture.md](architecture.md)** — system overview and module map
- **[pipeline.md](pipeline.md)** — voice → intent flow end-to-end
- **[multi-user.md](multi-user.md)** — speakers, separation, handoff design
- **[voice-pipeline.md](voice-pipeline.md)** — phonetic DB, fingerprints, confusion table
- **[languages.md](languages.md)** — multilingual support and how to add a language
- **[terminal.md](terminal.md)** — PTY, blocks, summarization, smart features
- **[browser.md](browser.md)** — web rendering, alx.sh, link handling
- **[todo.md](todo.md)** — active TODO list (curated, prioritized)
- **[decisions.md](decisions.md)** — architectural decisions and trade-offs
- **[glossary.md](glossary.md)** — terms and acronyms

## What lives outside this folder

- **[../TODO_fingerprint_db.md](../TODO_fingerprint_db.md)** — historical
  archive of per-version development notes from v0.0.x through v0.1.117.
  Sprawling but searchable. New per-version notes go in
  [changelog/](changelog/) instead.
- **[../HELIX_integration_design.md](../HELIX_integration_design.md)** —
  standalone design doc for editor integration (not yet implemented).
- **[../TPM_keys_design.md](../TPM_keys_design.md)** — standalone design
  doc for TPM-bound encryption (not yet implemented).
- **[../architecture.md](../architecture.md)** — older flat architecture
  doc (predates this docs folder). To be merged into
  [architecture.md](architecture.md) here.

## How to navigate by question

- **"What does X do?"** → `architecture.md` (module-level) or the
  specific topic file (`voice-pipeline.md`, `terminal.md`, etc.)
- **"Why is X designed this way?"** → `decisions.md`
- **"What's the plan for X?"** → `vision.md` or `todo.md`
- **"What changed in v0.1.N?"** → `changelog/` (recent) or
  `TODO_fingerprint_db.md` (older, ungrouped)
- **"What does this term mean?"** → `glossary.md`

## How to update

When making changes:

1. **Vision-level shift?** — edit `vision.md` and link from the relevant
   topic doc.
2. **New module or significant refactor?** — update `architecture.md` and
   the topic doc for that module.
3. **Trade-off you made consciously?** — append to `decisions.md`. Keep
   each decision brief: title, context, decision, alternatives considered,
   trade-offs accepted. ADR-style.
4. **New feature shipped?** — add to `changelog/` for the current minor
   version. Cross-link from `todo.md` (move item from pending to "done
   in v0.1.N").
5. **New TODO item?** — `todo.md`. Group by area. Include rough size
   estimate (S/M/L) and what enables it.

Don't dump everything into one file again. The 6400-line monolith is
what we're moving away from.
