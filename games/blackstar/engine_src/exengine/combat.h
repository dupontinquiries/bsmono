/* combat
  Hitscan resolution, ported in spirit from early-fps's combat.rs.

  A single ray is cast from a muzzle and we return the NEAREST of:
    - a destructible/cover chunk (ray vs AABB), or
    - an enemy (ray vs sphere).
  The caller applies damage to whatever the ray reached first. This matches
  the Cybertron design's long-range engagement: ranged characters fire
  hitscan beams down open lanes, and cover chunks block/absorb them.

  Kept dependency-light: works off coll_chunk's chunk world plus a small
  array of target spheres, so combat doesn't need to know about the enemy
  module's internals.
*/

#ifndef EX_COMBAT_H
#define EX_COMBAT_H

#include "math/mathlib.h"
#include "coll_chunk.h"
#include <stdint.h>

typedef enum {
  EX_HIT_NONE = 0,
  EX_HIT_CHUNK,
  EX_HIT_ENEMY
} ex_hit_kind;

typedef struct {
  ex_hit_kind kind;
  vec3  point;      // world-space impact point
  float dist;       // distance from origin
  int   index;      // chunk index or enemy id, depending on kind
} ex_hit_t;

// A spherical hittable (an enemy torso), passed in so combat stays decoupled.
typedef struct {
  vec3  center;
  float radius;
  int   id;
  int   alive;
} ex_target_t;

/**
 * [ex_ray_aabb ray vs axis-aligned box. Returns 1 and sets *t (entry dist)
 *  if the ray hits the box at t>0 within max_dist.]
 */
int ex_ray_aabb(vec3 origin, vec3 dir, vec3 box_center, vec3 box_half, float max_dist, float *t);

/**
 * [ex_ray_sphere ray vs sphere. Returns 1 and sets *t if hit at t>0.]
 */
int ex_ray_sphere(vec3 origin, vec3 dir, vec3 center, float radius, float max_dist, float *t);

/**
 * [ex_combat_raycast cast a ray against all solid chunks and live targets,
 *  return the nearest hit]
 * @param chunks   [chunk world (only solid chunks block)]
 * @param targets  [array of enemy target spheres]
 * @param n_targets[count]
 * @param origin   [ray start]
 * @param dir      [normalized ray direction]
 * @param max_dist [max range]
 * @return         [nearest hit; kind==EX_HIT_NONE if nothing in range]
 */
ex_hit_t ex_combat_raycast(ex_chunk_world_t *chunks, const ex_target_t *targets, int n_targets,
                           vec3 origin, vec3 dir, float max_dist);

#endif // EX_COMBAT_H
