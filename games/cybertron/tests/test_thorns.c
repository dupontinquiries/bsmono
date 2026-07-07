/* test_thorns.c -- the vine thorn field's geometry.
   Verifies the patch spreads 0->max over the grow time then holds, and that
   intensity is fiercest at the centre, tapers to the live rim, and is 0 outside. */
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include "thorns.h"
static int approx(float a,float b){ return fabsf(a-b)<1e-3f; }
int main(void){
  float M=30.0f, G=1.2f;
  // spread over grow time
  assert(ex_thorns_radius(0.0f,M,G)==0.0f);
  assert(approx(ex_thorns_radius(0.6f,M,G), M*0.5f));   // half-grown at half the grow time
  assert(approx(ex_thorns_radius(1.2f,M,G), M));        // full at grow time
  assert(approx(ex_thorns_radius(5.0f,M,G), M));        // holds afterwards
  printf("spread then hold OK\n");
  // intensity profile
  float r = ex_thorns_radius(2.0f,M,G);                 // = M (full)
  assert(approx(ex_thorns_intensity(0.0f,r),1.0f));     // fiercest at centre
  assert(approx(ex_thorns_intensity(r*0.5f,r),0.5f));   // linear taper
  assert(ex_thorns_intensity(r,r)==0.0f);               // nothing at the rim
  assert(ex_thorns_intensity(r+5.0f,r)==0.0f);          // nothing outside
  // a point at fixed distance is OUTSIDE early (small radius) and INSIDE later
  assert(ex_thorns_intensity(20.0f, ex_thorns_radius(0.2f,M,G))==0.0f);  // not yet spread this far
  assert(ex_thorns_intensity(20.0f, ex_thorns_radius(1.2f,M,G)) >0.0f);  // reached once full
  printf("intensity profile + spread-over-time OK\n");
  printf("\nTHORNS OK (spreading field, centre-weighted)\n");
  return 0;
}
