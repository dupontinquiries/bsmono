#include "encounter.h"
#include <string.h>
#include <math.h>

static float hash01(uint32_t x, uint32_t salt)
{
  uint32_t h = x*0x9E3779B1u ^ (salt+0x85EBCA6Bu);
  h^=h>>15; h*=0x2C1B3C6Du; h^=h>>12; h*=0x297A2D39u; h^=h>>15;
  return (float)(h&0xFFFFFF)/(float)0x1000000;
}

void ex_encounter_init(ex_encounter_t *e, ex_win_mode_t mode, float survive_seconds)
{
  memset(e, 0, sizeof(*e));
  e->mode = mode;
  e->survive_timer = survive_seconds;
}

int ex_encounter_spawn(ex_encounter_t *e, ex_enemy_kind kind, ex_element_t el,
                       vec3 pos, float hp)
{
  if (e->count >= EX_MAX_ENEMIES) return -1;
  int idx = e->count++;
  int id = 100 + idx; // stable ids
  e->enemies[idx] = ex_enemy_new_kind(id, pos, hp, kind, el);
  e->spawned_total++;
  return idx;
}

void ex_encounter_spawn_makeup(ex_encounter_t *e, int grunts, int corrupted, int hijacks,
                               float min_x, float min_z, float max_x, float max_z,
                               uint32_t seed)
{
  float cx = (min_x+max_x)*0.5f, cz = (min_z+max_z)*0.5f;
  float span = fminf(max_x-min_x, max_z-min_z) * 0.42f;
  int total = grunts + corrupted + hijacks;
  int placed = 0;
  // elements to rotate grunts through for variety
  ex_element_t els[6] = { EX_ELEM_ROCK, EX_ELEM_FIRE, EX_ELEM_ICE, EX_ELEM_ELECTRIC, EX_ELEM_WATER, EX_ELEM_AIR };

  for (int i = 0; i < grunts; i++) {
    float a = (float)(placed) / (total>0?total:1) * 6.2831853f + hash01(i, seed)*0.6f;
    float r = span * (0.6f + hash01(i, seed^0x11)*0.4f);
    vec3 p = { cx + cosf(a)*r, 0.0f, cz + sinf(a)*r };
    ex_encounter_spawn(e, EX_ENEMY_GRUNT, els[i%6], p, 120.0f);
    placed++;
  }
  for (int i = 0; i < corrupted; i++) {
    float a = (float)(placed) / (total>0?total:1) * 6.2831853f + hash01(i, seed^0x99)*0.6f;
    float r = span * (0.7f + hash01(i, seed^0x22)*0.3f);
    vec3 p = { cx + cosf(a)*r, 0.0f, cz + sinf(a)*r };
    ex_encounter_spawn(e, EX_ENEMY_CORRUPTED, EX_ELEM_VINE, p, 90.0f);
    placed++;
  }
  for (int i = 0; i < hijacks; i++) {
    float a = (float)(placed) / (total>0?total:1) * 6.2831853f + hash01(i, seed^0x55)*0.9f;
    float r = span * (0.5f + hash01(i, seed^0x33)*0.5f);
    vec3 p = { cx + cosf(a)*r, 0.0f, cz + sinf(a)*r };
    ex_encounter_spawn(e, EX_ENEMY_HIJACK, els[i%6], p, 45.0f);
    placed++;
  }
}

void ex_encounter_update(ex_encounter_t *e, ex_chunk_world_t *chunks,
                         vec3 player_pos, float dt)
{
  for (int i = 0; i < e->count; i++)
    ex_enemy_update(&e->enemies[i], chunks, player_pos, dt);

  if (e->mode == EX_WIN_SURVIVE) {
    if (e->survive_timer > 0.0f) e->survive_timer -= dt;
    if (e->survive_timer <= 0.0f) e->cleared = 1;
  } else {
    e->cleared = (ex_encounter_alive(e) == 0 && e->count > 0);
  }
}

int ex_encounter_targets(const ex_encounter_t *e, ex_target_t *out, int max)
{
  int n = 0;
  for (int i = 0; i < e->count && n < max; i++) {
    if (e->enemies[i].state == EX_ENEMY_DEAD) continue;
    out[n++] = ex_enemy_as_target(&e->enemies[i]);
  }
  return n;
}

ex_enemy_t* ex_encounter_by_id(ex_encounter_t *e, int id)
{
  for (int i = 0; i < e->count; i++)
    if (e->enemies[i].id == id) return &e->enemies[i];
  return NULL;
}

int ex_encounter_boss(ex_encounter_t *e, float cx, float cz, float hp)
{
  vec3 p = { cx, 0.0f, cz };
  return ex_encounter_spawn(e, EX_ENEMY_APEX, EX_ELEM_VINE, p, hp);
}
ex_enemy_t* ex_encounter_boss_ptr(ex_encounter_t *e)
{
  for (int i=0;i<e->count;i++)
    if (e->enemies[i].kind==EX_ENEMY_APEX && e->enemies[i].state!=EX_ENEMY_DEAD)
      return &e->enemies[i];
  return NULL;
}

int ex_encounter_alive(const ex_encounter_t *e)
{
  int n = 0;
  for (int i = 0; i < e->count; i++)
    if (e->enemies[i].state != EX_ENEMY_DEAD) n++;
  return n;
}
