/* waves
  Turns a match into a game loop: successive waves of escalating enemy makeups,
  a running score for kills (bonuses for tougher foes and wave-clear), and a
  wave counter. Between waves there is a short breather; when a wave is cleared
  the next spawns bigger. Endless (survival) or a fixed number of waves. Pure
  logic; the game supplies enemy spawns and reports kills.
*/
#ifndef EX_WAVES_H
#define EX_WAVES_H

#include <stdint.h>

typedef struct {
  int   wave;            // current wave (1-based); 0 = not started
  int   total_waves;     // 0 = endless
  float intermission;    // seconds until the next wave spawns (>0 = breather)
  int   pending_spawn;   // set the frame the game should spawn the next wave
  long  score;
  int   kills;
  int   finished;        // all waves cleared (win)

  // Escalation's energon economy: kills bank energon credits; on wave clear
  // the whole bank is FORGED into a permanent integrity increase + full
  // repair, so surviving compounds your strength. Harmless/unused by the
  // other modes.
  long  energon;         // currently banked credits (spent on forge)
  long  forged_total;    // lifetime forged credits (display only)
  int   forge_event;     // set the frame a wave-clear forge fires
  float forge_hp_amount; // permanent max-integrity granted this forge event
} ex_waves_t;

typedef struct { int grunts, corrupted, hijacks; int titan; } ex_wave_makeup_t;

void ex_waves_init(ex_waves_t *w, int total_waves);

/**
 * [ex_waves_makeup enemy composition for a given wave (escalating). Every 5th
 *  wave is a TITAN wave (out.titan=1): a titan-class apex boss joins the
 *  fight, and the regular makeup is thinned so the boss always gets a
 *  guaranteed spawn slot within the encounter budget.]
 */
ex_wave_makeup_t ex_waves_makeup(int wave);

/**
 * [ex_waves_begin_next advance to the next wave; fills the makeup to spawn.
 *  Returns 0 if there are no more waves (finished).]
 */
int ex_waves_begin_next(ex_waves_t *w, ex_wave_makeup_t *out);

/**
 * [ex_waves_on_kill award score for a kill of the given enemy kind, and bank
 *  energon credits toward the next forge (Escalation's economy).]
 */
void ex_waves_on_kill(ex_waves_t *w, int enemy_kind);

/**
 * [ex_waves_energon_value energon credits banked for a kill of this kind:
 *  grunt 20 / hijack 30 / corrupted 40 / apex 120.]
 */
long ex_waves_energon_value(int enemy_kind);

/**
 * [ex_waves_update tick the intermission; when a wave is cleared (alive==0 and
 *  a wave is active) start the breather, flag pending_spawn, and fire the
 *  Escalation forge (banked energon -> forge_hp_amount, forge_event=1,
 *  energon reset to 0, forged_total accumulated).]
 */
void ex_waves_update(ex_waves_t *w, int alive, float dt);

/**
 * [ex_waves_spend try to spend `cost` banked energon on a mid-run purchase
 *  (Escalation's buy system: perks, gates). Returns 1 and deducts on
 *  success, 0 (unchanged) if the bank can't cover it.]
 */
int ex_waves_spend(ex_waves_t *w, long cost);

#endif // EX_WAVES_H
