/* test_waves.c -- wave escalation, scoring, clear-bonus, progression. */
#include "waves.h"
#include <stdio.h>
#include <assert.h>

int main(void){
  printf("=== escalation ===\n");
  ex_wave_makeup_t m1=ex_waves_makeup(1), m3=ex_waves_makeup(3), m6=ex_waves_makeup(6);
  printf("  w1 g%d c%d h%d | w3 g%d c%d h%d | w6 g%d c%d h%d\n",
    m1.grunts,m1.corrupted,m1.hijacks, m3.grunts,m3.corrupted,m3.hijacks, m6.grunts,m6.corrupted,m6.hijacks);
  assert(m1.corrupted==0 && m1.hijacks==0);      // wave 1: grunts only
  assert(m3.corrupted>0 && m3.hijacks>0);        // later waves add variety
  assert(m6.grunts>=m1.grunts);                  // escalates
  assert(m6.grunts<=6 && m6.corrupted<=3 && m6.hijacks<=5); // capped

  printf("=== progression (5-wave) ===\n");
  ex_waves_t w; ex_waves_init(&w, 5);
  assert(w.pending_spawn);                        // wave 1 ready immediately
  ex_wave_makeup_t mk; 
  int got = ex_waves_begin_next(&w, &mk);
  assert(got && w.wave==1 && !w.pending_spawn);
  // kill scoring
  long s0=w.score;
  ex_waves_on_kill(&w, 0);  // grunt +100
  ex_waves_on_kill(&w, 1);  // corrupted +250
  ex_waves_on_kill(&w, 3);  // apex +2000
  assert(w.score == s0+2350 && w.kills==3);
  printf("  score after 3 kills: %ld\n", w.score);

  // clear the wave (alive drops to 0) -> breather -> next wave spawns
  long before = w.score;
  ex_waves_update(&w, 0, 0.016f);                 // triggers clear bonus + intermission
  assert(w.score > before);                        // wave-clear bonus
  assert(w.intermission > 0.0f && !w.pending_spawn);
  // wait out the breather
  for(int i=0;i<60*4 && !w.pending_spawn;i++) ex_waves_update(&w, 0, 1.0f/60.0f);
  assert(w.pending_spawn);
  ex_waves_begin_next(&w, &mk);
  assert(w.wave==2);
  printf("  advanced to wave %d\n", w.wave);

  // run through to the final wave -> finished
  while(!w.finished){
    ex_waves_update(&w, 0, 0.016f);
    if(w.pending_spawn){ if(!ex_waves_begin_next(&w,&mk)) break; }
    // fast-forward intermission
    for(int i=0;i<60*4 && w.intermission>0;i++) ex_waves_update(&w,0,1.0f/60.0f);
  }
  printf("  finished at wave %d, score %ld\n", w.wave, w.score);
  assert(w.finished && w.wave==5);

  // endless mode: never finishes
  ex_waves_t e; ex_waves_init(&e, 0);
  ex_waves_begin_next(&e,&mk);
  for(int i=0;i<20;i++){ ex_waves_update(&e,0,0.016f);
    for(int k=0;k<300 && e.intermission>0;k++) ex_waves_update(&e,0,1.0f/60.0f);
    if(e.pending_spawn) ex_waves_begin_next(&e,&mk); }
  assert(!e.finished && e.wave>5);
  printf("  endless reached wave %d\n", e.wave);

  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
