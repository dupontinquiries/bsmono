# MERGE PLAYBOOK — why these merges are painless, and how to make them frictionless
Empirical: 15 rounds, ~90 cores from 3 parallel chats, ~all merged with ZERO code conflicts. The only
friction ever seen is `pub mod` hunks rejecting on a stale-base round — one line per core, auto-fixed.

## What makes it easy (keep doing)
1. **Pure additive cores.** Each core is ONE new file. New files at distinct paths never conflict.
   The entire merge surface is "new files + one decl line each."
2. **One registration file per thread.** SIM→`sim/mod.rs`, RENDER→`lib.rs`, WORLD→`world/mod.rs`.
   Threads touch DIFFERENT files, so even the `pub mod` edits don't collide across threads.
3. **R1 purity (no render/ash/vk, no cross-core imports).** Cores are independent — no shared-file
   edits, no ordering deps, each is `rustc --test`-able alone. Nothing to reconcile semantically.
4. **Cut every round on the LATEST merged base.** Then the `pub mod` hunks apply 100% clean (R11, R14,
   R15 = 0 rejects). Stale-base rounds reject ONLY on mod-line context shift; the idempotent `add_mod`
   (grep-or-append) fixes it in one pass. This is the whole friction budget.
5. **Bit-parity for microopts.** Every `batch_*` ships a scalar reference + a parity test in the same
   file, so a perf core cannot silently change behavior — merging it is risk-free.
6. **The harness is the gate.** `coupling_check` (purity) + `pure_test` (per-file rustc) + `run.sh`
   (full) verify each merge deterministically. GREEN = roll the base.
7. **Uniform returns.** `RETURN_KIT` auto-names `CHANGES-T0N-RM.patch` + the `8<…>8` status block →
   provenance + verification are scriptable, never bespoke.
8. **Per-thread DECISIONS fragments.** Each worker appends `DECISIONS-T0N.md`; coordinator files them
   under round headers. No shared-doc edits → no doc conflicts either.

## How to make it frictionless (ranked by leverage)
1. **★ Generated module registration — removes the LAST conflict class.** Today the only reject is the
   `pub mod` line. Kill it: workers drop cores into a dedicated dir (e.g. `sim/cores/`, `world/cores/`,
   `render_cores/`) and a `build.rs` writes a `cores.rs` that `include!`s each file (Rust won't auto-glob,
   but a build script can enumerate the dir). Then workers edit ZERO shared files → every round, even
   stale-base, applies 100% clean with no `add_mod` step. Highest-value change; one-time migration.
2. **Stamp the base revision into each kit.** Add a `BASE_REV` file (round + short hash) the RETURN_KIT
   echoes in the status block, so the coordinator knows exactly which base a patch was cut on — ends the
   workers' "merged through R9" guessing and tells you instantly whether to expect mod-line rejects.
3. **Coordinator re-cuts staged rounds onto each new base.** Already done when parking (R15 was re-cut on
   merged14 → 0 rejects). Make it standing: after each roll, refresh any staged round's base.
4. **Pre-merge dry-run.** Before the real apply, `patch --dry-run` each onto a throwaway copy and report
   rejects up front — catch base drift before touching `merged{N}`.
5. **Sorted `pub mod` insertion** (if keeping manual mod files). Workers insert alphabetically, so
   multiple insertions land at predictable, non-adjacent lines → fewer context overlaps. (Minor vs #1.)
6. **Standard batch-core layout.** `batch_<x>.rs` = scalar `<x>_one` ref + `<x>_batch` + parity test,
   always. Uniform = trivially reviewable; lets perf bonuses scale without per-core thought.

## Net
The discipline that made volume cheap: additive pure files + one reg-file per thread + cut-on-latest +
harness gate. The one improvement that would make it FREE: generated registration (#1) — then rounds of
any size (like this R16) merge with literally nothing to reconcile.
