/* commander_ai -- deterministic doctrine FSMs for squad commanders.
 *
 * A commander is a "brain" that reads the battlefield and issues squad orders.
 * The KEY design choice: a commander FSM emits the SAME ex_cj_command_t that the
 * (future) Blackstar/Prime LLM will emit. So an FSM commander and an LLM
 * commander are interchangeable behind one interface -- the deterministic layer
 * is a strict subset of what the LLM layer can express, and both speak the one
 * command vocabulary (flock_to/flock_spread/hold/focus/retreat/ally_follow).
 *
 * Each archetype (see content/commanders.json) is a distilled military doctrine:
 * a small parameter set (aggression/caution/encircle/regroup) + an FSM policy
 * mapping battlefield conditions -> commands. This is the doctrine corpus turned
 * into behavior, not a quote dump.
 */
#ifndef EX_COMMANDER_AI_H
#define EX_COMMANDER_AI_H

#include "combat_json.h"

/* archetypes -- must match the keys in content/commanders.json */
typedef enum {
  EX_CMD_AGGRESSOR = 0,  /* concentrate + press */
  EX_CMD_SURROUNDER,     /* encircle with numbers */
  EX_CMD_SKIRMISHER,     /* hit and run, evade */
  EX_CMD_BAITER,         /* feign weakness, then collapse */
  EX_CMD_WARDEN,         /* hold the line with discipline */
  EX_CMD_ARCHETYPE_COUNT
} ex_cmd_archetype;

/* the FSM's coarse posture -- what the commander is currently doing. */
typedef enum {
  EX_CMD_STATE_PROBE = 0, /* feeling out the player, holding */
  EX_CMD_STATE_PRESS,     /* committing/pressing the attack */
  EX_CMD_STATE_ENCIRCLE,  /* spreading to surround */
  EX_CMD_STATE_FALLBACK,  /* retreating/harassing */
  EX_CMD_STATE_RALLY      /* regrouping the squad */
} ex_cmd_state;

/* the doctrine params (loaded from the archetype). */
typedef struct {
  float aggression;    /* 0..1 -- willingness to commit/press */
  float caution;       /* 0..1 -- willingness to fall back when hurt */
  float encircle_bias; /* 0..1 -- preference to spread + surround */
  float regroup_bias;  /* 0..1 -- preference to rally over pressing */
} ex_cmd_doctrine_t;

/* per-commander runtime brain. */
typedef struct {
  ex_cmd_archetype  archetype;
  ex_cmd_doctrine_t doctrine;
  ex_cmd_state      state;
  int               squad_id;     /* the commander enemy id it speaks for */
  float             think_cd;     /* seconds until the next decision */
  float             state_t;      /* time in the current state */
} ex_commander_t;

/* archetype <-> name (matches commanders.json keys) */
const char*       ex_cmd_archetype_name(ex_cmd_archetype a);
ex_cmd_archetype  ex_cmd_archetype_from_name(const char *s);

/* the built-in doctrine params for an archetype (mirrors commanders.json so the
 * engine has a sane default even without the content file loaded). */
ex_cmd_doctrine_t ex_cmd_doctrine_for(ex_cmd_archetype a);

/* init a commander brain with an archetype + the squad id it commands. */
void ex_commander_init(ex_commander_t *c, ex_cmd_archetype a, int squad_id);

/* One decision step. Reads the snapshot (its own squad's units + the engagement
 * signals), advances the FSM, and writes up to `max_cmds` commands into `out`.
 * Returns the number of commands written (0 = observe/hold this tick). Only
 * fires on the think cadence, so calling every frame is fine. */
int ex_commander_think(ex_commander_t *c, const ex_cj_snapshot_t *snap,
                       float dt, ex_cj_command_t *out, int max_cmds);

#endif /* EX_COMMANDER_AI_H */
