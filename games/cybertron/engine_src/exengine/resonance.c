#include "resonance.h"
#include <math.h>

float ex_resonance_strength(const float coords[][3], int n, const float p[3], float radius)
{
  if (!coords || n <= 0 || radius <= 0.0f) return 0.0f;
  float best = 0.0f;
  for (int i = 0; i < n; i++) {
    float dx = p[0] - coords[i][0];
    float dz = p[2] - coords[i][2];
    float d  = sqrtf(dx*dx + dz*dz);
    if (d >= radius) continue;
    float s = 1.0f - d / radius;      /* 1 at center -> 0 at the rim */
    if (s > best) best = s;
  }
  if (best < 0.0f) best = 0.0f;
  if (best > 1.0f) best = 1.0f;
  return best;
}
