/* test_siphon.c -- the devourer's energy-drain curve.
   Verifies max drain at point-blank, linear falloff to the rim, zero at/beyond
   the rim, and negative distances clamp to the center. */
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include "siphon.h"

static int approx(float a, float b){ return fabsf(a-b) < 1e-3f; }

int main(void)
{
  float R = 40.0f, M = 14.0f;

  // point-blank -> full rate
  assert(approx(ex_siphon_rate(0.0f, R, M), M));
  // negative distance clamps to center (full rate)
  assert(approx(ex_siphon_rate(-5.0f, R, M), M));
  printf("point-blank = max drain OK\n");

  // half radius -> half rate; quarter -> three-quarters
  assert(approx(ex_siphon_rate(20.0f, R, M), M*0.5f));
  assert(approx(ex_siphon_rate(10.0f, R, M), M*0.75f));
  printf("linear falloff OK\n");

  // at / beyond the rim -> zero
  assert(ex_siphon_rate(40.0f, R, M) == 0.0f);
  assert(ex_siphon_rate(60.0f, R, M) == 0.0f);
  printf("zero at/beyond rim OK\n");

  // monotonic non-increasing with distance
  float last = M + 1.0f;
  for (int i=0;i<=40;i++){ float r=ex_siphon_rate((float)i, R, M);
    assert(r <= last + 1e-6f); last = r; }
  printf("monotonic with distance OK\n");

  printf("\nSIPHON OK (point-blank drain, linear falloff)\n");
  return 0;
}
