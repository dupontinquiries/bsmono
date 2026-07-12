#include "waves.h"
#include <string.h>

void ex_waves_init(ex_waves_t *w, int total_waves){
  memset(w,0,sizeof(*w));
  w->total_waves = total_waves;
  w->intermission = 0.0f;
  w->pending_spawn = 1;   // spawn wave 1 immediately
}

ex_wave_makeup_t ex_waves_makeup(int wave){
  ex_wave_makeup_t m;
  if (wave < 1) wave = 1;
  // escalate: more grunts each wave; corrupted from wave 2; hijacks from wave 3
  m.grunts    = 2 + wave;                      // 3,4,5,...
  m.corrupted = (wave >= 2) ? (wave / 2) : 0;  // 1,1,2,2,3...
  m.hijacks   = (wave >= 3) ? (wave - 1) : 0;  // 2,3,4...
  if (m.grunts > 6) m.grunts = 6;              // capped by encounter budget
  if (m.corrupted > 3) m.corrupted = 3;
  if (m.hijacks > 5) m.hijacks = 5;
  // TITAN WAVE: every 5th wave brings a titan-class apex boss (WFC's
  // Omega-Supreme-style set-piece). Thin the regular makeup so the boss
  // always gets a guaranteed spawn slot within the encounter budget.
  m.titan = (wave % 5 == 0) ? 1 : 0;
  if (m.titan) {
    if (m.grunts    > 4) m.grunts    = 4;
    if (m.corrupted > 2) m.corrupted = 2;
    if (m.hijacks   > 3) m.hijacks   = 3;
  }
  return m;
}

int ex_waves_begin_next(ex_waves_t *w, ex_wave_makeup_t *out){
  if (w->total_waves > 0 && w->wave >= w->total_waves) { w->finished = 1; return 0; }
  w->wave++;
  w->pending_spawn = 0;
  w->intermission = 0.0f;
  if (out) *out = ex_waves_makeup(w->wave);
  return 1;
}

long ex_waves_energon_value(int enemy_kind){
  // kind order matches enemy.h: 0 grunt,1 corrupted,2 hijack,3 apex
  static const long ev[4] = { 20, 40, 30, 120 };
  return (enemy_kind>=0 && enemy_kind<4) ? ev[enemy_kind] : 20;
}

void ex_waves_on_kill(ex_waves_t *w, int enemy_kind){
  // scores by kind: grunt=100, corrupted=250, hijack=60, apex=2000 (kind order
  // matches enemy.h: 0 grunt,1 corrupted,2 hijack,3 apex)
  static const long pts[4] = { 100, 250, 60, 2000 };
  long p = (enemy_kind>=0 && enemy_kind<4) ? pts[enemy_kind] : 100;
  w->score += p;
  w->kills++;
  w->energon += ex_waves_energon_value(enemy_kind);
}

void ex_waves_update(ex_waves_t *w, int alive, float dt){
  w->forge_event = 0;
  w->forge_hp_amount = 0.0f;
  if (w->finished) return;
  if (w->intermission > 0.0f) {
    w->intermission -= dt;
    if (w->intermission <= 0.0f) { w->intermission = 0.0f; w->pending_spawn = 1; }
    return;
  }
  // a wave is active (>=1) and everything's dead -> clear bonus + breather
  if (w->wave >= 1 && alive == 0 && !w->pending_spawn) {
    w->score += 500L * w->wave;   // wave-clear bonus grows
    // FORGE: the whole banked energon converts to a permanent integrity gain
    // (plus a full repair, applied by the caller) -- playing greedy (banking
    // more energon before the wave clears) compounds into a stronger you.
    if (w->energon > 0) {
      w->forge_hp_amount = (float)w->energon * 0.4f;
      w->forged_total += w->energon;
      w->energon = 0;
      w->forge_event = 1;
    }
    if (w->total_waves > 0 && w->wave >= w->total_waves) { w->finished = 1; return; }
    w->intermission = 3.0f;       // breather before the next wave
  }
}

int ex_waves_spend(ex_waves_t *w, long cost){
  if (cost < 0 || w->energon < cost) return 0;
  w->energon -= cost;
  return 1;
}
