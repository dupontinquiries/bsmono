# Drift

A scope-creep / boundary-drift reconciliation layer for Rust. **Not a new
compiler and not a new language** — a `cargo` plugin + macro layer that makes
cross-boundary assumptions explicit and continuously checks them against what
actually happens.

The one-line thesis: *most failures are not wrong code, they are broken
assumptions between distant systems.* Drift makes those assumptions
first-class so the break shows itself instead of costing a debugging session.

---

## ⚠️ Read this first: what runs vs. what is spec

This matters — do not assume anything here is built beyond the playground.

- **`playground/` — REAL, COMPILES, RUNS.** A ~60-line working demo of the
  MVP feature (the two-sided boundary journal). Verified with `cargo 1.75`,
  builds and runs both with and without the feature flag.
- **`docs/` — DESIGN AND SPEC, NOT IMPLEMENTED.** The concept, the language
  surface, the roadmap, and the exploratory ideas. The `cargo drift ...`
  subcommands described in the docs **do not exist yet.** Treat them as a
  target, not an API.

If you (an AI) are about to use a `cargo drift` command — stop, it isn't real.
Build it or simulate it; don't hallucinate its output.

---

## Run the playground

```sh
cd playground
cargo run                          # drift OFF — the "pure" build; the bug is invisible
cargo run --features drift-trace   # drift ON  — the journal localizes the bug
```

The off-build silently does its work and tells you nothing — the same "no logs
at all" symptom that makes wiring bugs expensive. The on-build prints which
boundaries are healthy, which are broken wires, and which are dead surfaces.

## What's in the box

```
drift/
├── README.md                       you are here
├── CLAUDE.md                       brief for the testing agent — read before doing anything
├── playground/                     WORKING code (the MVP)
│   ├── Cargo.toml
│   ├── README.md
│   └── src/{main.rs, drift.rs}
└── docs/
    ├── 01-concept.md               the refined idea + the test for what's real
    ├── 02-language-spec.md         macros, annotations, cargo subcommands, semantics
    ├── 03-roadmap.md               what's done, what's next, the open risk
    └── 04-design-explorations.md   speculative: shell/python borrowing, interpreter, AI-readability
```

## The shortest possible summary

Annotate a **boundary** (a seam where two parts change independently). One side
claims it serviced an intent; the other claims it attempted one. Drift diffs
the two and tells you exactly where the wire broke. Everything else in the
design is a variation on that single primitive: *a human claim + a mechanical
verifier + reconciliation over time.*
