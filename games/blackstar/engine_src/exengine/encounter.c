#include "encounter.h"
#include <string.h>
#include <math.h>

static void ex_encounter_assign_zones(ex_encounter_t *e, vec3 player_pos);

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
  e->enemies[idx].zone = -1;       // unassigned -> zoned once on first update, then sticky
  e->spawned_total++;
  return idx;
}

// Spawn a COMMANDER + a squad of troops bound to it. The troops carry the
// commander's id in their squad field, so killing the commander scatters
// exactly this group. Troops are ringed around the center. Returns the
// commander's enemy index (or -1 if full before it could be placed).
int ex_encounter_spawn_squad(ex_encounter_t *e, ex_enemy_kind cmd_kind,
                             ex_enemy_kind troop_kind, ex_element_t el,
                             vec3 center, float cmd_hp, float troop_hp,
                             int n_troops, float ring_r)
{
  int ci = ex_encounter_spawn(e, cmd_kind, el, center, cmd_hp);
  if (ci < 0) return -1;
  e->enemies[ci].is_commander = 1;
  int cmd_id = e->enemies[ci].id;
  for (int k = 0; k < n_troops; k++) {
    float a = (float)k / (float)(n_troops > 0 ? n_troops : 1) * 6.28318f;
    vec3 p = { center[0] + cosf(a)*ring_r, center[1], center[2] + sinf(a)*ring_r };
    int ti = ex_encounter_spawn(e, troop_kind, el, p, troop_hp);
    if (ti < 0) break;
    e->enemies[ti].squad = cmd_id;      // answers to this commander
  }
  return ci;
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
  // Destiny owned-space: assign each enemy to a front/mid/back line by its
  // distance from the player, so they hold their zone instead of all swarming.
  ex_encounter_assign_zones(e, player_pos);

  for (int i = 0; i < e->count; i++)
    ex_enemy_update(&e->enemies[i], chunks, player_pos, dt);

  // collapse a line once it's been cleared -> survivors advance to claim space
  ex_encounter_update_zones(e);

  // commander cohesion: leader deaths scatter their squads
  ex_encounter_update_command(e, dt);

  if (e->mode == EX_WIN_SURVIVE) {
    if (e->survive_timer > 0.0f) e->survive_timer -= dt;
    if (e->survive_timer <= 0.0f) e->cleared = 1;
  } else {
    e->cleared = (ex_encounter_alive(e) == 0 && e->count > 0);
  }
}

// --- Destiny "owned space" zone system ---------------------------------------
// Standoff hold-distances per zone. Front presses close (brawl range), middle
// holds mid, back holds far (anchors/snipers projecting danger from range).
static const float EX_ZONE_STANDOFF[3] = { 4.0f, 16.0f, 30.0f };

// Assign each live enemy to a line by its distance from the player, nearest
// third = front, etc. Anchors (Apex/Corrupted) bias toward holding further
// back where their danger-projection matters; Hijacks push to the front.
static void ex_encounter_assign_zones(ex_encounter_t *e, vec3 player_pos)
{
  for (int i = 0; i < e->count; i++) {
    ex_enemy_t *en = &e->enemies[i];
    if (en->state == EX_ENEMY_DEAD) continue;
    // STICKY zones: only assign an INITIAL zone (once) from the enemy's spawn
    // distance. After that the zone persists and is only changed by
    // update_zones' collapse-advance -- otherwise re-deriving from distance
    // every frame would instantly undo the advance and the "claim space"
    // mechanic would never take effect.
    if (en->zone >= 0) continue;                                // already zoned -> leave it
    float dx = en->position[0]-player_pos[0], dz = en->position[2]-player_pos[2];
    float d = sqrtf(dx*dx + dz*dz);
    int z = (d < 12.0f) ? 0 : (d < 26.0f) ? 1 : 2;
    if (en->kind == EX_ENEMY_HIJACK && z > 0) z -= 1;          // scouts crowd forward
    if ((en->kind == EX_ENEMY_APEX || en->kind == EX_ENEMY_CORRUPTED) && z < 2) z += 1; // anchors hold back
    en->zone = z;
    en->standoff = EX_ZONE_STANDOFF[z];
  }
}

// When a front line has been thinned enough, collapse it: survivors in that
// line and behind advance one line forward (smaller standoff), claiming the
// vacated space -- the player's progress pushing the fight forward.
void ex_encounter_update_zones(ex_encounter_t *e)
{
  int alive_in[3] = {0,0,0}, total_in[3] = {0,0,0};
  for (int i = 0; i < e->count; i++) {
    ex_enemy_t *en = &e->enemies[i];
    int z = en->zone; if (z<0||z>2) continue;
    total_in[z]++;
    if (en->state != EX_ENEMY_DEAD) alive_in[z]++;
  }
  // front collapses when <=1 of its line remains alive: everyone advances
  for (int line = 0; line < 2; line++) {
    if (total_in[line] > 0 && alive_in[line] <= 1) {
      for (int i = 0; i < e->count; i++) {
        ex_enemy_t *en = &e->enemies[i];
        if (en->state == EX_ENEMY_DEAD) continue;
        if (en->zone > line) {                 // behind the collapsed line: advance
          en->zone -= 1;
          en->standoff = EX_ZONE_STANDOFF[en->zone];
        }
      }
    }
  }
}

// --- commander / squad cohesion --------------------------------------------
// SCATTER_TIME: how long a leaderless troop panics (breaks zone discipline).
#define EX_SCATTER_TIME 4.0f

// Is a squad's commander still alive?
static int squad_has_commander(ex_encounter_t *e, int squad_id)
{
  if (squad_id < 0) return 0;
  for (int i = 0; i < e->count; i++) {
    ex_enemy_t *en = &e->enemies[i];
    if (en->is_commander && en->id == squad_id && en->state != EX_ENEMY_DEAD)
      return 1;
  }
  return 0;
}

// Each update: when a squad's commander has just died, its surviving troops
// scatter -- panic-flee for a while, then fight on demoralized. Also decays the
// per-troop scatter timer. A squad that gains a live commander again rallies.
void ex_encounter_update_command(ex_encounter_t *e, float dt)
{
  for (int i = 0; i < e->count; i++) {
    ex_enemy_t *en = &e->enemies[i];
    if (en->state == EX_ENEMY_DEAD) continue;
    if (en->is_commander || en->squad < 0) continue;      // leaders + independents unaffected

    int has_cmd = squad_has_commander(e, en->squad);
    if (!has_cmd && !en->demoralized && en->scatter_t <= 0.0f) {
      // commander just lost -> scatter this troop
      en->scatter_t = EX_SCATTER_TIME;
    }
    if (has_cmd) {
      // a commander is (still/again) leading -> disciplined, clear panic
      en->scatter_t = 0.0f;
      en->demoralized = 0;
    }
    if (en->scatter_t > 0.0f) {
      en->scatter_t -= dt;
      if (en->scatter_t <= 0.0f) {
        en->scatter_t = 0.0f;
        en->demoralized = 1;        // panic over -> fight on, but shaken
      }
    }
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
