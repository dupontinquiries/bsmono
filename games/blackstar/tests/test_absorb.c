/* test_absorb.c -- the Corrupted absorbing metal to grow.
   Verifies grow starts/holds at >=1, accumulates with diminishing returns,
   never exceeds the cap, and that size/damage scale monotonically from it. */
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include "absorb.h"

int main(void)
{
  // baseline: grow 1.0 -> no scaling
  ex_absorb_stats_t b = ex_absorb_stats(1.0f);
  assert(b.scale == 1.0f && b.dmg_mul == 1.0f);
  // defensive: sub-1 grow clamps to baseline
  assert(ex_absorb_stats(0.2f).scale == 1.0f);
  printf("baseline (grow<=1) -> no scaling OK\n");

  // accumulation rises and saturates, never past the cap
  float g = 1.0f, last = 1.0f;
  for (int i=0;i<200;i++){
    g = ex_absorb_accumulate(g, 0.10f);
    assert(g >= last - 1e-6f && "grow is monotonic");
    assert(g <= EX_ABSORB_GROW_MAX + 1e-4f && "never exceeds cap");
    last = g;
  }
  assert(fabsf(g - EX_ABSORB_GROW_MAX) < 1e-2f && "saturates at the cap");
  printf("accumulate saturates at cap %.2f OK\n", (double)EX_ABSORB_GROW_MAX);

  // diminishing returns: first bite grows more than a later one
  float e1 = ex_absorb_accumulate(1.0f, 0.10f) - 1.0f;
  float mid = ex_absorb_accumulate(1.0f, 0.10f);
  for (int i=0;i<8;i++) mid = ex_absorb_accumulate(mid, 0.10f);
  float e2 = ex_absorb_accumulate(mid, 0.10f) - mid;
  assert(e1 > e2 && "later absorption yields less");
  printf("diminishing returns OK (early=%.3f late=%.3f)\n", e1, e2);

  // stats climb monotonically with grow, capping at the ceiling
  ex_absorb_stats_t cap = ex_absorb_stats(EX_ABSORB_GROW_MAX);
  ex_absorb_stats_t mid_s = ex_absorb_stats(1.6f);
  assert(mid_s.scale > 1.0f && mid_s.scale < cap.scale);
  assert(mid_s.dmg_mul > 1.0f && mid_s.dmg_mul < cap.dmg_mul);
  assert(ex_absorb_stats(999.0f).scale == cap.scale && "clamped above cap");
  printf("scale/dmg climb to cap (scale<=%.2f dmg<=%.2f) OK\n", cap.scale, cap.dmg_mul);

  printf("\nCORRUPTED ABSORB OK (bounded grow, monotonic scaling)\n");
  return 0;
}
