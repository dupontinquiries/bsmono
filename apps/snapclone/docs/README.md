# SnapClone documentation

This directory holds the project's long-form documentation. For a
high-level overview of the app see the [top-level README](../README.md).

## Start here

| Document | What it covers |
|-|-|
| [VISION.md](VISION.md) | What we're building and why. Read this first. |
| [ROADMAP.md](ROADMAP.md) | Strategic priorities organised by stage. |
| [TODO.md](TODO.md) | Tactical work items with priority/status tags. |
| [DEVELOPING.md](DEVELOPING.md) | Practical guide to building, debugging, and architecture invariants. |
| [WORK_LOG.md](WORK_LOG.md) | Session-by-session summary of significant fixes and decisions. |

## Subsystem deep-dives

| Document | What it covers |
|-|-|
| [../ARCHITECTURE.md](../ARCHITECTURE.md) | Architecture Decision Records (ADRs). |
| [DATABUS.md](DATABUS.md) | Internal event bus design. |
| [SignalRegistry.md](SignalRegistry.md) | Catalog of Qt signals and their listeners. |
| [LOCATION_PRIVACY.md](LOCATION_PRIVACY.md) | Location data handling. |

## When to update each file

- **VISION.md** — almost never. The vision changes maybe once a year.
- **ROADMAP.md** — when stage emphasis shifts; when a stage completes.
- **TODO.md** — every session. This is the working list.
- **DEVELOPING.md** — when an architecture invariant changes, or when
  a new contributor hits an undocumented gotcha.
- **WORK_LOG.md** — at the end of any session that made non-trivial
  changes. Be candid about wrong turns.
- **ARCHITECTURE.md** — when a major design decision is made or revised.

If you find yourself wondering "where should this go," the answer is
usually:
- Long-lived intent → VISION
- Multi-session plan → ROADMAP
- Single-session work → TODO
- "What is this code doing and why" → DEVELOPING
- "What just happened" → WORK_LOG
- "Why did we choose X over Y" → ARCHITECTURE
