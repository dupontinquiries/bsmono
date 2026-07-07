/* matrix -- the Matrix of Leadership, and the Rodimus apotheosis.
 *
 * From the film script's pivotal beat: the reckless scout takes up the Matrix at
 * the worst moment and rises out of the light as Rodimus. Unlike Photon Man (the
 * Watch's AUTO-triggered defensive last stand), the Matrix is CLAIMED -- a relic
 * you find and take up on purpose -- and its gift is offensive: a golden power
 * spike that turns the tide.
 *
 * This module is the pure state timer: claim it, tick it, read whether Rodimus is
 * active and how bright. No auto-trigger, no once-per-life lock -- claiming again
 * refreshes it. Testable in isolation.
 */
#ifndef EX_MATRIX_H
#define EX_MATRIX_H

#define EX_MATRIX_DURATION 15.0f   /* seconds of Rodimus (longer than Photon's burst) */

typedef struct { float timer; } ex_matrix_t;

void  ex_matrix_reset(ex_matrix_t *m);          /* clear (on deploy) */
void  ex_matrix_claim(ex_matrix_t *m);          /* take up the Matrix -> start/refresh Rodimus */
void  ex_matrix_update(ex_matrix_t *m, float dt);
int   ex_matrix_active(const ex_matrix_t *m);   /* 1 while Rodimus */
float ex_matrix_glow(const ex_matrix_t *m);     /* 0..1 aura intensity (fades in/out) */

#endif /* EX_MATRIX_H */
