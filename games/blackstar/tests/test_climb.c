/* test_climb.c -- verifies ex_step_wall_ahead: the pure geometry behind the
   wall-climb. A climbable wall must be ahead, tall enough, and not already
   crested. No GL, no game state. */
#include <stdio.h>
#include <assert.h>
#include "terrain_step.h"

int main(void)
{
  // a tall wall centered at x=3, spanning y 0..6, z -2..2
  ex_step_box_t wall = { {3.0f, 3.0f, 0.0f}, {0.5f, 3.0f, 2.0f} };
  ex_step_box_t boxes[1] = { wall };
  float pos[3] = { 2.0f, 1.0f, 0.0f };   // standing just in front, feet at y=1
  float pr = 0.4f, reach = 0.5f, min_rise = 1.0f, top = 0.0f;

  // facing +x (into the wall) -> climbable, top = 6
  assert(ex_step_wall_ahead(pos, 1.0f, 0.0f, pr, boxes, 1, reach, min_rise, &top) == 1);
  assert(top > 5.99f && top < 6.01f);
  printf("into wall: climb, top=%.2f OK\n", top);

  // facing -x (away) -> nothing
  assert(ex_step_wall_ahead(pos, -1.0f, 0.0f, pr, boxes, 1, reach, min_rise, &top) == 0);
  printf("away from wall: no climb OK\n");

  // a short step (top y=1.2) should NOT count as a climbable wall
  ex_step_box_t step = { {3.0f, 0.6f, 0.0f}, {0.5f, 0.6f, 2.0f} }; // top=1.2
  ex_step_box_t sb[1] = { step };
  assert(ex_step_wall_ahead(pos, 1.0f, 0.0f, pr, sb, 1, reach, min_rise, &top) == 0);
  printf("short step: no climb (that's a step-up) OK\n");

  // already above the top -> no climb
  float high[3] = { 2.0f, 6.5f, 0.0f };
  assert(ex_step_wall_ahead(high, 1.0f, 0.0f, pr, boxes, 1, reach, min_rise, &top) == 0);
  printf("above ledge: no climb OK\n");

  // too far to reach the face -> no climb
  float far[3] = { 0.5f, 1.0f, 0.0f };
  assert(ex_step_wall_ahead(far, 1.0f, 0.0f, pr, boxes, 1, reach, min_rise, &top) == 0);
  printf("out of reach: no climb OK\n");

  // --- SWEPT LANDING: a fast fall that crosses a step top in one frame must LAND
  //     on top (was above last frame), not get shoved out the side. ---
  ex_step_box_t landbox = { {3.0f, 0.5f, 0.0f}, {1.0f, 0.5f, 1.0f} }; // top y=1.0
  {
    float p[3] = { 3.0f, 0.4f, 0.0f };   // ended below the top this frame...
    float v[3] = { 0.0f, -15.0f, 0.0f }; // ...descending
    float prev_y = 1.3f;                 // ...but was ABOVE the top last frame
    int landed = ex_step_resolve_box(p, v, 0.4f, &landbox, prev_y);
    assert(landed == 1 && "crossed the top from above -> land");
    assert(p[1] > 0.99f && p[1] < 1.01f && v[1] == 0.0f);
    printf("swept fall onto step: landed at y=%.2f OK\n", p[1]);
  }
  {
    // approaching the same box from the SIDE (was below the top) -> push out, no land
    float p[3] = { 2.2f, 0.4f, 0.0f };
    float v[3] = { 5.0f, -1.0f, 0.0f };
    float prev_y = 0.4f;                  // never above the top
    int landed = ex_step_resolve_box(p, v, 0.4f, &landbox, prev_y);
    assert(landed == 0 && "hitting the side from below must not snap to top");
    printf("side approach: pushed out (no false land) OK\n");
  }

  printf("\nCLIMB PROBE OK\n");
  return 0;
}
