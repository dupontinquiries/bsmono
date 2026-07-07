/* test_matrix.c -- the Matrix of Leadership -> Rodimus state.
   Verifies claim starts the state, it runs for its duration then ends, claiming
   again refreshes it (no once-per-life lock, unlike Photon), glow ramps, and
   reset clears it. */
#include <stdio.h>
#include <assert.h>
#include "matrix.h"

int main(void)
{
  ex_matrix_t m; ex_matrix_reset(&m);
  assert(!ex_matrix_active(&m) && ex_matrix_glow(&m) == 0.0f);

  // claim -> Rodimus active
  ex_matrix_claim(&m);
  assert(ex_matrix_active(&m));
  printf("claim activates Rodimus OK\n");

  // runs ~DURATION then ends
  float t = 0.0f; int ended_at = -1;
  for (int i=0;i<300 && ex_matrix_active(&m);i++){ t += 0.1f; ex_matrix_update(&m, 0.1f);
    if (!ex_matrix_active(&m)) ended_at = i; }
  assert(!ex_matrix_active(&m));
  assert(t > EX_MATRIX_DURATION-0.25f && t < EX_MATRIX_DURATION+0.25f);
  printf("runs ~%.0fs then ends (%.1fs) OK\n", (double)EX_MATRIX_DURATION, t);

  // claiming again refreshes (unlike Photon's once-per-life lock)
  ex_matrix_claim(&m);
  assert(ex_matrix_active(&m));
  ex_matrix_update(&m, 5.0f);
  assert(ex_matrix_active(&m));
  ex_matrix_claim(&m);                 // refresh back to full
  ex_matrix_update(&m, 5.0f);
  assert(ex_matrix_active(&m) && "refresh extends it");
  printf("re-claim refreshes OK\n");

  // glow is within [0,1] while active, 0 when inactive
  ex_matrix_reset(&m);
  assert(ex_matrix_glow(&m) == 0.0f);
  ex_matrix_claim(&m);
  ex_matrix_update(&m, 0.6f);
  { float g = ex_matrix_glow(&m); assert(g > 0.0f && g <= 1.0f); }
  printf("glow bounded OK\n");

  printf("\nMATRIX OK (claimed, refreshable Rodimus state)\n");
  return 0;
}
