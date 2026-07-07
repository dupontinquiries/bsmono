/* test_containment.c -- the failing Pluto containment boundary.
   Verifies the radius shrinks from r0 to r1 over the close time (clamped, monotonic
   non-increasing) and that breach is zero inside and grows outside. */
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include "containment.h"

static int approx(float a, float b){ return fabsf(a-b) < 1e-3f; }

int main(void)
{
  float R0=140.0f, R1=52.0f, CT=60.0f;

  // opens at r0, closes to r1, half-way is the midpoint
  assert(approx(ex_containment_radius(0.0f, R0,R1,CT), R0));
  assert(approx(ex_containment_radius(CT,  R0,R1,CT), R1));
  assert(approx(ex_containment_radius(CT*0.5f, R0,R1,CT), (R0+R1)*0.5f));
  // clamped before start / after close
  assert(approx(ex_containment_radius(-5.0f, R0,R1,CT), R0));
  assert(approx(ex_containment_radius(999.0f, R0,R1,CT), R1));
  printf("radius shrinks r0->r1, clamped OK\n");

  // monotonic non-increasing
  float last = R0 + 1.0f;
  for (int i=0;i<=120;i++){ float r=ex_containment_radius((float)i*0.5f, R0,R1,CT);
    assert(r <= last + 1e-4f); last=r; }
  printf("monotonic close OK\n");

  // breach: zero inside/at edge, positive (= overshoot) outside
  assert(ex_containment_breach(40.0f, 52.0f) == 0.0f);
  assert(ex_containment_breach(52.0f, 52.0f) == 0.0f);
  assert(approx(ex_containment_breach(70.0f, 52.0f), 18.0f));
  printf("breach inside=0 / outside grows OK\n");

  // degenerate close_time -> immediately closed
  assert(approx(ex_containment_radius(0.0f, R0,R1,0.0f), R1));
  printf("degenerate close_time OK\n");

  printf("\nCONTAINMENT OK (contracting boundary, breach beyond it)\n");
  return 0;
}
