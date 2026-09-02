# The wiring system — does it help or hurt? (2026-09-02)

What "the wiring system" is in the tree: `sim/service_registry.rs` +
`service_registry_batch.rs` + two generated `service_registry_gen*.rs`
(90 `#[distributed_slice(SERVICES)]` registrations), `sim/kernel_registry.rs`
+ two generated `kernel_registry_gen*.rs` (1 148 `#[distributed_slice(KERNELS)]`
registrations), `tools/bootstrap_registry.py` which generates them by scanning
the crate for pure functions whose parameters it can synthesize, 67 `batch_*`
modules (31 at the crate root, 36 in `sim/`) that are batched mirrors of scalar
kernels, and the `docs/WIRING_STATUS.md` ledger (12 303 lines). The fullgame
constructs a `ServiceHost::from_registry()` and a `KernelHost::new(8)` at
startup and, when `UNENDLICH_SIM_SWEEP=1`, ticks all services and sweeps an
eighth of the kernels every frame with synthesized inputs (`KernelCtx`: a
240-tick sawtooth and a triangle wave).

Sizes: 12 818 lines of registry code (12 027 generated), 5 119 lines of
`batch_*` modules, 1 204 module declarations across `lib.rs`/`sim`/`world`/
`render`. Per the ledger's own census: 1 025 engine modules, 185 reached from
the bin, ~840 orphans.

## What it was for, and what it actually does

It was built in a period when the windowed bin could not be compiled where
the code was written, to buy two things: (1) orphan modules get *linked* into
the shipping binary — `linkme` registrations survive dead-code elimination —
so "it compiles" became "it links and ships"; (2) with the sweep on, every
pure function is called every few frames on a moving input, a "perpetual
fuzz-lite" that trips `debug_assert`s and panics in dev.

It is opt-in now (r316) because (2) crashed the game: under `panic=abort` the
kernel host's `catch_unwind` cannot catch, and the service sweep never had one.
The default fullgame therefore does: instantiate ~90 service objects at startup
(`Colony::seeded(8, 42)`, `StormField::new(16, 16)`, `FaunaMind::new(...)`,
a fireball at the origin, …), print `[services] wired 90` and `[kernels]
wired 1148`, and never tick them.

## What it costs

- **Binary weight, always.** 1 148 kernel and 90 service registrations pin
  ~840 modules that nothing calls into `unendlich-fullgame`. The lib compiles
  them regardless; what the registrations add is *codegen and link* for code
  that would otherwise be dropped. On the phone that is download size and
  page-in; on the desktop it is link time on every build. **Unmeasured here**
  (a release link of the bin is beyond this sandbox). The measurement is one
  build with the `distributed_slice` attributes behind a feature and
  `cargo bloat --release --bin unendlich-fullgame -n 0` before/after, or
  `nm --size-sort` on both binaries.
- **Runtime, when on:** ~144 kernels/frame is genuinely small, but the
  services are not: `popdyn`, `storm_field`, `fauna_mind`, `adaptive_stream`,
  `locomotion_sim` are simulations ticked against a world that isn't there.
  Pure CPU with no gameplay effect, in the sim frame, and one of them panics
  on a degenerate synthesized input and the game is gone. That is why it is
  off. **When off: ~zero per frame** (a startup allocation burst and the
  prints). It is not a source of the stutters.
- **Duplicated code.** The 67 `batch_*` modules are documented as
  "bit-for-bit identical to calling the scalar kernel" and have no caller
  outside the registries. That is 5 119 lines that exist to give the
  registry something batch-shaped to tick. A future reader who finds
  `sim::boids` and `sim::batch_boids` has to work out which one is real
  (neither is called).
- **The word "wired".** The ledger is honest — its tiers are CHECKED / PROVEN /
  WIRED / RAN / KEPT and it says "almost nothing is WIRED" — but the startup
  line says `wired 90 registered services`. A registry tick with neutral
  inputs is not a call site. The thing the game actually needs for those
  modules (a real caller with real world inputs, per the registry's own doc)
  is exactly what the registry lets everyone stop looking for.
- **Generator coupling.** `bootstrap_registry.py` synthesizes arguments by
  type name. Any signature change in a pure kernel that the generator can't
  map silently drops it from the registry (or keeps a stale registration
  that no longer compiles). It is a second, weaker type system over 1 148
  functions.

## What it gives that is worth keeping

- `ServiceHost` / `Service` as a *shape* — a trait with `tick(&mut self,
  &ServiceCtx)` and a registry — is fine for the handful of stateful things
  that do have real inputs (music director, weather, the population model
  once it reads real creature counts). The mechanism is not the problem;
  the batch of neutral adapters is.
- The fuzz-lite idea has value as a **CI job**, not as a game-loop feature:
  a `sim-sweep` bin that links the registries and runs the sweep for N ticks
  under `panic=unwind` gives the "every kernel is exercised" property without
  touching the shipping binary.
- The census machinery (what's reached from the bin) is the useful output.
  Orphans are a design fact to act on, not to hide.

## Verdict

In its current shape it hurts more than it helps: it costs binary size and
link time on every build for a benefit (the sweep) that is turned off because
it crashed the game, it doubled 5 K lines of kernels into batch mirrors nobody
calls, and it makes "wired" mean "linked and ticked with fake inputs". The
stutters do not come from it (off by default), so it is not a performance
target; it is a weight-and-honesty target.

## The change (one afternoon, no hardware needed)

1. **Feature-gate the registrations.** `linkme` + both `*_gen*.rs` + the
   batch adapters behind `features = ["sim-registry"]`, default **off**. The
   fullgame stops pulling ~840 modules into its link. (`ServiceHost` stays
   available; a service that is hand-registered by a real call site keeps
   working without the feature.)
2. **`sim-sweep` bin** (features `sim-registry`, `panic = "unwind"` profile)
   that runs `tick_all` + `exercise()` for 2 400 ticks and exits non-zero on
   any panic. Run it in CI. Delete `UNENDLICH_SIM_SWEEP` from the fullgame.
3. **Delete the `batch_*` mirrors** that have no caller outside the registries
   (`orphan_check`'s duplicate-public-function pass lists them; today that is
   all 67). If a batch shape is ever needed it is a `for` loop over the
   scalar kernel at the call site.
4. **Re-label the ledger's 17 batch-wired services as `unwired`** and keep the
   census; the next real step for each is a call site with world inputs, and
   the ledger already knows which ones can never have one ("pure kernels…
   cannot be batch-wired meaningfully" — then they are `#[test]`ed library
   functions or deleted).
5. Measure step 1 on the box: bin size and link time before/after. That number
   decides whether to also revisit the 1 204 module declarations for
   `render/`-style consolidation, which is a different, larger job.
