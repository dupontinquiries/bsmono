# Documentation index

This directory contains two kinds of files, mixed together for
historical reasons. Read this index first to know which is which.

## Curated documentation (start here)

These are the **current, maintained** docs. Prefixed with `00_`, `01_`,
etc. so they sort to the top of `ls`. Read them in order on first
contact with the codebase, or jump to whichever applies.

| File | Purpose |
|---|---|
| [00_CLAUDE_CONTEXT.md](00_CLAUDE_CONTEXT.md) | **Start here.** Curated orientation for a fresh session (Claude or human). |
| [01_OVERVIEW.md](01_OVERVIEW.md) | What this app is and isn't. 60-second pitch. |
| [02_ARCHITECTURE.md](02_ARCHITECTURE.md) | Module map, data flow, key invariants. |
| [03_DESIGN_DECISIONS.md](03_DESIGN_DECISIONS.md) | Decisions we've made and why. Prevents re-litigating. |
| [04_TODO.md](04_TODO.md) | Open work, ranked. |
| [05_VISION.md](05_VISION.md) | Long-term ideas worth keeping in mind. |
| [06_TESTING.md](06_TESTING.md) | What's tested, what isn't, how to run them. |
| `features/` | Per-feature deep dives. |

## Public API docs (project root, not here)

These live at the project root, not in `docs/`, because they're the
**external contract** with consumers — not internal documentation:

- `../SERVICE_API.md` — D-Bus, XDG inbox, Unix socket
- `../SCREEN_TEMPS.md` — `/run` bitfield format and hint protocol

If you change the contract, those files must be updated in lockstep.

## Historical pile (everything else in this directory)

The rest of `docs/` — ~140 files like `EMERGENCY_FIX.md`,
`CRITICAL_FIXES_APPLIED.md`, `CHATGPT_IMPROVEMENTS_PLAN.md`, etc. —
is **historical**. These accumulated across many development
sessions and represent decisions and fixes that were correct at the
time but haven't been rolled up into the curated set. They are:

- **Probably partially stale.** Don't trust them blindly. Check live
  code if in doubt.
- **Sometimes useful for archaeology** ("why was this done this way?").
  Search them for a specific term if a piece of the current code
  doesn't make sense.
- **Not in scope for maintenance.** Adding more like them is
  discouraged. Update the curated set instead.

If you find yourself wanting to add another `FOO_FIXES.md` from a
chat session, please instead:
1. Update the relevant curated doc(s).
2. Add an entry to `04_TODO.md` if something is left undone.
3. Note major decisions in `03_DESIGN_DECISIONS.md`.

That keeps the signal-to-noise ratio sustainable.

## On the older `README.md`

The pre-existing `README.md` in this directory is itself part of the
historical pile — it describes a "barebones scaffold" version that's
long obsolete. The current entry point is this file (`_INDEX.md`).
