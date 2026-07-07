/* graviton -- Unicron's graviton well.
 *
 * From the film script: the devourer's gravity is a force in its own right --
 * it tears satellites out of orbit and bends everything nearby toward itself.
 *
 * In-game the apex/Unicron-tier boss projects a persistent well: a constant
 * inward pull, weak at the rim and stronger toward the center, that you must
 * fight to hold your distance. Kill the boss and the well collapses. This module
 * is the pure pull field -- direction + magnitude for a point. No globals.
 */
#ifndef EX_GRAVITON_H
#define EX_GRAVITON_H

#define EX_GRAVITON_RADIUS 95.0f

/* Inward pull acceleration for point `p` toward `center` (xz-plane).
   Returns 0 outside `radius` or at the exact center. Linear falloff: `strength`
   at the center, tapering to 0 at the rim. If out_dir != NULL it receives the
   normalized xz direction toward the center (zeroed when there's no pull). */
float ex_graviton_pull(const float center[3], const float p[3],
                       float radius, float strength, float out_dir[3]);

#endif /* EX_GRAVITON_H */
