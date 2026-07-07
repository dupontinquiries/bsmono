#include "mapfsm.h"
#include <string.h>
#include <stdio.h>
#include <math.h>

// mirrors fsm.rs constants
#define SIGHTLINE_BREAKER_MIN   18.0f
#define SIGHTLINE_BREAKER_LONG  34.0f
#define WRECKAGE_MIN_SIGHTLINE  24.0f
#define SENTINEL_MIN_SIGHTLINE  22.0f
#define SENTINEL_MIN_SLOPE      0.12f

// Each feature is a small gate function returning a result (feature NONE if it
// declines). Order below = priority; the first that grows wins its site,
// matching the "one agent claims the spot" behaviour of the level builder.

static ex_fsm_result_t none_result(void)
{
  ex_fsm_result_t r; memset(&r, 0, sizeof(r));
  r.feature = EX_FEATURE_NONE;
  return r;
}

// SightlineBreaker: cover sized to the sightline it interrupts.
static ex_fsm_result_t grow_sightline_breaker(const ex_site_t *s)
{
  if (s->sightline < SIGHTLINE_BREAKER_MIN)
    return none_result();

  int long_lane = s->sightline > SIGHTLINE_BREAKER_LONG;
  float bump = s->high_traffic ? 0.25f : 0.0f;
  float sy = long_lane ? 1.5f : 0.6f;

  ex_fsm_result_t r = none_result();
  r.feature = EX_FEATURE_SIGHTLINE_BREAKER;
  r.spawn_chunk = 1;
  r.destructible = 1;
  r.half_extents[0] = 1.0f + bump;
  r.half_extents[1] = sy;
  r.half_extents[2] = 0.7f + bump;
  // taller/high cover is sturdier
  r.hp = long_lane ? 140.0f : 80.0f;
  r.hit_mask = EX_CHUNK_HIT_HEAVY | EX_CHUNK_HIT_TITAN | EX_CHUNK_HIT_VEHICLE_RAM;
  return r;
}

// VerticalRiskReward: a vantage, only on the high ground.
static ex_fsm_result_t grow_vantage(const ex_site_t *s)
{
  if (!s->is_high_point)
    return none_result();

  ex_fsm_result_t r = none_result();
  r.feature = EX_FEATURE_VANTAGE;
  r.spawn_chunk = 1;
  r.destructible = 0; // structural vantage, not a breakable cover
  r.half_extents[0] = 1.6f;
  r.half_extents[1] = 2.4f;
  r.half_extents[2] = 1.6f;
  r.hp = 0.0f;
  r.hit_mask = 0;
  return r;
}

// SentinelNode: fortified position, wants a slope overlooking open ground.
static ex_fsm_result_t grow_sentinel(const ex_site_t *s)
{
  if (s->slope < SENTINEL_MIN_SLOPE || s->sightline < SENTINEL_MIN_SIGHTLINE)
    return none_result();

  ex_fsm_result_t r = none_result();
  r.feature = EX_FEATURE_SENTINEL;
  r.spawn_chunk = 1;
  r.destructible = 1;
  r.half_extents[0] = 1.7f;
  r.half_extents[1] = 0.7f;
  r.half_extents[2] = 1.5f;
  r.hp = 200.0f; // concrete + sandbags, tough
  r.hit_mask = EX_CHUNK_HIT_HEAVY | EX_CHUNK_HIT_TITAN;
  return r;
}

// LogisticsSpine: tall landmark, wants open sky and to be the high point.
static ex_fsm_result_t grow_logistics_spine(const ex_site_t *s)
{
  if (s->indoor || !s->is_high_point)
    return none_result();

  ex_fsm_result_t r = none_result();
  r.feature = EX_FEATURE_LOGISTICS_SPINE;
  r.spawn_chunk = 1;
  r.destructible = 0; // landmark, structural
  r.half_extents[0] = 1.4f;
  r.half_extents[1] = 7.0f;
  r.half_extents[2] = 1.4f;
  r.hp = 0.0f;
  r.hit_mask = 0;
  return r;
}

// KineticWreckage: breaks long lanes.
static ex_fsm_result_t grow_wreckage(const ex_site_t *s)
{
  if (s->sightline < WRECKAGE_MIN_SIGHTLINE)
    return none_result();

  ex_fsm_result_t r = none_result();
  r.feature = EX_FEATURE_WRECKAGE;
  r.spawn_chunk = 1;
  r.destructible = 1;
  r.half_extents[0] = 3.0f;
  r.half_extents[1] = 0.9f;
  r.half_extents[2] = 3.0f;
  r.hp = 120.0f;
  r.hit_mask = EX_CHUNK_HIT_HEAVY | EX_CHUNK_HIT_TITAN | EX_CHUNK_HIT_VEHICLE_RAM;
  return r;
}

// AdHocShelter: makeshift shelter, wants open, not-too-steep, outdoor ground.
static ex_fsm_result_t grow_shelter(const ex_site_t *s)
{
  if (s->slope > 0.5f || s->indoor)
    return none_result();

  ex_fsm_result_t r = none_result();
  r.feature = EX_FEATURE_SHELTER;
  r.spawn_chunk = 1;
  r.destructible = 1;
  r.half_extents[0] = 1.5f;
  r.half_extents[1] = 0.9f;
  r.half_extents[2] = 1.5f;
  r.hp = 40.0f; // flimsy
  r.hit_mask = EX_CHUNK_HIT_ANY;
  return r;
}

ex_fsm_result_t ex_fsm_grow(const ex_site_t *site)
{
  // priority order: landmarks & vantages claim high points first, then
  // fortified positions on slopes, then lane-breaking cover/wreckage, then
  // opportunistic shelter fills the rest.
  ex_fsm_result_t r;

  r = grow_logistics_spine(site); if (r.feature != EX_FEATURE_NONE) return r;
  r = grow_vantage(site);         if (r.feature != EX_FEATURE_NONE) return r;
  r = grow_sentinel(site);        if (r.feature != EX_FEATURE_NONE) return r;
  r = grow_wreckage(site);        if (r.feature != EX_FEATURE_NONE) return r;
  r = grow_sightline_breaker(site);if (r.feature != EX_FEATURE_NONE) return r;
  r = grow_shelter(site);         if (r.feature != EX_FEATURE_NONE) return r;

  return none_result();
}

ex_feature_t ex_fsm_grow_into_world(ex_chunk_world_t *world, const ex_site_t *site, int index)
{
  ex_fsm_result_t r = ex_fsm_grow(site);
  if (r.feature == EX_FEATURE_NONE || !r.spawn_chunk)
    return r.feature;

  char name[48];
  snprintf(name, sizeof(name), "%s_%d", ex_feature_name(r.feature), index);

  vec3 tris[6] = {
    {0,0,0},{1,0,0},{1,1,0},
    {0,0,0},{1,1,0},{0,1,0},
  };
  rect_t bounds = ex_rect_new(
    (vec3){ site->x - r.half_extents[0], site->ground_y - r.half_extents[1], site->z - r.half_extents[2] },
    (vec3){ site->x + r.half_extents[0], site->ground_y + r.half_extents[1], site->z + r.half_extents[2] });

  ex_chunk_add(world, name, bounds, tris, 2, r.hp, r.hit_mask, r.destructible);
  return r.feature;
}

const char* ex_feature_name(ex_feature_t f)
{
  switch (f) {
    case EX_FEATURE_SIGHTLINE_BREAKER: return "breaker";
    case EX_FEATURE_VANTAGE:           return "vantage";
    case EX_FEATURE_SENTINEL:          return "sentinel";
    case EX_FEATURE_LOGISTICS_SPINE:   return "spine";
    case EX_FEATURE_WRECKAGE:          return "wreckage";
    case EX_FEATURE_SHELTER:           return "shelter";
    default:                           return "none";
  }
}
