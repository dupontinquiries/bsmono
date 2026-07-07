/* arc -- the Watch's electric form: chain lightning.
 *
 * Completes the clone-power set alongside ice (warp leap), fire (star), and water
 * (tsunami). The electric form looses a bolt that leaps from foe to foe: it grounds
 * itself through the nearest target, then arcs to the nearest one it hasn't hit yet,
 * and again, until it runs out of reach or jumps.
 *
 * This module is the pure targeting: build the chain order. No damage, no vfx, no
 * globals -> testable.
 */
#ifndef EX_ARC_H
#define EX_ARC_H

#define EX_ARC_JUMPS 5        /* max targets in a chain */
#define EX_ARC_RANGE 30.0f    /* max reach of each jump */

/* Build the chain: from `origin`, repeatedly hop to the nearest live enemy not yet
   hit and within `range` of the last hit, up to `max_jumps`. Enemy positions in
   `epos[n][3]`; `alive[i]`!=0 means targetable (NULL = all alive). Fills out_chain
   with enemy indices in strike order; returns the count (0 if none in reach). */
int ex_arc_chain(const float origin[3], const float (*epos)[3], const int *alive,
                 int n, float range, int max_jumps, int *out_chain);

#endif /* EX_ARC_H */
