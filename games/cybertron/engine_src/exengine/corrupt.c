#include "corrupt.h"

static float capf(float v, float lo, float hi){ return v < lo ? lo : (v > hi ? hi : v); }

ex_corrupt_drift_t ex_corrupt_drift(int generation)
{
  int g = generation; if (g < 0) g = 0;
  ex_corrupt_drift_t d;
  /* each generation adds a little, with a hard ceiling so a long fight escalates
     into something dangerous but never absurd. */
  d.speed_mul  = 1.0f + capf(g * 0.05f, 0.0f, 0.55f);   /* up to +55% */
  d.scale      = 1.0f + capf(g * 0.03f, 0.0f, 0.40f);   /* up to +40% */
  d.aggression =        capf(g * 0.08f, 0.0f, 1.00f);   /* 0..1 */
  d.darken     =        capf(g * 0.06f, 0.0f, 0.70f);   /* 0..0.7 */
  return d;
}
