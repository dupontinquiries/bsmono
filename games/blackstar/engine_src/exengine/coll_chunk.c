#include "coll_chunk.h"
#include <stdlib.h>
#include <string.h>
#include <math.h>

#define EX_CHUNK_WORLD_INITIAL_CAP 32

ex_chunk_world_t* ex_chunk_world_new(void)
{
  ex_chunk_world_t *world = malloc(sizeof(ex_chunk_world_t));
  world->count = 0;
  world->capacity = EX_CHUNK_WORLD_INITIAL_CAP;
  world->chunks = malloc(sizeof(ex_coll_chunk_t*) * world->capacity);
  return world;
}

void ex_chunk_world_free(ex_chunk_world_t *world)
{
  if (!world)
    return;

  for (int i = 0; i < world->count; i++) {
    free(world->chunks[i]->tris);
    free(world->chunks[i]);
  }

  free(world->chunks);
  free(world);
}

ex_coll_chunk_t* ex_chunk_add(ex_chunk_world_t *world, const char *name,
                               rect_t bounds, vec3 *tris, int tri_count,
                               float hp, uint32_t hit_mask, int destructible)
{
  if (world->count >= world->capacity) {
    world->capacity *= 2;
    world->chunks = realloc(world->chunks, sizeof(ex_coll_chunk_t*) * world->capacity);
  }

  ex_coll_chunk_t *chunk = malloc(sizeof(ex_coll_chunk_t));
  memset(chunk, 0, sizeof(ex_coll_chunk_t));

  strncpy(chunk->name, name, EX_CHUNK_NAME_MAX - 1);
  chunk->bounds = bounds;

  chunk->tri_count = tri_count;
  chunk->tris = malloc(sizeof(vec3) * tri_count * 3);
  memcpy(chunk->tris, tris, sizeof(vec3) * tri_count * 3);

  chunk->state = EX_CHUNK_STATE_INTACT;
  chunk->hit_mask = hit_mask;
  chunk->hp = hp;
  chunk->max_hp = hp;
  chunk->destructible = destructible;
  chunk->material = EX_MATCLASS_HUMAN;                     // default tier; override via ex_chunk_set_material
  chunk->strength = ex_matclass_default_strength(EX_MATCLASS_HUMAN);
  chunk->dirty = 0;

  world->chunks[world->count++] = chunk;
  return chunk;
}

ex_coll_chunk_t* ex_chunk_find(ex_chunk_world_t *world, const char *name)
{
  for (int i = 0; i < world->count; i++) {
    if (strncmp(world->chunks[i]->name, name, EX_CHUNK_NAME_MAX) == 0)
      return world->chunks[i];
  }
  return NULL;
}

void ex_chunk_set_material(ex_coll_chunk_t *chunk, ex_matclass m, float strength)
{
  chunk->material = m;
  chunk->strength = (strength > 0.0f) ? strength : ex_matclass_default_strength(m);
}

float ex_matclass_effective_damage(float amount, float strength)
{
  if (strength <= 0.0f) return amount;         // no material gating
  if (amount >= strength) return amount;       // overmatch: full bite
  float r = amount / strength;                 // 0..1, how close to threshold
  if (r < 0.125f) return 0.0f;                 // hard floor: too weak to mark it at all
  // smooth falloff: r^1.5 keeps near-threshold hits meaningful, weak hits tiny
  return amount * (r * sqrtf(r));
}

int ex_chunk_apply_damage(ex_coll_chunk_t *chunk, float amount, uint32_t source_mask)
{
  if (!chunk->destructible)
    return 0;

  if (chunk->state == EX_CHUNK_STATE_DESTROYED)
    return 0;

  // this hit type isn't allowed to affect this chunk
  // (e.g. a Standard-class melee hit against a Heavy-only wall)
  if (!(chunk->hit_mask & source_mask))
    return 0;

  // MATERIAL PENETRATION: scale the incoming damage by how much this hit can
  // actually bite the chunk's material. Clone hull shrugs off small-arms.
  float bite = ex_matclass_effective_damage(amount, chunk->strength);
  if (bite <= 0.0f)
    return 0;

  chunk->hp -= bite;
  chunk->dirty = 1;

  if (chunk->hp <= 0.0f) {
    chunk->hp = 0.0f;
    ex_chunk_set_state(chunk, EX_CHUNK_STATE_DESTROYED);
    return 1;
  }

  // still standing, but visibly cracked/damaged past the halfway point
  if (chunk->state == EX_CHUNK_STATE_INTACT && chunk->hp <= chunk->max_hp * 0.5f)
    ex_chunk_set_state(chunk, EX_CHUNK_STATE_DAMAGED);

  return 0;
}

void ex_chunk_set_state(ex_coll_chunk_t *chunk, ex_chunk_state new_state)
{
  if (chunk->state == new_state)
    return;

  chunk->state = new_state;
  chunk->dirty = 1;
  // NOTE: caller (game layer) is responsible for:
  //  - swapping the rendered mesh to match new_state
  //  - broadcasting this change to other clients if server-authoritative
  //  - firing any dependent map events (e.g. "velatris_bridge_02_destroyed")
}

void ex_chunk_world_collide(ex_chunk_world_t *world, ex_coll_packet_t *packet)
{
  for (int i = 0; i < world->count; i++) {
    ex_coll_chunk_t *chunk = world->chunks[i];

    if (!ex_chunk_is_solid(chunk))
      continue;

    // broad phase: skip chunks the sweep can't possibly reach
    if (!ex_rect_intersect_sphere(chunk->bounds, packet->r3_position, 1.0f + vec3_len(packet->r3_velocity)))
      continue;

    // narrow phase: same per-triangle test used against static level geo
    for (int t = 0; t < chunk->tri_count; t++) {
      vec3 *tri = &chunk->tris[t * 3];
      ex_collision_check_triangle(packet, tri[0], tri[1], tri[2]);
    }
  }
}

void ex_chunk_world_clear_dirty(ex_chunk_world_t *world)
{
  for (int i = 0; i < world->count; i++)
    world->chunks[i]->dirty = 0;
}
