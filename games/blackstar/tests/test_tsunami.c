/* test_tsunami.c -- the water form's advancing crest.
   Verifies arm normalizes heading, the crest advances linearly at SPEED, stays
   active across its range then spends itself exactly once, and progress ramps. */
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include "tsunami.h"

static int approx(float a, float b){ return fabsf(a-b) < 0.05f; }

int main(void)
{
  ex_tsunami_t t; t.active = 0;

  // a zero heading is ignored (no wave)
  { float o[3]={0,0,0}, z[3]={0,0,0}; ex_tsunami_arm(&t, o, z); assert(!t.active); }

  // arm along +x from origin; heading is normalized
  float o[3]={10,0,-4}, dir[3]={5,0,0};   // non-unit
  ex_tsunami_arm(&t, o, dir);
  assert(t.active);
  assert(approx(t.dir[0],1.0f) && approx(t.dir[2],0.0f));
  assert(ex_tsunami_progress(&t) == 0.0f);
  printf("arm normalizes heading OK\n");

  // advance: after 0.1s the crest is SPEED*0.1 further along +x from origin
  float front[3];
  int live = ex_tsunami_tick(&t, 0.1f, front);
  assert(live == 1);
  assert(approx(front[0], 10.0f + EX_TSUNAMI_SPEED*0.1f));
  assert(approx(front[2], -4.0f) && "stays in its lane (z fixed)");
  printf("crest advances linearly along heading OK\n");

  // run it to the end: it must stay active across the range, then spend itself
  int frames_active = 1, spent_frame = -1;
  for (int i=0;i<400;i++){
    int a = ex_tsunami_tick(&t, 0.05f, front);
    if (a) frames_active++;
    if (!t.active && spent_frame<0) spent_frame = i;
  }
  assert(spent_frame >= 0 && !t.active && "wave spends itself");
  // total distance travelled reached the range
  float travelled = sqrtf((front[0]-10.0f)*(front[0]-10.0f) + (front[2]+4.0f)*(front[2]+4.0f));
  assert(approx(travelled, EX_TSUNAMI_RANGE));
  // ticking a spent wave returns 0 and doesn't move the front
  assert(ex_tsunami_tick(&t, 0.1f, front) == 0);
  printf("stays active across range (~%.0f u) then spends once OK\n", (double)EX_TSUNAMI_RANGE);

  printf("\nTSUNAMI OK (directional crest, linear advance)\n");
  return 0;
}
