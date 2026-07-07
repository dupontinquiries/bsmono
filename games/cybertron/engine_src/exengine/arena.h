/* arena
  Procedural arena population. Scatters candidate sites across the play
  area, runs the map-FSM (mapfsm.h -- the early-fps fsm.rs logic) on each,
  and spawns the resulting cover/destructible chunks into a chunk world.
  Also records where features landed so the game layer can place matching
  visual models.

  This is what turns "an empty level + a wall" into an actual arena that
  is dressed differently every match (different seed => different layout),
  realising the design doc's "endgame feels different than the beginning".
*/

#ifndef EX_ARENA_H
#define EX_ARENA_H

#include "math/mathlib.h"
#include "coll_chunk.h"
#include "mapfsm.h"
#include <stdint.h>

#define EX_ARENA_MAX_FEATURES 64

// one placed feature -- enough for the game layer to spawn a visual for it
typedef struct {
  ex_feature_t feature;
  vec3  position;
  vec3  half_extents;
  int   destructible;
} ex_placed_feature_t;

typedef struct {
  ex_placed_feature_t features[EX_ARENA_MAX_FEATURES];
  int count;
} ex_arena_t;

/**
 * [ex_arena_generate scatter sites over [min_xz,max_xz] and grow features]
 * @param arena    [output: filled with placed features]
 * @param chunks   [chunk world the spawned chunks are added to]
 * @param min_x,min_z,max_x,max_z [play-area bounds]
 * @param ground_y [floor height]
 * @param grid     [sites per axis (grid*grid candidate spots)]
 * @param seed     [match seed -- same seed => same layout]
 *
 * Site environment (sightline/slope/high-point/traffic) is synthesised from
 * a cheap hash of the grid cell so the FSM has varied input to gate on; a
 * real integration would sample these from the actual level geometry.
 */
void ex_arena_generate(ex_arena_t *arena, ex_chunk_world_t *chunks,
                       float min_x, float min_z, float max_x, float max_z,
                       float ground_y, int grid, uint32_t seed);

#endif // EX_ARENA_H
