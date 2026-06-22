# 03 — Roadmap

## Done

- **Two-sided boundary journal (MVP).** Working in `playground/`. Records
  producer "serviced" and consumer "attempted" events, diffs them into
  healthy / broken-wire / dead-surface. Zero-cost when the feature is off.

## Next slices (in order)

1. **Cross-boundary correlation.** Match producer and consumer events by a
   shared id, not just an intent-name string, so the journal works across a
   real Rust↔QML gap. This is what disambiguates "wire broke" from "path never
   ran" — the two branches a human debugger burns a session distinguishing.
2. **`#[temporary]` + expiry escalation.** Successor-conditioned > version >
   date. Warning escalates to error as the deadline passes / the successor
   appears.
3. **`cargo drift journal` / `cargo drift debt`.** Promote the
   `drift_report!()` call into real subcommands; persist acknowledged drifts to
   a `.drift/` ledger so debt accumulates as data.

## v2 (harder, higher false-positive risk)

4. **Static QML reachability scan (`cargo drift check`).** Enumerate Rust
   methods exposed to QML, scan .qml for `backend.<method>(...)`, flag
   nonexistent calls and unreferenced surfaces. Standalone pass, outside the
   crate graph. Catches the *never-run* case the runtime journal cannot see.
5. **Intent state-machine contracts.** Tagged terminal/non-terminal enum
   variants; flag when a consumer assumes terminal and the producer gains a
   non-terminal variant. Latency budgets checked via the journal.
6. **Derived `cargo drift outline`** for fast (human and AI) reading — see
   docs/04.

## Probably never (fails the verifier test)

- Scope forecasting ("module X becomes Y in 3 months" via "naming entropy").
- Regret-as-judgment narration.
- `Political<T>` / `Human<T>` / `std::hope` as real constructs.

These were comedy in the original brainstorm. They have no verifier. They stay
comedy.

## The open risk that the field test must answer

The technical path is clear and feasible. The risk is human:

> **Do the boundary annotations survive past week three of real churn?**

If annotating a seam costs more than the bug it prevents, people delete the
annotations and Drift becomes dead weight. The mitigation hypothesis — infer
the baseline, have the human confirm only the delta, backfill everything
derivable — is untested. A good field test is designed to measure annotation
survival, not just whether the journal prints.
