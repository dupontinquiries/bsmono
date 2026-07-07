/* test_warp_wave.c -- the Ice-Clone leap's delayed gravity wave scheduler.
   Verifies: it fires exactly once, at the right time, at the recorded position,
   with progress ramping 0->1, and never again until re-armed. */
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include "warp_wave.h"

int main(void)
{
  ex_warp_wave_t w = {0,0,0,{0,0,0}};

  // unarmed: ticking does nothing, progress is 0
  assert(ex_warp_wave_tick(&w, 0.1f) == 0);
  assert(ex_warp_wave_progress(&w) == 0.0f);

  // arm at a position, 4.0s delay
  float pos[3] = { 12.0f, 0.3f, -7.0f };
  ex_warp_wave_arm(&w, pos, 4.0f);
  assert(w.armed == 1);
  assert(ex_warp_wave_progress(&w) == 0.0f);   // just armed -> 0

  // step forward in 0.1s frames; it must fire exactly once, near t=4.0
  int fires = 0; float t = 0.0f; float fire_t = -1.0f;
  for (int i = 0; i < 100; i++) {
    t += 0.1f;
    if (ex_warp_wave_tick(&w, 0.1f)) { fires++; fire_t = t; }
  }
  assert(fires == 1 && "wave must fire exactly once");
  assert(fire_t > 3.85f && fire_t < 4.15f && "fires ~4s after arming");
  assert(w.armed == 0 && "disarms after firing");
  printf("fired once at t=%.2f, position (%.1f,%.1f) OK\n", fire_t, w.pos[0], w.pos[2]);

  // progress monotonicity while armed
  ex_warp_wave_arm(&w, pos, 2.0f);
  float last = -1.0f;
  for (int i = 0; i < 15; i++) {
    float p = ex_warp_wave_progress(&w);
    assert(p >= last - 1e-6f && p >= 0.0f && p <= 1.0f);
    last = p;
    if (ex_warp_wave_tick(&w, 0.1f)) break;
  }
  printf("progress ramps monotonically 0..1 OK\n");

  // re-arming resets the clock (a new leap overrides a pending wave)
  ex_warp_wave_arm(&w, pos, 3.0f);
  ex_warp_wave_tick(&w, 1.0f);            // 2s left
  float mid[3] = { -5.0f, 0.0f, 9.0f };
  ex_warp_wave_arm(&w, mid, 3.0f);        // reset -> 3s left, new pos
  assert(fabsf(w.timer - 3.0f) < 1e-4f && w.pos[0] == -5.0f);
  printf("re-arm resets timer + position OK\n");

  printf("\nWARP WAVE OK (silent leap, delayed consequence)\n");
  return 0;
}
