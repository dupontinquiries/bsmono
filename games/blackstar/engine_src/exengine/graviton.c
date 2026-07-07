#include "graviton.h"
#include <math.h>

float ex_graviton_pull(const float center[3], const float p[3],
                       float radius, float strength, float out_dir[3])
{
  if (out_dir) { out_dir[0] = out_dir[1] = out_dir[2] = 0.0f; }
  if (radius <= 0.0f) return 0.0f;
  float dx = center[0] - p[0];
  float dz = center[2] - p[2];
  float d  = sqrtf(dx*dx + dz*dz);
  if (d < 1e-3f || d >= radius) return 0.0f;   /* at center or beyond the rim: no pull */
  if (out_dir) { out_dir[0] = dx / d; out_dir[2] = dz / d; }
  float mag = strength * (1.0f - d / radius);   /* strong at center -> 0 at the rim */
  return mag < 0.0f ? 0.0f : mag;
}
