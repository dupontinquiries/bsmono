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
} ex_waves_t;

typedef struct { int grunts, corrupted, hijacks; } ex_wave_makeup_t;

void ex_waves_init(ex_waves_t *w, int total_waves);

/**
 * [ex_waves_makeup enemy composition for a given wave (escalating)]
 */
ex_wave_makeup_t ex_waves_makeup(int wave);

/**
 * [ex_waves_begin_next advance to the next wave; fills the makeup to spawn.
 *  Returns 0 if there are no more waves (finished).]
 */
int ex_waves_begin_next(ex_waves_t *w, ex_wave_makeup_t *out);

/**
 * [ex_waves_on_kill award score for a kill of the given enemy kind]
 */
void ex_waves_on_kill(ex_waves_t *w, int enemy_kind);

/**
 * [ex_waves_update tick the intermission; when a wave is cleared (alive==0 and
 *  a wave is active) start the breather, then flag pending_spawn]
 */
void ex_waves_update(ex_waves_t *w, int alive, float dt);

#endif // EX_WAVES_H
