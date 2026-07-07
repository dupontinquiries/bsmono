/* corrupt -- Jhiaxus's Corrupted, and their generational drift.
 *
 * From the film script: the Corrupted are grown from Unicron's neural remnants,
 * and Jhiaxus's fatal error is that each generation drifts "slightly more
 * autonomous than the last." His experiment exceeds his containment models --
 * dozens now, each one further from anything he can predict or control.
 *
 * In-game each Corrupted is tagged with a GENERATION (spawn order, climbing as
 * the fight/waves go on). Later generations are more evolved: quicker, larger,
 * more mutated, and harder to shake. This module is the pure escalation curve --
 * bounded and monotonic so it never runs away. No rendering, no globals.
 */
#ifndef EX_CORRUPT_H
#define EX_CORRUPT_H

typedef struct {
  float speed_mul;    /* movement multiplier (>= 1) */
  float scale;        /* body-size multiplier (>= 1) -- later gens are bigger/mutated */
  float aggression;   /* 0..1: less hesitation, keener senses, harder to lose */
  float darken;       /* 0..1: tint toward corrupted black */
} ex_corrupt_drift_t;

/* Drift for a given generation (0 = baseline). Monotonic and capped. */
ex_corrupt_drift_t ex_corrupt_drift(int generation);

#endif /* EX_CORRUPT_H */
