#include "terrain_step.h"
#include <math.h>

int ex_step_resolve_box(float pos[3], float vel[3], float pr, const ex_step_box_t *b, float prev_y)
{
  float minx = b->center[0]-b->half[0]-pr, maxx = b->center[0]+b->half[0]+pr;
  float minz = b->center[2]-b->half[2]-pr, maxz = b->center[2]+b->half[2]+pr;
  float top  = b->center[1]+b->half[1];
  if (pos[0]<=minx || pos[0]>=maxx || pos[2]<=minz || pos[2]>=maxz)
    return 0;
  // standing on / stepping onto the top surface
  if (pos[1] >= top - EX_STEP_UP_TOL) {
    if (pos[1] < top + EX_STEP_SNAP_TOL) { pos[1] = top; if (vel[1] < 0) vel[1] = 0; return 1; }
    return 0;
  }
  // SWEPT LANDING: if the feet were at/above the top LAST frame and fell below it
  // THIS frame, the character crossed the top -> land on it, rather than being
  // pushed out the side (which made fast jumps onto steps fail). The upward snap
  // is at most one frame's fall, so it stays physical -- no teleport from inside.
  if (vel[1] < 0.0f && prev_y >= top - EX_STEP_UP_TOL && pos[1] < top) {
    pos[1] = top; vel[1] = 0.0f; return 1;
  }
  // otherwise push out along the least-penetration horizontal axis
  float pxl = pos[0]-minx, pxr = maxx-pos[0];
  float pzl = pos[2]-minz, pzr = maxz-pos[2];
  float m = pxl; int ax = 0;
  if (pxr<m){m=pxr;ax=1;} if (pzl<m){m=pzl;ax=2;} if (pzr<m){m=pzr;ax=3;}
  if      (ax==0) pos[0]=minx;
  else if (ax==1) pos[0]=maxx;
  else if (ax==2) pos[2]=minz;
  else            pos[2]=maxz;
  return 0;
}

int ex_step_resolve(float pos[3], float vel[3], float pr, const ex_step_box_t *boxes, int n, float prev_y)
{
  int grounded = 0;
  for (int i = 0; i < n; i++)
    if (ex_step_resolve_box(pos, vel, pr, &boxes[i], prev_y)) grounded = 1;
  return grounded;
}

int ex_step_wall_ahead(const float pos[3], float dx, float dz, float pr,
                       const ex_step_box_t *boxes, int n,
                       float reach, float min_rise, float *out_top)
{
  float dl = dx*dx + dz*dz;
  if (dl < 1e-8f) return 0;
  dl = 1.0f / (float)sqrt(dl); dx *= dl; dz *= dl;
  // sample point just past the collider radius, in the facing direction
  float px = pos[0] + dx * (pr + reach);
  float pz = pos[2] + dz * (pr + reach);
  float best_top = -1e30f; int found = 0;
  for (int i = 0; i < n; i++) {
    const ex_step_box_t *b = &boxes[i];
    float minx = b->center[0]-b->half[0], maxx = b->center[0]+b->half[0];
    float minz = b->center[2]-b->half[2], maxz = b->center[2]+b->half[2];
    float top  = b->center[1]+b->half[1], bot = b->center[1]-b->half[1];
    if (px < minx || px > maxx || pz < minz || pz > maxz) continue; // not facing this box
    if (top < pos[1] + min_rise) continue;   // too short -> a step, handled by resolve, not a wall
    if (pos[1] > top - 0.10f)     continue;   // already at/above the top
    if (pos[1] < bot - 0.75f)     continue;   // wall face starts well above reach
    if (top > best_top) { best_top = top; found = 1; }
  }
  if (found && out_top) *out_top = best_top;
  return found;
}
