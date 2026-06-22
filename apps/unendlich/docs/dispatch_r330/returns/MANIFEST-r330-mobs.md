# MANIFEST — r330-mobs (roamer spawn-admission cap)

## Task
T-mobs: build the pure admission core the spawn director consults so spawning
never pushes roamer population past the cap (`NEXT_SESSION_GUIDE.md §4d`:
73 vs cap 8). **Spawn-side ONLY — no despawn/eviction** (`WIRING_STATUS.md r300`:
operator declined population caps; herd kept intact).

## Files
- NEW  crates/unendlich-engine/src/sim/roamer_cap.rs   (core + 8 tests, self-contained)
- EDIT crates/unendlich-engine/src/sim/mod.rs           (+1 line: `pub mod roamer_cap;`)

Both are in CHANGES-r330-mobs.patch (git diff, applies `-p1` at repo root).
Raw new .rs also shipped at its repo path. Handoff: HANDOFF_roamer_cap_wiring.md.

## API added (pure, alloc-free, saturating; no unwrap/expect/panic/IO/RNG/clock)
- `roamer_cap::admit(current: u32, cap: u32, requested: u32) -> u32`
    = min(requested, cap - current); 0 when current >= cap.
    Invariant: current + admit(..) <= cap, for all inputs.
- `roamer_cap::overflow_count(current: u32, cap: u32) -> u32`
    = current - cap (saturating). DIAGNOSTIC ONLY — not a despawn list.
- `roamer_cap::headroom(current: u32, cap: u32) -> u32`  (convenience) = cap - current (sat).

Consistency: `admit(c,cap,r) == batch_spawn_budget::allowed_one(r, u32::MAX, c, cap)`
(the cap-only specialization of the existing spawn-budget rule) — asserted in tests.

## No-despawn guarantee
Module has no removal path: every fn returns a non-negative spawn count or a
read-only diagnostic; none can shrink the live population. Test
`never_implies_removal` asserts this. Over-cap → admit 0 → herd settles naturally.

## Verify (ran in-sandbox)
Toolchain: stock `rustc 1.75.0` from the Ubuntu archive (offline `rustup.zip`
bundle absent this session; module is dependency-free edition-2021 `sim::` code,
so the documented standalone `rustc --test` floor applies — no crate:: refs).

- Standalone gate (SHARED_RULES §3):
    `rustc --edition 2021 --test roamer_cap.rs -o /tmp/t && /tmp/t`
    -> running 8 tests ... test result: **ok. 8 passed; 0 failed**.
- Warning-clean library build:
    `rustc --edition 2021 --crate-type lib -D warnings roamer_cap.rs` -> CLEAN.
- Patch applies on a fresh base:
    `git apply -p1 --check CHANGES-r330-mobs.patch` -> OK; applied; re-ran gate
    on the applied tree -> 8 passed / 0 failed.
- Panic-surface audit of non-test code: no unwrap/expect/panic/assert (only the
  doc line stating the guarantee matches).

Tests (8, brief asked >=6): admit_never_exceeds_cap (50k-iter fuzz incl. edges +
u32::MAX), cap_zero_admits_nothing, at_or_over_cap_admits_zero (incl. the §4d
73-vs-8 case), fills_available_headroom, saturating_no_overflow,
overflow_count_is_pure_diagnostic, matches_spawn_budget_rule_and_deterministic
(10k-iter parity vs allowed_one), never_implies_removal.

## Did NOT do (scope/rules)
- No edit to mob_spawn_director.rs / batch_spawn_budget.rs / the bin — wiring is
  PROPOSED in HANDOFF_roamer_cap_wiring.md (SHARED_RULES §1–§2,§4).
- No despawn/eviction path (r300 constraint).
- No full-workspace `cargo build` (coordinator runs on merge; standalone gate is
  the documented substitute for a dependency-free sim module).
- Did not add the optional director call-site to avoid a shared-file edit before
  the mobs merge slot (items -> bughunt -> mobs); proposed as option B instead.

## Merge note
Merge slot 3 (items -> bughunt -> mobs -> ...). Touches only the new file + one
`pub mod` line; the mod.rs line was appended at EOF (after `world_prober`) to
mirror the r330 `net_channels` convention and minimize reconcile conflict.
