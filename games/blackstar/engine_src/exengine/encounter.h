/* encounter
  Manages the set of enemies in a match: spawns the mission's makeup (N
  grunts + N corrupted) around the arena, updates them all against the player,
  builds the combat target list, and evaluates the win condition (eliminate
  all, or survive a timer). This turns a match from a fixed 1v1 into a real
  arena fight driven by the campaign/skirmish enemy counts.

  Pure logic over the enemy module (no rendering), so it is testable headless;
  the game maps each live enemy onto a pooled body each frame.
*/

#ifndef EX_ENCOUNTER_H
#define EX_ENCOUNTER_H

#include "enemy.h"
#include "combat.h"
#include "element.h"
#include "coll_chunk.h"

#define EX_MAX_ENEMIES 12

typedef enum { EX_WIN_ELIMINATE = 0, EX_WIN_SURVIVE = 1 } ex_win_mode_t;

typedef struct {
  ex_enemy_t enemies[EX_MAX_ENEMIES];
  int   count;
  ex_win_mode_t mode;
  float survive_timer;   // counts down (survive mode)
  int   cleared;         // win achieved
  int   spawned_total;   // for scoring
} ex_encounter_t;

void ex_encounter_init(ex_encounter_t *e, ex_win_mode_t mode, float survive_seconds);

/**
 * [ex_encounter_spawn add one enemy; returns its index or -1 if full]
 */
int ex_encounter_spawn(ex_encounter_t *e, ex_enemy_kind kind, ex_element_t el,
                       vec3 pos, float hp);

/**
 * [ex_encounter_spawn_squad spawn a commander + a ring of troops bound to it;
 *  killing the commander scatters exactly this squad. Returns commander index]
 */
int ex_encounter_spawn_squad(ex_encounter_t *e, ex_enemy_kind cmd_kind,
                             ex_enemy_kind troop_kind, ex_element_t el,
                             vec3 center, float cmd_hp, float troop_hp,
                             int n_troops, float ring_r);

/**
 * [ex_encounter_spawn_makeup scatter `grunts` + `corrupted` enemies around a
 *  ring inside the arena bounds, deterministic per seed]
 */
void ex_encounter_spawn_makeup(ex_encounter_t *e, int grunts, int corrupted, int hijacks,
                               float min_x, float min_z, float max_x, float max_z,
                               uint32_t seed);

/**
 * [ex_encounter_update tick every live enemy toward the player; advance the
 *  survive timer; evaluate the win condition]
 */
void ex_encounter_update(ex_encounter_t *e, ex_chunk_world_t *chunks,
                         vec3 player_pos, float dt);

/**
 * [ex_encounter_update_zones collapse a cleared front line, advancing the
 *  survivors forward (Destiny owned-space progression). Called by
 *  ex_encounter_update; exposed for testing.]
 */
void ex_encounter_update_zones(ex_encounter_t *e);

/**
 * [ex_encounter_update_command commander cohesion: when a squad's commander
 *  dies its surviving troops scatter (panic-flee, then fight on demoralized);
 *  a squad that still has a live commander stays disciplined]
 */
void ex_encounter_update_command(ex_encounter_t *e, float dt);

/**
 * [ex_encounter_targets fill a target array with live enemies; returns count]
 */
int ex_encounter_targets(const ex_encounter_t *e, ex_target_t *out, int max);

/**
 * [ex_encounter_by_id find an enemy by id (nullable)]
 */
ex_enemy_t* ex_encounter_by_id(ex_encounter_t *e, int id);

int ex_encounter_alive(const ex_encounter_t *e);

/* [ex_encounter_spawn_boss add the apex Corrupted at the arena centre] */
int ex_encounter_boss(ex_encounter_t *e, float cx, float cz, float hp);
/* [ex_encounter_boss_ptr the live boss, or NULL] */
ex_enemy_t* ex_encounter_boss_ptr(ex_encounter_t *e);

#endif // EX_ENCOUNTER_H
