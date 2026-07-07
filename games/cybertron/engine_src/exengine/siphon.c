#include "siphon.h"

float ex_siphon_rate(float dist, float radius, float max_rate)
{
  if (radius <= 0.0f || dist >= radius) return 0.0f;
  if (dist < 0.0f) dist = 0.0f;
  float f = 1.0f - dist / radius;   /* 1 at the center -> 0 at the rim */
  float r = max_rate * f;
  return r < 0.0f ? 0.0f : r;
}
