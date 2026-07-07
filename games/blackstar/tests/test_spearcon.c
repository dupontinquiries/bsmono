/* test_spearcon.c -- Cryotek's sticky countdown charges (pool).
   Verifies plant/capacity, fuse countdown, single detonation reporting the
   right position, progress ramp, and slot reuse after detonation. */
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include "spearcon.h"

int main(void)
{
  ex_spearcon_pool_t p; ex_spearcon_reset(&p);
  assert(ex_spearcon_active_count(&p) == 0);

  float a[3] = { 5.0f, 0.3f, -2.0f };
  int i = ex_spearcon_plant(&p, a, 2.0f);
  assert(i >= 0 && ex_spearcon_active_count(&p) == 1);
  assert(ex_spearcon_progress(&p.s[i]) == 0.0f);

  // countdown: no detonation before the fuse; exactly one at the end, at pos a
  float det[EX_SPEARCON_MAX][3];
  int total = 0; float t = 0.0f; float fired_x = -999.0f;
  for (int k=0;k<30;k++){
    t += 0.1f;
    int n = ex_spearcon_update(&p, 0.1f, det, EX_SPEARCON_MAX);
    if (n){ total += n; fired_x = det[0][0]; }
  }
  assert(total == 1 && "detonates exactly once");
  assert(fabsf(fired_x - 5.0f) < 1e-4f && "detonation carries the plant position");
  assert(ex_spearcon_active_count(&p) == 0 && "slot freed after blast");
  printf("plant -> countdown -> single detonation at x=%.1f OK\n", fired_x);

  // capacity: fill the pool, next plant fails
  ex_spearcon_reset(&p);
  for (int k=0;k<EX_SPEARCON_MAX;k++){ float q[3]={(float)k,0,0}; assert(ex_spearcon_plant(&p,q,1.0f)>=0); }
  { float q[3]={99,0,0}; assert(ex_spearcon_plant(&p,q,1.0f) == -1 && "pool full rejects"); }
  assert(ex_spearcon_active_count(&p) == EX_SPEARCON_MAX);
  printf("capacity cap OK (%d slots)\n", EX_SPEARCON_MAX);

  // progress ramps 0..~1 while a charge burns
  ex_spearcon_reset(&p);
  int j = ex_spearcon_plant(&p, a, 1.0f);
  float last = -1.0f;
  for (int k=0;k<9;k++){ ex_spearcon_update(&p, 0.1f, det, EX_SPEARCON_MAX);
    float pr = ex_spearcon_progress(&p.s[j]);
    if (!p.s[j].active) break;
    assert(pr >= last-1e-6f && pr >= 0.0f && pr <= 1.0f); last = pr; }
  printf("progress ramps monotonically OK\n");

  printf("\nSPEARCONS OK (sticky charges, ticking detonation)\n");
  return 0;
}
