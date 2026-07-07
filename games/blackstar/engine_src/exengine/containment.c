#include "containment.h"

float ex_containment_radius(float elapsed, float r0, float r1, float close_time)
{
  if (close_time <= 0.0f) return r1;
  float f = elapsed / close_time;
  if (f < 0.0f) f = 0.0f; else if (f > 1.0f) f = 1.0f;
  return r0 + (r1 - r0) * f;   /* r0 at the start -> r1 once fully closed */
}

float ex_containment_breach(float dist, float radius)
{
  float b = dist - radius;
  return b > 0.0f ? b : 0.0f;
}
