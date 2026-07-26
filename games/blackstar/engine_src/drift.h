/* drift.h -- DRIFT SECTORS: the pure rules for patrol's uncharted space-bridge jumps.
   The escalation loop cycles 12 authored arenas; every THIRD jump instead tears to a
   DRIFT SECTOR -- an authored arena's terrain re-dressed with freshly generated cover
   (the mapgen graph->blueprint pipeline re-seeded), named like a chart annotation.
   Kept as pure static-inline rules so test_drift can pin them without the engine:
     - cadence: jumps 2, 5, 8, ... (every 3rd) drift;
     - base rotation keys on the DRIFT index k = jump/3 with a stride COPRIME to the
       arena count, so successive drifts walk ALL 12 themes (a jump-indexed stride of
       3-step jumps only ever visits 4 -- the first draft's arithmetic bug, now a test);
     - names are deterministic: GREEK[k] over the base theme;
     - seeds are deterministic per (jump, mission) and distinct per jump. */
#ifndef EX_DRIFT_H
#define EX_DRIFT_H

static inline int ex_drift_is(int jump) { return (jump % 3) == 2; }

/* base-theme index for drift number k (= jump/3), in [0, n): stride 5 is coprime
   to 12 so k = 0..11 is a full permutation of the themes. */
static inline int ex_drift_base(int k, int n) { return n > 0 ? (k * 5 + 2) % n : 0; }

static inline const char *ex_drift_greek(int k) {
  static const char *g[8] = { "ALPHA","BETA","GAMMA","DELTA","THETA","SIGMA","OMEGA","KAPPA" };
  return g[k & 7];
}

static inline unsigned ex_drift_seed(int jump, int mission) {
  return 0xD21F7A11u ^ (unsigned)jump * 2654435761u ^ (unsigned)mission * 101u;
}

#endif
