/* photon -- Photon Man, the Watch's unreachable ceiling.
 *
 * From the film script: the thirteenth form is not one the bearer can choose.
 * The Watch shows it "like a door that is warm to the touch but has no handle."
 * It has only ever activated ONCE, on its own, in a battle where every other
 * form was cycled through and none was enough. The bearer does not decide the
 * moment -- the Watch does -- and afterward cannot summon it again.
 *
 * So in-game Photon Man is an AUTO-triggered last stand, not a button: when the
 * bearer is about to be overwhelmed (critically low, under real threat) the Watch
 * steps in for a short window of overwhelming power -- then locks out for the
 * rest of the life. Pure state machine; no rendering, no game globals -> testable.
 */
#ifndef EX_PHOTON_H
#define EX_PHOTON_H

#define EX_PHOTON_HP_TRIGGER 0.16f   /* the Watch only steps in when nothing else is enough */
#define EX_PHOTON_DURATION   8.0f    /* seconds of light form (the script's ~40s, gameplay-scaled) */

typedef struct {
  int   state;   /* 0 idle, 1 active, 2 spent (locked for this life) */
  float timer;   /* active seconds remaining */
  float total;   /* the duration it activated with (for glow ramp) */
} ex_photon_t;

/* Reset to idle+unspent. Call at deploy / respawn. */
void ex_photon_reset(ex_photon_t *p);

/* Advance one frame.
   hp_frac      : player health 0..1
   under_threat : 1 if genuinely in danger (recent damage / enemies on you)
   eligible     : 1 if the bearer can access it right now (full transformer, in play)
   Returns 1 on the SINGLE frame it activates; 0 otherwise. */
int ex_photon_update(ex_photon_t *p, float hp_frac, int under_threat, int eligible, float dt);

int   ex_photon_active(const ex_photon_t *p);   /* 1 while in light form */
int   ex_photon_spent(const ex_photon_t *p);    /* 1 once used this life */
float ex_photon_glow(const ex_photon_t *p);     /* 0..1, fades in fast, out slow */

#endif /* EX_PHOTON_H */
