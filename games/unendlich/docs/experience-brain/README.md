# Experience Brain — package contents

- **EXPERIENCE_BRAIN_GUIDE.md** — the guide. Read this first. Staged plan
  (Stage 0 → Stage 4), the performance constraint that shapes everything, where it
  plugs into the existing engine, and the ablation discipline.
- **sketch/lauscher_brain.rs** — a concrete, deterministic Stage-0/Stage-2 sketch
  grounded in the engine's `Goal`/`Signals`/`Disposition`/`MobMemory` types. Adapt to
  the tick loop; it is a sketch, not drop-in code.
- **reference/fsm.rs** — the weighted generative automaton (memory states +
  `evolve_toward` + structural operators) to port as the behaviour-grammar substrate.
- **reference/macros.rs** — the harvest → discover → **ablate** experience loop; the
  template for the experience tier and, just as importantly, how to validate it.

Build order: Stage 0 (ship + tune by hand) → Stage 1 (objectives) → Stage 2
(budgeted experience tier, ablated) → only then Stages 3–4 as measured prototypes.
