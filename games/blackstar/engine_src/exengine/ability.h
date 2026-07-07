/* ability
  Equippable abilities the player customizes into loadout slots. Each ability
  has a cooldown and an active duration, plus a passive/active effect the game
  reads: HOVER (temporary flight/low-gravity), TURRET (deploy a stationary
  auto-gun), SHIELD (a damage-absorbing bubble), DASH (a fast burst of speed),
  CLOAK (enemies lose track -- reduces their sight), OVERCLOCK (fire faster).
  Pure state machine (cooldown/active timers); the game applies the effects.
*/
#ifndef EX_ABILITY_H
#define EX_ABILITY_H

#include <stdint.h>

typedef enum {
  EX_ABIL_NONE = 0,
  EX_ABIL_HOVER,
  EX_ABIL_TURRET,
  EX_ABIL_SHIELD,
  EX_ABIL_DASH,
  EX_ABIL_CLOAK,
  EX_ABIL_OVERCLOCK,
  EX_ABIL_RESONANCE,   /* signature: a devouring shockwave pulse */
  EX_ABIL_PULSE,       /* directional pulse blast */
  EX_ABIL_BLINK,       /* short-range teleport */
  EX_ABIL_COUNT
} ex_ability_id;

typedef struct {
  ex_ability_id id;
  float cooldown;     // seconds to recharge
  float duration;     // seconds active (0 = instant)
  float cd_timer;     // counts down to ready
  float active_timer; // counts down while active
} ex_ability_t;

#define EX_ABILITY_SLOTS 3
typedef struct {
  ex_ability_t slots[EX_ABILITY_SLOTS];
  int count;
} ex_ability_loadout_t;

const char* ex_ability_name(ex_ability_id id);
void ex_ability_defaults(ex_ability_t *a, ex_ability_id id); // fill cd/duration

void ex_loadout_init(ex_ability_loadout_t *l);
int  ex_loadout_add(ex_ability_loadout_t *l, ex_ability_id id);
void ex_loadout_set(ex_ability_loadout_t *l, int slot, ex_ability_id id);

/* [ex_ability_ready can this slot be triggered now?] */
int  ex_ability_ready(const ex_ability_loadout_t *l, int slot);
/* [ex_ability_trigger fire a slot -> 1 if activated, starts timers] */
int  ex_ability_trigger(ex_ability_loadout_t *l, int slot);
/* [ex_ability_active is a slot currently in its active window?] */
int  ex_ability_active(const ex_ability_loadout_t *l, int slot);
/* [ex_ability_active_any is any slot with this id active?] */
int  ex_ability_active_any(const ex_ability_loadout_t *l, ex_ability_id id);
void ex_loadout_update(ex_ability_loadout_t *l, float dt);

#endif // EX_ABILITY_H
