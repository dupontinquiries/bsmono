# CLAUDE.md — brief for the testing agent

You are being asked to field-test Drift. Read this whole file before writing
code. The hard part of this project is not implementation; it is *not letting
the concept bloat back into a 20-feature pile.* Most of your value is in
holding the discipline below.

## Mission

1. Confirm the MVP (the two-sided boundary journal in `playground/`) does what
   it claims, on a real boundary.
2. Optionally build the next slice (see priority order).
3. Report back on the **one question that actually matters** (see end).

## The discipline (these are rules, not suggestions)

**1. Boundaries only.** Drift annotations belong only at seams where two parts
change *independently*: Rust↔QML, service↔service, team-A-module↔team-B-consumer,
schema↔persisted-data. Internal code recompiles together — it can't drift
against itself, and the compiler already covers it. Never annotate the inside
of a function with mood/purpose tags. That sprawl is the failure mode.

**2. Every feature needs a verifier.** A Drift construct is real only if there
is an *observable signal* that confirms or refutes its claim. "This is
temporary" has a verifier (time + presence). "This is political" does not —
nothing distinguishes `Political<T>` from `T`. If you can't name the verifier,
it's decoration; don't build it.

**3. Do not build the fantasy features.** Scope forecasting ("this module will
become an OrderSystem"), regret-as-judgment, and the joke types
(`Political`, `Human`, `std::hope`, `cargo-cult`) all fail rule 2. They were
comedy in the original brainstorm. Leave them there.

**4. Respect module purity.** Some modules are deliberately kept out of the
crate's dependency graph (e.g. a pure download helper with no `clog!` / crate
refs). Do **not** inject logging or `crate::` macros into those to instrument
them. Instrument at the call sites, or run the boundary scan as a standalone
pass *outside* the crate graph. Preserving what the author kept pure is part of
the job.

**5. Annotations must be cheap or derived.** The existential risk is not
feasibility — it's that hand-written annotations rot and get deleted by week
three. Prefer inferring the baseline and asking the human to assert only the
*delta from default*. Where a hint can be derived mechanically (e.g. escaping
effects — see docs/04), derive it; don't make a human author it.

**6. False positives kill trust.** Every warning must be acknowledgeable, and
the acknowledgment must become *data* (it goes in the journal). The debt ledger
is not a thing you curate — it is the accumulated log of drifts the team chose
to accept, on the record. Suppressing a warning updates the model; it does not
hide it.

## Priority order for what to build

1. **Run the playground both ways.** Confirm the three-way diagnostic
   (healthy / broken wire / dead surface). This is the whole thesis in 60 lines.
2. **Correlation across a real boundary.** The journal currently matches on a
   string intent name. For a real Rust↔QML test, add a shared correlation id so
   a producer event and a consumer event can be matched even across the
   language gap, and disambiguate the two failure modes:
   - consumer attempted, producer never serviced → broken wire
   - consumer never attempted → upstream path never ran (component never built,
     guard returned early)
   That disambiguation is the exact thing a human debugger burns a session on.
3. **`#[temporary(...)]` with the expiry hierarchy** (docs/02). Prefer
   successor-conditioned expiry (`expires_when = exists("queue_delivery")`) over
   a version, over a date.
4. **(v2, harder, higher false-positive risk)** A *static* reachability scan
   over the .qml files: enumerate the Rust methods exposed to QML, scan QML for
   `backend.<method>(...)` call patterns, flag calls to methods that don't exist
   and exposed methods no QML references. This catches the never-run case the
   runtime journal can't see. Build it as a standalone pass (rule 4).

## What NOT to do

- Do not rewrite the journal into a new language or a custom compiler.
- Do not add per-function mood annotations (`#[fragile]`, `#[political]`, ...).
- Do not build scope forecasting or regret analysis.
- Do not break module purity to add instrumentation.
- Do not invent `cargo drift` output — those subcommands aren't implemented.

## The one question to report back on

Not "does the journal work" — it does. The real question is:

> **Would this have ended the debugging session in one run instead of many,
> and would the annotations still be alive after three weeks of churn?**

If the answer to the first is yes and you have any evidence on the second,
that's the field-test result worth having.
