#include "combat.h"
#include <math.h>
#include <float.h>

int ex_ray_aabb(vec3 origin, vec3 dir, vec3 box_center, vec3 box_half, float max_dist, float *t)
{
  // slab method
  float tmin = 0.0f, tmax = max_dist;
  for (int i = 0; i < 3; i++) {
    float lo = (box_center[i] - box_half[i] - origin[i]);
    float hi = (box_center[i] + box_half[i] - origin[i]);
    if (fabsf(dir[i]) < 1e-6f) {
      // ray parallel to slab: miss if origin outside the slab
      if (lo > 0.0f || hi < 0.0f) return 0;
    } else {
      float inv = 1.0f / dir[i];
      float t0 = lo * inv, t1 = hi * inv;
      if (t0 > t1) { float tmp = t0; t0 = t1; t1 = tmp; }
      if (t0 > tmin) tmin = t0;
      if (t1 < tmax) tmax = t1;
      if (tmin > tmax) return 0;
    }
  }
  if (tmin < 0.0f) return 0;
  *t = tmin;
  return 1;
}

int ex_ray_sphere(vec3 origin, vec3 dir, vec3 center, float radius, float max_dist, float *t)
{
  vec3 oc;
  oc[0] = origin[0] - center[0];
  oc[1] = origin[1] - center[1];
  oc[2] = origin[2] - center[2];
  float b = oc[0]*dir[0] + oc[1]*dir[1] + oc[2]*dir[2];
  float c = oc[0]*oc[0] + oc[1]*oc[1] + oc[2]*oc[2] - radius*radius;
  float disc = b*b - c;
  if (disc < 0.0f) return 0;
  float sq = sqrtf(disc);
  float t0 = -b - sq;
  if (t0 < 0.0f) t0 = -b + sq; // origin inside sphere -> use far root
  if (t0 < 0.0f || t0 > max_dist) return 0;
  *t = t0;
  return 1;
}

ex_hit_t ex_combat_raycast(ex_chunk_world_t *chunks, const ex_target_t *targets, int n_targets,
                           vec3 origin, vec3 dir, float max_dist)
{
  ex_hit_t best;
  best.kind = EX_HIT_NONE;
  best.dist = max_dist;
  best.index = -1;
  best.point[0] = origin[0] + dir[0]*max_dist;
  best.point[1] = origin[1] + dir[1]*max_dist;
  best.point[2] = origin[2] + dir[2]*max_dist;

  // chunks (only currently-solid ones block the shot)
  if (chunks) {
    for (int i = 0; i < chunks->count; i++) {
      ex_coll_chunk_t *ch = chunks->chunks[i];
      if (!ex_chunk_is_solid(ch)) continue;

      vec3 center = {
        (ch->bounds.min[0] + ch->bounds.max[0]) * 0.5f,
        (ch->bounds.min[1] + ch->bounds.max[1]) * 0.5f,
        (ch->bounds.min[2] + ch->bounds.max[2]) * 0.5f,
      };
      vec3 half = {
        (ch->bounds.max[0] - ch->bounds.min[0]) * 0.5f,
        (ch->bounds.max[1] - ch->bounds.min[1]) * 0.5f,
        (ch->bounds.max[2] - ch->bounds.min[2]) * 0.5f,
      };
      float t;
      if (ex_ray_aabb(origin, dir, center, half, best.dist, &t) && t < best.dist) {
        best.kind = EX_HIT_CHUNK;
        best.dist = t;
        best.index = i;
        best.point[0] = origin[0] + dir[0]*t;
        best.point[1] = origin[1] + dir[1]*t;
        best.point[2] = origin[2] + dir[2]*t;
      }
    }
  }

  // enemy target spheres
  for (int i = 0; i < n_targets; i++) {
    if (!targets[i].alive) continue;
    float t;
    if (ex_ray_sphere(origin, dir, (float*)targets[i].center, targets[i].radius, best.dist, &t)
        && t < best.dist) {
      best.kind = EX_HIT_ENEMY;
      best.dist = t;
      best.index = targets[i].id;
      best.point[0] = origin[0] + dir[0]*t;
      best.point[1] = origin[1] + dir[1]*t;
      best.point[2] = origin[2] + dir[2]*t;
    }
  }

  return best;
}
