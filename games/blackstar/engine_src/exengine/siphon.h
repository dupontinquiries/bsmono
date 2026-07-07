/* siphon -- the devourer feeds (Unicron / Blackstar's "entropy profit").
 *
 * From the film script: the antagonist doesn't just fight, it FEEDS -- draining
 * energy from what it fights and mending itself with the take. In-game the boss
 * siphons your spark when you linger in close: strongest at point-blank, fading
 * to nothing at the rim, so it punishes camping in its face and rewards breaking
 * range. Pairs with the graviton well (which drags you into siphon range).
 *
 * This module is the pure drain-rate curve. No globals, no rendering -> testable.
 */
#ifndef EX_SIPHON_H
#define EX_SIPHON_H

#define EX_SIPHON_RADIUS 40.0f   /* inner feeding field (well radius is larger) */
#define EX_SIPHON_RATE   14.0f   /* hp/sec drained at point-blank */

/* Drain rate (hp/sec) for a target at `dist` from the feeder: `max_rate` at the
   center, tapering linearly to 0 at `radius`. 0 at/beyond the rim. */
float ex_siphon_rate(float dist, float radius, float max_rate);

#endif /* EX_SIPHON_H */
