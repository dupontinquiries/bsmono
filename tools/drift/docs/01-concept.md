# 01 — Concept

## The reframe

Drift began as "a programming language for inevitable scope creep" with ~20
features. Most of those were comedy wearing a spec costume. What survives
scrutiny is narrower and more defensible:

> Drift is a layer that makes **cross-boundary assumptions explicit** and
> **continuously reconciles them against observed reality.**

Scope creep is the *force* that makes boundaries drift. Drift treats the drift,
not the creep. It is a `cargo` plugin + macro layer, not a compiler — because
new languages are essentially never adopted, but a crate you add to an existing
project can be.

## The one primitive

Every worthwhile Drift feature is the same shape:

> **a human-authored claim + a mechanical verifier + reconciliation over time.**

The product is the *delta* between what was claimed and what is observed.

| feature              | claim                              | verifier                          |
|----------------------|------------------------------------|-----------------------------------|
| expiring temp code   | "gone by X"                        | is it still here?                 |
| intent contract      | "returns terminal success, fast"   | what did it return, how slow?     |
| blast radius         | "these are my dependencies"        | the real graph                    |
| boundary journal     | "I serviced / attempted intent X"  | did both sides agree?             |

## The test for whether a feature is real

> **Is there an observable signal that confirms or refutes the claim?**

If not, it's decoration. This single test does all the sorting:

- `Temporary<T>` — real. Verifier: time + presence.
- `Deferred<T>` — real (if pointed at a concrete successor). Verifier: does the
  successor exist yet?
- `Political<T>`, `Human<T>`, `std::hope` — decoration. Nothing observable
  distinguishes them from a plain `T`. They were jokes; keep them as jokes.

## The scope discipline

A claim can only *drift* if the two things it relates change **independently.**
Internal code recompiles together — it cannot drift against itself, and the
compiler already catches that. So Drift earns its keep **only at boundaries
between independently-changing systems:**

- Rust ↔ QML (the canonical case — types match, the compiler is green, the
  button is dead because the *meaning* changed)
- service ↔ service
- team-A's module ↔ team-B's consumer
- schema ↔ persisted data

This discipline is what kills the original brainstorm's per-function
annotations. Annotate seams, never internals.

## The debt ledger maintains itself

Every hand-curated tech-debt list dies of staleness. Drift's doesn't, because
you never curate it. When a drift fires and you respond "yes, this can return
`Pending` now, the UI handles it," that acknowledgment is *data* — it lands in
the journal with who, when, and why. The debt ledger is just the accumulated
log of drifts the team chose to accept. Suppressing a warning updates the model
instead of hiding it.

## Why the two-sided journal is the MVP

The killer diagnostic falls straight out of instrumenting *both* sides of a
boundary. One side gives you "X never serviced" — ambiguous. Both sides give
you the set difference, which localizes the break with no guessing:

```
attempted ∩ serviced  → healthy boundary
attempted − serviced  → BROKEN WIRE  (consumer called, nobody answered)
serviced  − attempted → DEAD SURFACE (producer ready, nobody called it)
```

It's cheap (two macros + a sink), it produces an immediate aha on a real bug,
the debt ledger falls out of it for free, and it needs no hard static analysis.
That's why it's first. See `playground/` for the working version.

## The honest open risk

Feasibility is not the risk. **Annotation survival is.** Every part of this
needs a human to write an assumption down at a seam. If that costs more than
the bug it prevents, Drift dies the way every "just document your assumptions"
discipline dies. The current best hypothesis: infer the baseline, make the
human assert only the delta — and let the toolchain backfill everything it can
so the human confirms rather than authors. Whether that's enough is exactly
what a field test should measure.
