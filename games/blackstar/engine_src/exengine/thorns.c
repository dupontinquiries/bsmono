#include "thorns.h"

float ex_thorns_radius(float age, float max_r, float grow_time)
{
  if (age <= 0.0f) return 0.0f;
  if (grow_time <= 0.0f) return max_r;
  float r = (age >= grow_time) ? max_r : max_r * (age / grow_time);
  return (r < 0.0f) ? 0.0f : (r > max_r ? max_r : r);
}

float ex_thorns_intensity(float dist, float radius)
{
  if (radius <= 0.0f || dist >= radius) return 0.0f;
  if (dist < 0.0f) dist = 0.0f;
  float i = 1.0f - dist / radius;   /* fiercest at the centre */
  return (i < 0.0f) ? 0.0f : i;
}
