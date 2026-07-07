/* test_cyclone.c -- the air whirlwind's wind field.
   Verifies: no effect outside the radius; caught inside; the swirl is tangential
   (perpendicular to the radial) with a net inward drag; strength grows toward the
   eye; the eye itself is caught but directionless. */
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include "cyclone.h"

int main(void){
  float R=40.0f, S=30.0f, P=8.0f, v[3];

  // outside -> not caught, zero velocity
  assert(ex_cyclone_vel(50,0,R,S,P,v)==0 && v[0]==0 && v[2]==0);
  assert(ex_cyclone_vel(40,0,R,S,P,v)==0);       // exactly at rim = outside
  printf("outside rim -> no effect OK\n");

  // a point on +x inside: tangential should be +z (CCW), plus inward (-x)
  int in = ex_cyclone_vel(20,0,R,S,P,v);
  assert(in==1);
  assert(v[2] > 0.0f && "swirls counter-clockwise");
  assert(v[0] < 0.0f && "dragged inward (toward the eye)");
  // net motion is mostly perpendicular: tangential component dominates the pull
  assert(fabsf(v[2]) > fabsf(v[0]) && "swirl dominates the gentle inward drag");
  printf("swirl tangential + inward drag OK\n");

  // radial (outward) component is inward: v . outward_radial < 0
  { float dx=0,dz=25; ex_cyclone_vel(dx,dz,R,S,P,v);
    float rlen=sqrtf(dx*dx+dz*dz); float dot=(v[0]*dx+v[2]*dz)/rlen;
    assert(dot < 0.0f && "net inward on every bearing"); }
  printf("inward on arbitrary bearing OK\n");

  // stronger near the eye than near the rim
  float near[3], far[3];
  ex_cyclone_vel(6,0,R,S,P,near); ex_cyclone_vel(36,0,R,S,P,far);
  float mn=sqrtf(near[0]*near[0]+near[2]*near[2]), mf=sqrtf(far[0]*far[0]+far[2]*far[2]);
  assert(mn > mf && "wind is stronger near the eye");
  printf("stronger near the eye OK\n");

  // the exact eye: caught but no direction
  assert(ex_cyclone_vel(0,0,R,S,P,v)==1 && v[0]==0 && v[2]==0);
  printf("calm eye OK\n");

  printf("\nCYCLONE OK (tangential swirl + inward drag)\n");
  return 0;
}
