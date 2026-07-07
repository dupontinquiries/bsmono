#include "arena.h"
#include <string.h>

// cheap deterministic hash -> [0,1)
static float hash01(uint32_t x, uint32_t salt)
{
  uint32_t h = x * 0x9E3779B1u ^ (salt + 0x85EBCA6Bu);
  h ^= h >> 15; h *= 0x2C1B3C6Du; h ^= h >> 12; h *= 0x297A2D39u; h ^= h >> 15;
  return (float)(h & 0xFFFFFF) / (float)0x1000000;
}

void ex_arena_generate(ex_arena_t *arena, ex_chunk_world_t *chunks,
                       float min_x, float min_z, float max_x, float max_z,
                       float ground_y, int grid, uint32_t seed)
{
  memset(arena, 0, sizeof(*arena));
  if (grid < 1) grid = 1;

  float span_x = (max_x - min_x);
  float span_z = (max_z - min_z);

  int idx = 0;
  for (int gz = 0; gz < grid; gz++) {
    for (int gx = 0; gx < grid; gx++) {
      if (arena->count >= EX_ARENA_MAX_FEATURES)
        return;

      uint32_t cell = (uint32_t)(gz * grid + gx);
      // jitter the site within its cell so layouts don't look like a grid
      float jx = (hash01(cell, seed) - 0.5f) * (span_x / grid) * 0.7f;
      float jz = (hash01(cell, seed ^ 0x55) - 0.5f) * (span_z / grid) * 0.7f;
      float x = min_x + (gx + 0.5f) / grid * span_x + jx;
      float z = min_z + (gz + 0.5f) / grid * span_z + jz;

      // synthesise the environment the FSM gates on
      ex_site_t s;
      memset(&s, 0, sizeof(s));
      s.x = x; s.z = z; s.ground_y = ground_y;
      s.seed = seed ^ (cell * 2654435761u);
      // longer sightlines toward the middle of the arena, plus noise
      float cx = (min_x + max_x) * 0.5f, cz = (min_z + max_z) * 0.5f;
      float d_center = ((x-cx)*(x-cx) + (z-cz)*(z-cz));
      float span2 = span_x*span_x + span_z*span_z;
      s.sightline = 10.0f + (1.0f - d_center / (span2 * 0.25f + 1.0f)) * 30.0f
                    + hash01(cell, seed ^ 0xA1) * 12.0f;
      if (s.sightline < 0.0f) s.sightline = 0.0f;
      s.danger_yaw   = hash01(cell, seed ^ 0xB2) * 6.2831853f;
      s.high_traffic = hash01(cell, seed ^ 0xC3) > 0.6f;
      s.is_high_point= hash01(cell, seed ^ 0xD4) > 0.82f; // rare
      s.slope        = hash01(cell, seed ^ 0xE5) * 0.5f;
      s.wet          = hash01(cell, seed ^ 0xF6) > 0.85f;
      s.indoor       = 0;

      ex_feature_t f = ex_fsm_grow_into_world(chunks, &s, idx);
      idx++;
      if (f == EX_FEATURE_NONE)
        continue;

      // record placement for the game layer's visuals
      ex_fsm_result_t r = ex_fsm_grow(&s);
      ex_placed_feature_t *pf = &arena->features[arena->count++];
      pf->feature = f;
      pf->position[0] = x; pf->position[1] = ground_y; pf->position[2] = z;
      pf->half_extents[0] = r.half_extents[0];
      pf->half_extents[1] = r.half_extents[1];
      pf->half_extents[2] = r.half_extents[2];
      pf->destructible = r.destructible;
    }
  }
}
