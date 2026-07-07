/* element
  Elemental combat system, inspired by original worldbuilding: a coalition
  of elemental races (Ice, Fire, Air, Electricity, Vine, Rock, Water) and a
  "Watch"-style device that lets one warrior embody any element mid-battle to
  turn a fight. Names/lore here are original; only the *mechanic* is borrowed.

  Two things live here:
    1. Elements with a rock-paper-scissors advantage graph + per-element combat
       modifiers (damage, projectile speed, range, cooldown), so switching
       element is a real tactical choice, not a reskin.
    2. The Watch: cycles a character through elements (like the transform
       state machine, but for element instead of robot/vehicle mode), with a
       short attune delay so you can't hot-swap for free every shot.

  Designed to layer onto the existing combat + transform modules: a character's
  active element scales the hitscan damage and range from combat.h.
*/

#ifndef EX_ELEMENT_H
#define EX_ELEMENT_H

#include <stdint.h>

typedef enum {
  EX_ELEM_NONE = 0,
  EX_ELEM_ICE,
  EX_ELEM_FIRE,
  EX_ELEM_AIR,
  EX_ELEM_ELECTRIC,
  EX_ELEM_VINE,
  EX_ELEM_ROCK,
  EX_ELEM_WATER,
  EX_ELEM_COUNT
} ex_element_t;

// per-element combat profile -- switching element changes how you fight
typedef struct {
  float damage_mul;      // hitscan damage multiplier
  float range_mul;       // effective range multiplier
  float cooldown_mul;    // fire-cooldown multiplier (lower = faster)
  float move_mul;        // movement multiplier
} ex_element_profile_t;

/**
 * [ex_element_profile combat modifiers for an element]
 * Rough identities (tunable):
 *   Ice      - control: slower, longer range, steady
 *   Fire     - burst: high damage, short range
 *   Air      - mobility: fast move + fast cooldown, low damage
 *   Electric - reach: long range, chain-friendly, medium damage
 *   Vine     - sustain: medium everything, snares (move penalty to target)
 *   Rock     - tank: high damage, slow, short range
 *   Water    - flexible: balanced, slight range
 */
ex_element_profile_t ex_element_profile(ex_element_t e);

/**
 * [ex_element_advantage damage multiplier when `attacker` hits `defender`]
 * Returns 1.5 for a strong matchup, 0.66 for a weak one, 1.0 otherwise.
 * The cycle (each beats the next two, loses to the previous two) gives every
 * element two favourable and two unfavourable matchups.
 */
float ex_element_advantage(ex_element_t attacker, ex_element_t defender);

/**
 * [ex_element_name human-readable element label]
 */
const char* ex_element_name(ex_element_t e);

// ---- The Watch: cycles a character through elements ----

#define EX_WATCH_ATTUNE_SEC 0.5f  // delay after switching before the new element is active

typedef struct {
  ex_element_t current;
  ex_element_t pending;    // element being attuned to (during the delay)
  float attune_timer;      // counts down while switching
  int   attuning;
} ex_watch_t;

/**
 * [ex_watch_new create a Watch starting on an element]
 */
ex_watch_t ex_watch_new(ex_element_t start);

/**
 * [ex_watch_switch begin attuning to a new element (denied if mid-attune)]
 * @return [1 if the switch began, 0 if denied]
 */
int ex_watch_switch(ex_watch_t *w, ex_element_t to);

/**
 * [ex_watch_cycle convenience: switch to the next element in order]
 */
int ex_watch_cycle(ex_watch_t *w);

/**
 * [ex_watch_update advance the attune timer; commits the pending element]
 */
void ex_watch_update(ex_watch_t *w, float dt);

/**
 * [ex_watch_active the element currently in effect (NONE while attuning)]
 */
static inline ex_element_t ex_watch_active(const ex_watch_t *w) {
  return w->attuning ? EX_ELEM_NONE : w->current;
}

#endif // EX_ELEMENT_H
