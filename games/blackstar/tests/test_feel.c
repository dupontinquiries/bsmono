#include "exengine/feel.h"
#include <stdio.h>
#include <assert.h>
#include <math.h>
int main(void){
  g_feel.spring_hz = 11.0f; float w2a=ex_feel_w2(), zwa=ex_feel_2zw();
  g_feel.spring_hz = 16.0f; float w2b=ex_feel_w2(), zwb=ex_feel_2zw();
  assert(w2b>w2a && zwb>zwa);   // damping tracks stiffness (stays stable)
  g_feel.mass=1.0f; float t1=ex_feel_turn_rate(20.0f,0.85f), s1=ex_feel_susp_scale();
  g_feel.mass=2.0f; float t2=ex_feel_turn_rate(20.0f,0.85f), s2=ex_feel_susp_scale();
  assert(t2<t1 && s2>s1);
  g_feel.spring_hz=11.0f; g_feel.damp_ratio=1.0f; g_feel.mass=1.0f;
  float x=1.0f,v=0.0f, peak=0;
  for(int i=0;i<2000;i++){ ex_feel_spring(&x,&v,0.0f,1.0f/240.0f); if(x<peak)peak=x; }
  assert(peak > -0.05f && fabsf(x) < 0.01f);
  printf("feel: derived spring/turn/suspension relations hold\n");
  printf("ALL ASSERTIONS PASSED\n");
  return 0;
}
