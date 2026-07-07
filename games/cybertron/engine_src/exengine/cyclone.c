#include "cyclone.h"
#include <math.h>

int ex_cyclone_vel(float dx, float dz, float radius, float swirl, float pull,
                   float out_vel[3])
{
  out_vel[0] = out_vel[1] = out_vel[2] = 0.0f;
  if (radius <= 0.0f) return 0;
  float d = sqrtf(dx*dx + dz*dz);
  if (d >= radius) return 0;                 /* outside the vortex */
  if (d < 1e-3f) return 1;                    /* the calm eye: caught, but no direction */
  float f  = 1.0f - d / radius;               /* proximity 0..1 (strong near the eye) */
  float rx = dx / d, rz = dz / d;             /* outward radial unit */
  float tx = -rz, tz = rx;                    /* tangential unit (counter-clockwise) */
  out_vel[0] = tx*swirl*f - rx*pull*f;        /* swirl + gentle inward drag */
  out_vel[2] = tz*swirl*f - rz*pull*f;
  return 1;
}
