/* absorb -- the Corrupted absorbing metal to grow.
 *
 * From the film script / Jhiaxus design: a fully-corrupted unit doesn't just
 * attack -- it CONSUMES. It pulls in loose metal and wreckage and grows, its
 * mass and reach swelling the longer a fight (and its debris) lasts.
 *
 * `grow` is a multiplier that starts at 1.0 and climbs as the unit absorbs,
 * saturating toward a cap. This module is the pure curve: accumulate absorbed
 * mass into grow, and derive the size/damage scaling from it. No globals.
 */
#ifndef EX_ABSORB_H
#define EX_ABSORB_H

#define EX_ABSORB_GROW_MAX 2.2f   /* cap on the grow multiplier */

/* Fold `amount` of absorbed mass into `grow` (>=1), with diminishing returns as
   it nears the cap. Never exceeds EX_ABSORB_GROW_MAX. */
float ex_absorb_accumulate(float grow, float amount);

typedef struct {
  float scale;    /* body-size multiplier (>=1) */
  float dmg_mul;  /* melee/impact damage multiplier (>=1) */
} ex_absorb_stats_t;

/* Size + damage scaling for a grow value. Baseline (1,1) at grow<=1, rising to
   its ceiling at grow==EX_ABSORB_GROW_MAX. */
ex_absorb_stats_t ex_absorb_stats(float grow);

#endif /* EX_ABSORB_H */
