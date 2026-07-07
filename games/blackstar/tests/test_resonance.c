/* test_resonance.c -- Blackstar's resonance field membership/strength.
   Verifies: full strength at a coordinator, linear falloff to the rim, zero
   outside, max across multiple coordinators, and zero with no coordinators. */
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include "resonance.h"

static int approx(float a, float b){ return fabsf(a-b) < 1e-3f; }

int main(void)
{
  float R = 34.0f;
  float coords[2][3] = { {0,0,0}, {100,0,0} };

  // no coordinators -> 0
  { float p[3]={0,0,0}; assert(ex_resonance_strength(coords, 0, p, R) == 0.0f); }
  printf("no coordinators -> 0 OK\n");

  // at the coordinator -> full strength 1
  { float p[3]={0,0,0}; assert(approx(ex_resonance_strength(coords, 2, p, R), 1.0f)); }
  // halfway to the rim -> ~0.5
  { float p[3]={17.0f,0,0}; assert(approx(ex_resonance_strength(coords, 2, p, R), 0.5f)); }
  // at/just past the rim -> 0
  { float p[3]={34.0f,0,0}; assert(ex_resonance_strength(coords, 2, p, R) == 0.0f);
    float q[3]={40.0f,0,0}; assert(ex_resonance_strength(coords, 2, q, R) == 0.0f); }
  printf("center=1, mid=0.5, rim=0 OK\n");

  // between two far coordinators, outside both -> 0
  { float p[3]={50.0f,0,0}; assert(ex_resonance_strength(coords, 2, p, R) == 0.0f); }
  // near the SECOND coordinator -> strength from it (max across fields)
  { float p[3]={100.0f,0,0}; assert(approx(ex_resonance_strength(coords, 2, p, R), 1.0f));
    float q[3]={90.0f,0,0};  assert(approx(ex_resonance_strength(coords, 2, q, R), 1.0f - 10.0f/34.0f)); }
  printf("max across multiple fields OK\n");

  // only Z separation counts too (xz-plane), Y ignored
  { float p[3]={0,999,3.4f}; assert(approx(ex_resonance_strength(coords, 1, p, R), 1.0f - 3.4f/34.0f)); }
  printf("xz-plane (Y ignored) OK\n");

  printf("\nRESONANCE FIELD OK (coordination membership + falloff)\n");
  return 0;
}
