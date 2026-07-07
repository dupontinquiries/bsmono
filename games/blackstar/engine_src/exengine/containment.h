/* containment -- the failing Pluto containment field.
 *
 * From the film script's cosmology: the technocrats could not destroy Unicron, so
 * they contained him -- Earth's mantle a prison, the Outer Rim a graveyard, and
 * the Pluto region a containment field. As his apparatus stirs, that field is
 * failing: the safe boundary draws inward, and beyond it is the consuming void.
 *
 * In-game this is a survival mechanic: a perimeter that contracts as the clock
 * runs down, punishing anyone caught outside it. This module is the pure geometry
 * -- the current radius over time, and how far a point has breached it. No globals.
 */
#ifndef EX_CONTAINMENT_H
#define EX_CONTAINMENT_H

#define EX_CONTAIN_R0 140.0f   /* opening radius (nearly the whole arena) */
#define EX_CONTAIN_R1 52.0f    /* fully-closed core (still livable) */

/* Boundary radius: shrinks linearly from r0 to r1 as `elapsed` goes 0..close_time,
   clamped to [r1, r0] (assumes r1 < r0). */
float ex_containment_radius(float elapsed, float r0, float r1, float close_time);

/* How far a point at `dist` from the centre is OUTSIDE `radius` (0 if inside). */
float ex_containment_breach(float dist, float radius);

#endif /* EX_CONTAINMENT_H */
