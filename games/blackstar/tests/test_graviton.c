/* test_graviton.c -- Unicron's graviton well pull field.
   Verifies zero outside the rim and at the exact center, correct inward
   direction, linear falloff (stronger toward the center), and Y being ignored. */
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include "graviton.h"

static int approx(float a, float b){ return fabsf(a-b) < 1e-3f; }

int main(void)
{
  float R = 100.0f, S = 14.0f;
  float c[3] = { 0, 0, 0 };
  float dir[3];

  // beyond the rim -> no pull, dir zeroed
  { float p[3]={150,0,0}; float m=ex_graviton_pull(c,p,R,S,dir);
    assert(m==0.0f && dir[0]==0.0f && dir[2]==0.0f); }
  // exactly at the rim -> no pull
  { float p[3]={100,0,0}; assert(ex_graviton_pull(c,p,R,S,dir)==0.0f); }
  // at the exact center -> no pull (avoid divide-by-zero)
  { float p[3]={0,0,0}; assert(ex_graviton_pull(c,p,R,S,NULL)==0.0f); }
  printf("zero outside rim / at center OK\n");

  // inside: pulls back toward center (negative x side pulls +x)
  { float p[3]={-50,0,0}; float m=ex_graviton_pull(c,p,R,S,dir);
    assert(m>0.0f && approx(dir[0],1.0f) && approx(dir[2],0.0f)); }
  { float p[3]={0,0,50}; float m=ex_graviton_pull(c,p,R,S,dir);
    assert(m>0.0f && approx(dir[2],-1.0f)); }
  printf("inward direction OK\n");

  // linear falloff: half-radius -> half strength; stronger nearer the center
  { float half[3]={50,0,0}, near[3]={10,0,0};
    float mh=ex_graviton_pull(c,half,R,S,dir);
    float mn=ex_graviton_pull(c,near,R,S,dir);
    assert(approx(mh, S*0.5f));
    assert(mn > mh && "pull grows toward the center"); }
  printf("linear falloff, stronger near center OK\n");

  // Y is ignored (xz-plane field)
  { float p[3]={30,999,0}; float m=ex_graviton_pull(c,p,R,S,dir);
    assert(approx(m, S*(1.0f-30.0f/100.0f))); }
  printf("xz-plane (Y ignored) OK\n");

  printf("\nGRAVITON WELL OK (inward pull, linear falloff)\n");
  return 0;
}
