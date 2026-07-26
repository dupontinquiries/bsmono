#include "enemy.h"
#include <math.h>
#include <string.h>

static int s_cloak = 0;
void ex_enemy_set_cloak(int on){ s_cloak = on; }

ex_enemy_t ex_enemy_new(int id, vec3 pos, float hp)
{
  return ex_enemy_new_kind(id, pos, hp, EX_ENEMY_GRUNT, EX_ELEM_NONE);
}

ex_enemy_t ex_enemy_new_kind(int id, vec3 pos, float hp, ex_enemy_kind kind, ex_element_t element)
{
  ex_enemy_t e;
  memset(&e, 0, sizeof(e));
  e.id = id;
  e.squad = -1;         // independent until a commander claims it
  e.kind = kind;
  e.element = element;
  e.position[0] = pos[0]; e.position[1] = pos[1]; e.position[2] = pos[2];
  e.radius = 0.9f;
  e.hp = hp; e.max_hp = hp;
  e.state = EX_ENEMY_IDLE;

  if (kind == EX_ENEMY_CORRUPTED) {
    // horror: fast, relentless, melee-only, notices you from anywhere (spark
    // resonance -- can't be hidden from). Begins at Stage 1 wrong-geometry and
    // escalates through Stage 2/3 over time, growing as it absorbs metal.
    e.radius = 1.1f;
    e.move_speed = 6.5f;
    e.sight_range = 1e9f;   // hunts by spark resonance
    e.attack_range = 2.2f;  // melee lunge
    e.fire_cooldown = 0.8f;
    e.fire_timer = 0.0f;
    e.stage = 1;
    e.stage_timer = 0.0f;
    e.grow = 1.0f;
    e.transmits = 1;        // corrupted energon transmits on contact
  }
  if (kind == EX_ENEMY_HIJACK) {
    // Trilogy Two swarm unit: fast, fragile, erratic, indifferent to its own
    // survival -- it never flees and closes with short weak shots. Deadly only
    // in numbers, which is exactly how the Hijacks fight.
    e.radius = 0.8f;
    e.hp = e.max_hp = 45.0f;
    e.move_speed = 8.5f;
    e.sight_range = 130.0f;
    e.attack_range = 70.0f;   // erratic ranged pot-shots while closing, from range
    e.fire_cooldown = 0.5f;
    e.fire_timer = 0.0f;
  }
  if (kind == EX_ENEMY_APEX) {
    // the apex Corrupted: the largest, weeks of absorbed infrastructure, a
    // shifting mass with a "preferred configuration" it returns to. Slow but
    // relentless, huge HP, phase-gated: it reconfigures as it takes damage --
    // adding a ground-slam wave, then summoning Hijack scouts.
    e.radius = 3.0f;
    e.hp = e.max_hp = (hp > 0.0f ? hp : 1600.0f);
    e.move_speed = 3.6f;
    e.sight_range = 1e9f;     // hunts by spark resonance
    e.attack_range = 4.5f;
    e.fire_cooldown = 1.2f;
    e.fire_timer = 0.0f;
    e.stage = 3;              // fully corrupted from the start
    e.grow = 1.0f;
    e.transmits = 1;
    e.phase = 1;
    e.slam_timer = 6.0f;
    e.summon_timer = 12.0f;
  } else if (kind == EX_ENEMY_GRUNT) {
    // standard ranged trooper: the default enemy. Long sight + range so it
    // engages from real distance instead of being a free target up close.
    e.move_speed = 4.0f;
    e.sight_range = 140.0f;  // long-range design: notice + engage from real distance
    e.attack_range = 90.0f;  // open fire at range (was 22 -- you could poke them freely from outside it)
    e.fire_cooldown = 1.1f;
    e.fire_timer = 0.0f;
  }
  // NOTE: CORRUPTED and HIJACK set their own stats in their if-blocks above and
  // must NOT be caught by a catch-all else here (that was a bug: it clobbered
  // the melee-hunter + swarm profiles with grunt ranges).
  e.wants_to_fire = 0;
  return e;
}

ex_target_t ex_enemy_as_target(const ex_enemy_t *e)
{
  ex_target_t t;
  t.center[0] = e->position[0];
  t.center[1] = e->position[1];
  t.center[2] = e->position[2];
  t.radius = e->radius;
  t.id = e->id;
  t.alive = (e->state != EX_ENEMY_DEAD);
  return t;
}

int ex_enemy_has_los(const ex_enemy_t *e, ex_chunk_world_t *chunks, vec3 target)
{
  vec3 dir = {
    target[0] - e->position[0],
    target[1] - e->position[1],
    target[2] - e->position[2],
  };
  float dist = sqrtf(dir[0]*dir[0] + dir[1]*dir[1] + dir[2]*dir[2]);
  if (dist < 1e-4f) return 1;
  float inv = 1.0f / dist;
  dir[0]*=inv; dir[1]*=inv; dir[2]*=inv;

  // a chunk blocks LOS if the ray hits one before reaching the target
  ex_hit_t hit = ex_combat_raycast(chunks, NULL, 0, (float*)e->position, dir, dist);
  return hit.kind == EX_HIT_NONE; // clear if nothing solid in the way
}

int ex_enemy_damage(ex_enemy_t *e, float amount)
{
  if (e->state == EX_ENEMY_DEAD) return 0;
  e->hp -= amount;
  if (e->hp <= 0.0f) {
    e->hp = 0.0f;
    e->state = EX_ENEMY_DEAD;
    return 1;
  }
  // taking fire from an unseen source still pulls it into the fight
  if (e->state == EX_ENEMY_IDLE)
    e->state = EX_ENEMY_CHASE;
  return 0;
}

void ex_enemy_update(ex_enemy_t *e, ex_chunk_world_t *chunks, vec3 player_pos, float dt)
{
  e->wants_to_fire = 0;
  if (e->state == EX_ENEMY_DEAD) return;

  if (e->fire_timer > 0.0f) e->fire_timer -= dt;

  // Corrupted escalate: Stage 1 -> 2 -> 3 over time. Each stage is faster and
  // hits harder; Stage 3 keeps growing (absorbing surrounding metal).
  if (e->kind == EX_ENEMY_CORRUPTED && e->stage < 3) {
    e->stage_timer += dt;
    float threshold = (e->stage == 1) ? 10.0f : 16.0f; // s to next stage
    if (e->stage_timer >= threshold) {
      e->stage++; e->stage_timer = 0.0f;
      e->move_speed *= 1.15f;
      e->radius *= 1.12f;
    }
  }
  if (e->kind == EX_ENEMY_CORRUPTED && e->stage >= 3) {
    e->grow += dt * 0.03f;              // slow relentless growth
    if (e->grow > 2.2f) e->grow = 2.2f;
  }

  // apex boss: phase by remaining HP, and time its slam / summon signals.
  e->wants_slam = 0; e->wants_summon = 0;
  if (e->kind == EX_ENEMY_APEX) {
    e->grow += dt * 0.02f; if (e->grow > 1.8f) e->grow = 1.8f;
    float frac = e->hp / e->max_hp;
    int new_phase = (frac > 0.66f) ? 1 : (frac > 0.33f) ? 2 : 3;
    if (new_phase != e->phase) {          // reconfigure on phase change
      e->phase = new_phase;
      e->move_speed += 0.8f;              // faster each phase
      e->slam_timer = 3.0f;
    }
    if (e->phase >= 2) {                   // ground-slam from phase 2
      e->slam_timer -= dt;
      if (e->slam_timer <= 0.0f) { e->wants_slam = 1; e->slam_timer = 5.0f; }
    }
    if (e->phase >= 3) {                   // summons Hijack scouts in phase 3
      e->summon_timer -= dt;
      if (e->summon_timer <= 0.0f) { e->wants_summon = 1; e->summon_timer = 9.0f; }
    }
  }

  vec3 to_player = {
    player_pos[0] - e->position[0],
    player_pos[1] - e->position[1],
    player_pos[2] - e->position[2],
  };
  float dist = sqrtf(to_player[0]*to_player[0] + to_player[1]*to_player[1] + to_player[2]*to_player[2]);
  // Corrupted & the Apex hunt by spark resonance -- they always know where you
  // are, ignore line of sight, and are NOT fooled by cloak. Grunts and Hijacks
  // rely on sight, so an active cloak makes them lose track of you entirely.
  int spark_hunter = (e->kind == EX_ENEMY_CORRUPTED || e->kind == EX_ENEMY_APEX);
  int has_los = spark_hunter ? 1 : ex_enemy_has_los(e, chunks, player_pos);
  if (s_cloak && !spark_hunter) {
    has_los = 0;
    if (e->state == EX_ENEMY_CHASE || e->state == EX_ENEMY_ATTACK) e->state = EX_ENEMY_IDLE;
  }

  switch (e->state) {
    case EX_ENEMY_IDLE:
      if (dist <= e->sight_range && has_los)
        e->state = EX_ENEMY_CHASE;
      break;

    case EX_ENEMY_CHASE: {
      // COMMANDER DOWN -> SCATTER: with its leader dead, this troop panics and
      // flees the player erratically, abandoning its zone. This is the payoff
      // for decapitating a squad -- it visibly breaks apart.
      if (e->scatter_t > 0.0f) {
        if (dist > 1e-3f) {
          float inv = 1.0f / dist;
          float ax = -to_player[0] * inv, az = -to_player[2] * inv;  // away from player
          e->stage_timer += dt;
          float w = sinf(e->stage_timer * 5.0f + (float)e->id) * 0.9f; // erratic veer
          float px = -az, pz = ax;
          ax += px * w; az += pz * w;
          float l = sqrtf(ax*ax+az*az); if (l>1e-3f){ ax/=l; az/=l; }
          e->position[0] += ax * e->move_speed * 1.2f * dt;   // flee a touch faster
          e->position[2] += az * e->move_speed * 1.2f * dt;
        }
        break;   // no firing while panicking
      }
      // Destiny owned-space: hold at the zone's standoff distance rather than
      // crowding the player. Front line stands off close, back line far.
      // DEMORALIZED squads (leader dead, panic over) hold much wider + shyer.
      float hold = e->standoff;
      if (e->demoralized) hold = (hold > 0.0f ? hold : 8.0f) + 18.0f;
      // fire whenever in weapon range AND we've reached our hold line (or have
      // no hold). standoff governs WALK distance; attack_range governs SHOOT
      // distance -- so a back-line unit holds far but still opens fire at range.
      int at_hold = (hold <= 0.0f) || (dist <= hold + 1.0f);
      if (dist <= e->attack_range && has_los && at_hold) {
        e->state = EX_ENEMY_ATTACK;
        break;
      }
      // walk toward the player on XZ, but stop advancing once at standoff
      if (dist > 1e-3f && (hold <= 0.0f || dist > hold)) {
        float inv = 1.0f / dist;
        float mx = to_player[0] * inv, mz = to_player[2] * inv;
        // Hijacks move erratically -- a wobbling strafe layered on the approach,
        // so a swarm jitters and weaves rather than marching in a line.
        if (e->kind == EX_ENEMY_HIJACK) {
          e->stage_timer += dt; // reuse timer as a wobble phase
          float w = sinf(e->stage_timer * 6.0f + (float)e->id) * 0.7f;
          float px = -mz, pz = mx; // perpendicular
          mx += px * w; mz += pz * w;
          float l = sqrtf(mx*mx+mz*mz); if (l>1e-3f){ mx/=l; mz/=l; }
        }
        e->position[0] += mx * e->move_speed * dt;
        e->position[2] += mz * e->move_speed * dt;
      }
      break;
    }

    case EX_ENEMY_ATTACK:
      if (dist > e->attack_range || !has_los) {
        e->state = EX_ENEMY_CHASE;
        break;
      }
      // in range with LOS: fire on cooldown
      if (e->fire_timer <= 0.0f) {
        e->wants_to_fire = 1;
        e->fire_timer = e->fire_cooldown;
      }
      break;

    case EX_ENEMY_DEAD:
    default:
      break;
  }
}

const char* ex_enemy_stage_name(int stage){
  switch(stage){ case 1: return "wrong-geometry"; case 2: return "mid-corruption"; case 3: return "full-corruption"; default: return ""; }
}
float ex_enemy_melee_damage(const ex_enemy_t *e){
  float base = 15.0f;
  if (e->kind == EX_ENEMY_APEX) return 34.0f * e->grow;   // heavy boss blows
  if (e->kind != EX_ENEMY_CORRUPTED) return base;
  return base * (0.7f + 0.5f*e->stage) * e->grow; // stage + growth scaling
}
float ex_enemy_visual_radius(const ex_enemy_t *e){
  if (e->kind == EX_ENEMY_APEX) return e->radius * e->grow;
  return e->radius * ((e->kind==EX_ENEMY_CORRUPTED && e->stage>=3) ? e->grow : 1.0f);
}

const char* ex_enemy_state_name(ex_enemy_state s)
{
  switch (s) {
    case EX_ENEMY_IDLE:   return "idle";
    case EX_ENEMY_CHASE:  return "chase";
    case EX_ENEMY_ATTACK: return "attack";
    case EX_ENEMY_DEAD:   return "dead";
    default:              return "?";
  }
}
