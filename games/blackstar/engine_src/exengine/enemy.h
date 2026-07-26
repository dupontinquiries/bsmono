/* enemy
  A minimal enemy agent, ported in spirit from early-fps's enemy.rs:
  a tiny state machine idle -> chase -> attack -> dead. It chases the
  player on the XZ plane, only "sees" the player when it has line of
  sight (no solid chunk between them), and dies when its HP hits zero.

  Combat treats each enemy as a Target sphere (see combat.h). Rendering
  is the caller's job -- this module owns position, HP, and AI state so
  it's fully testable headless.
*/

#ifndef EX_ENEMY_H
#define EX_ENEMY_H

#include "math/mathlib.h"
#include "coll_chunk.h"
#include "combat.h"
#include "element.h"
#include <stdint.h>

typedef enum {
  EX_ENEMY_IDLE,
  EX_ENEMY_CHASE,
  EX_ENEMY_ATTACK,
  EX_ENEMY_DEAD
} ex_enemy_state;

// enemy archetype -- Grunt fights at range; Corrupted is horror: no LOS gate,
// no ranged attack, just relentlessly closes distance (from the Dominion
// "it does not speak, it only closes the distance" design).
typedef enum {
  EX_ENEMY_GRUNT,
  EX_ENEMY_CORRUPTED,
  EX_ENEMY_HIJACK,   // Trilogy Two swarm: erratic, fast, fragile, never flees
  EX_ENEMY_APEX      // the apex Corrupted -- Jhiaxus's masterpiece, boss
} ex_enemy_kind;

typedef struct {
  int   id;
  ex_enemy_kind kind;
  ex_element_t  element;   // for elemental advantage vs the player
  vec3  position;
  float radius;
  float hp, max_hp;
  ex_enemy_state state;

  float move_speed;
  float sight_range;    // won't notice the player beyond this
  float attack_range;   // switches to ATTACK inside this
  float fire_cooldown;  // seconds between shots
  float fire_timer;     // counts down

  int   wants_to_fire;  // set true on the frame a shot should be spawned

  // --- Destiny "owned space": which line this enemy holds and how far it
  //     stands off. Front (0) pushes close, middle (1) holds mid, back (2)
  //     holds far (anchors/snipers). Enemies HOLD their zone at standoff
  //     range rather than all swarming, and advance a line when their zone
  //     collapses (enough of that line has been cleared). ---
  int   zone;           // 0 front, 1 middle, 2 back
  float standoff;       // preferred distance from the player to hold at

  // --- Corrupted infection (from the Jhiaxus/Corrupted design) ---
  int   stage;          // 0 (n/a) or 1..3: wrong-geometry -> mid -> full
  float stage_timer;    // time in current stage; escalates over time
  float grow;           // Stage 3 absorbs metal -> grows (scales radius/hp/dmg)
  int   transmits;      // 1 if melee transmits corruption to the player

  // --- apex boss ---
  int   phase;          // 1..3, gated by remaining HP
  float slam_timer;     // counts down to a ground-slam AoE
  float summon_timer;   // counts down to summoning adds
  int   wants_slam;     // set the frame a slam should resolve
  int   wants_summon;   // set the frame adds should spawn

  // --- commander / squad cohesion ---
  //   A COMMANDER coordinates a squad: while it lives its troops hold their
  //   zones with discipline. Kill it and the squad SCATTERS -- they break
  //   formation, panic-flee for scatter_t seconds, then fight on demoralized
  //   (wider standoff, worse cadence) unless another commander rallies them.
  int   is_commander;   // 1 if this enemy leads a squad
  int   squad;          // leader id this troop answers to (-1 = independent)
  float scatter_t;      // >0: leaderless panic countdown (breaks zone discipline)
  int   demoralized;    // 1 after scattering with no commander left
} ex_enemy_t;

/**
 * [ex_enemy_new spawn a ranged Grunt at a position]
 */
ex_enemy_t ex_enemy_new(int id, vec3 pos, float hp);

/**
 * [ex_enemy_new_kind spawn a specific archetype + element]
 */
ex_enemy_t ex_enemy_new_kind(int id, vec3 pos, float hp, ex_enemy_kind kind, ex_element_t element);

/**
 * [ex_enemy_as_target build a combat Target sphere for this enemy]
 */
ex_target_t ex_enemy_as_target(const ex_enemy_t *e);

/**
 * [ex_enemy_has_los line of sight to a point: true if no solid chunk blocks it]
 */
int ex_enemy_has_los(const ex_enemy_t *e, ex_chunk_world_t *chunks, vec3 target);

/**
 * [ex_enemy_damage apply damage; transitions to DEAD at <=0 hp. Returns 1 if it just died.]
 */
int ex_enemy_damage(ex_enemy_t *e, float amount);

/**
 * [ex_enemy_update advance the state machine one tick toward the player]
 * @param e          [the enemy]
 * @param chunks     [world (for LOS + simple avoidance)]
 * @param player_pos [where the player is]
 * @param dt         [delta time]
 * Sets e->wants_to_fire = 1 on ticks where it should loose a shot at the player.
 */
void ex_enemy_update(ex_enemy_t *e, ex_chunk_world_t *chunks, vec3 player_pos, float dt);

const char* ex_enemy_state_name(ex_enemy_state s);

/* [ex_enemy_stage_name label for a Corrupted stage (1..3)] */
const char* ex_enemy_stage_name(int stage);
/* [ex_enemy_melee_damage corrupted melee damage, scaled by stage + growth] */
float ex_enemy_melee_damage(const ex_enemy_t *e);
/* [ex_enemy_visual_radius effective radius including Stage-3 growth] */
float ex_enemy_visual_radius(const ex_enemy_t *e);

/* [ex_enemy_set_cloak while set, grunts/hijacks cannot acquire the player;
 * Corrupted/Apex ignore it -- they hunt by spark resonance, not sight] */
void ex_enemy_set_cloak(int on);

#endif // EX_ENEMY_H
