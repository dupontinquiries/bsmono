/* thorns -- the Watch's vine form: a spreading thorn field.
 *
 * Completes six of the seven elemental clone powers (ice/fire/water/electric/air,
 * now vine). Cast it and brambles erupt from the ground and spread outward from
 * where you stand, holding a patch of the arena: anything caught in the thorns is
 * torn at and snared for as long as it lingers. It's the game's first PLAYER-placed
 * persistent area hazard -- zone control, where the others are bursts.
 *
 * This module is the pure field geometry -- how wide the patch has grown, and how
 * fierce the thorns are at a given point. No globals, no rendering -> testable.
 */
#ifndef EX_THORNS_H
#define EX_THORNS_H

#define EX_THORNS_RADIUS   30.0f   /* full spread */
#define EX_THORNS_DURATION 6.0f    /* seconds the patch persists */
#define EX_THORNS_GROW     1.2f    /* seconds to spread to full radius */

/* Live radius at `age` seconds: spreads 0 -> max_r over grow_time, then holds at
   max_r. Clamped to [0, max_r]. */
float ex_thorns_radius(float age, float max_r, float grow_time);

/* Thorn intensity (0..1) for a point at `dist` from the centre: 1 at the centre,
   tapering to 0 at the live `radius`, and 0 outside. Callers scale damage/snare
   by this. */
float ex_thorns_intensity(float dist, float radius);

#endif /* EX_THORNS_H */
