/* test_corrupt.c -- Jhiaxus's Corrupted generational drift.
   Verifies gen 0 is baseline, every axis rises monotonically with generation,
   and all axes are capped so a long fight escalates but never runs away. */
#include <stdio.h>
#include <assert.h>
#include "corrupt.h"

int main(void)
{
  // baseline: generation 0 is a plain Corrupted
  ex_corrupt_drift_t d0 = ex_corrupt_drift(0);
  assert(d0.speed_mul == 1.0f && d0.scale == 1.0f);
  assert(d0.aggression == 0.0f && d0.darken == 0.0f);
  printf("gen 0 baseline OK\n");

  // negative clamps to baseline (defensive)
  assert(ex_corrupt_drift(-5).speed_mul == 1.0f);

  // monotonic non-decreasing across generations
  ex_corrupt_drift_t prev = d0;
  for (int g=1; g<=40; g++){
    ex_corrupt_drift_t d = ex_corrupt_drift(g);
    assert(d.speed_mul  >= prev.speed_mul  - 1e-6f);
    assert(d.scale      >= prev.scale      - 1e-6f);
    assert(d.aggression >= prev.aggression - 1e-6f);
    assert(d.darken     >= prev.darken     - 1e-6f);
    prev = d;
  }
  printf("monotonic escalation OK\n");

  // a later generation is strictly more dangerous than gen 0
  ex_corrupt_drift_t d5 = ex_corrupt_drift(5);
  assert(d5.speed_mul > 1.0f && d5.scale > 1.0f && d5.aggression > 0.0f);
  printf("gen 5 evolved: speed=%.2f scale=%.2f aggr=%.2f dark=%.2f OK\n",
         d5.speed_mul, d5.scale, d5.aggression, d5.darken);

  // hard ceilings hold no matter how long the fight runs
  ex_corrupt_drift_t big = ex_corrupt_drift(100000);
  assert(big.speed_mul  <= 1.55f + 1e-4f);
  assert(big.scale      <= 1.40f + 1e-4f);
  assert(big.aggression <= 1.00f + 1e-4f);
  assert(big.darken     <= 0.70f + 1e-4f);
  printf("capped at speed<=1.55 scale<=1.40 aggr<=1.0 dark<=0.7 OK\n");

  printf("\nCORRUPTED DRIFT OK (bounded, monotonic escalation)\n");
  return 0;
}
