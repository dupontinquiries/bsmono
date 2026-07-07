#include "feel.h"

/* Defaults chosen to match the game's current hand-tuned feel:
 *  spring_hz ~11 and damp_ratio ~0.68 reproduce the old k=120 / c=15 spring. */
ex_feel_t g_feel = {
  .mass       = 1.0f,
  .spring_hz  = 11.0f,
  .damp_ratio = 0.68f,
  .g_show     = 1.0f,
  .cam_lag    = 0.16f,
  .grip       = 0.85f,
  .drag       = 1.0f,
  .turn       = 2.8f,
};
