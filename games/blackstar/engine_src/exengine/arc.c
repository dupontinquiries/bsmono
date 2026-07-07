#include "arc.h"

int ex_arc_chain(const float origin[3], const float (*epos)[3], const int *alive,
                 int n, float range, int max_jumps, int *out_chain)
{
  if (!epos || !out_chain || n <= 0 || max_jumps <= 0) return 0;
  float from[3] = { origin[0], origin[1], origin[2] };
  float r2 = range * range;
  int count = 0;
  for (int j = 0; j < max_jumps; j++) {
    int best = -1; float bd = r2;
    for (int i = 0; i < n; i++) {
      if (alive && !alive[i]) continue;
      int seen = 0;                              /* already struck earlier in the chain? */
      for (int k = 0; k < count; k++) if (out_chain[k] == i) { seen = 1; break; }
      if (seen) continue;
      float dx = epos[i][0] - from[0], dz = epos[i][2] - from[2];
      float d2 = dx*dx + dz*dz;
      if (d2 < bd) { bd = d2; best = i; }
    }
    if (best < 0) break;                         /* nothing left in reach */
    out_chain[count++] = best;
    from[0] = epos[best][0]; from[1] = epos[best][1]; from[2] = epos[best][2];
  }
  return count;
}
