#include "commander_ai.h"
#include <string.h>
#include <math.h>

/* ---- archetype <-> name (matches commanders.json) ---- */
const char* ex_cmd_archetype_name(ex_cmd_archetype a) {
  switch (a) {
    case EX_CMD_AGGRESSOR:  return "aggressor";
    case EX_CMD_SURROUNDER: return "surrounder";
    case EX_CMD_SKIRMISHER: return "skirmisher";
    case EX_CMD_BAITER:     return "baiter";
    case EX_CMD_WARDEN:     return "warden";
    default:                return "aggressor";
  }
}
ex_cmd_archetype ex_cmd_archetype_from_name(const char *s) {
  if (!s) return EX_CMD_AGGRESSOR;
  for (int a = 0; a < EX_CMD_ARCHETYPE_COUNT; a++)
    if (strcmp(s, ex_cmd_archetype_name((ex_cmd_archetype)a)) == 0) return (ex_cmd_archetype)a;
  return EX_CMD_AGGRESSOR;
}

/* built-in doctrine params (mirror content/commanders.json). */
ex_cmd_doctrine_t ex_cmd_doctrine_for(ex_cmd_archetype a) {
  switch (a) {
    case EX_CMD_AGGRESSOR:  return (ex_cmd_doctrine_t){ 0.85f, 0.15f, 0.3f, 0.2f };
    case EX_CMD_SURROUNDER: return (ex_cmd_doctrine_t){ 0.60f, 0.30f, 0.9f, 0.4f };
    case EX_CMD_SKIRMISHER: return (ex_cmd_doctrine_t){ 0.45f, 0.75f, 0.5f, 0.6f };
    case EX_CMD_BAITER:     return (ex_cmd_doctrine_t){ 0.55f, 0.50f, 0.6f, 0.5f };
    case EX_CMD_WARDEN:     return (ex_cmd_doctrine_t){ 0.30f, 0.80f, 0.2f, 0.85f };
    default:                return (ex_cmd_doctrine_t){ 0.50f, 0.50f, 0.5f, 0.5f };
  }
}

void ex_commander_init(ex_commander_t *c, ex_cmd_archetype a, int squad_id) {
  if (!c) return;
  memset(c, 0, sizeof(*c));
  c->archetype = a;
  c->doctrine = ex_cmd_doctrine_for(a);
  c->state = EX_CMD_STATE_PROBE;
  c->squad_id = squad_id;
  c->think_cd = 0.0f;
}

/* survey this commander's own squad in the snapshot. */
typedef struct {
  int   alive;         /* living troops in the squad */
  int   total;         /* total troops seen for the squad */
  float hp_frac;       /* squad average hp fraction (0 if none) */
  float centroid[3];   /* mean position of living troops */
  int   commander_alive;
} squad_survey;

static squad_survey survey_squad(const ex_cj_snapshot_t *snap, int squad_id) {
  squad_survey s; memset(&s, 0, sizeof(s));
  float hp_acc = 0.0f;
  for (int i = 0; i < snap->n_units; i++) {
    const ex_cj_unit_t *u = &snap->units[i];
    if (u->id == squad_id && u->is_commander) {
      s.commander_alive = u->alive;
      continue;
    }
    if (u->squad != squad_id) continue;
    s.total++;
    if (u->alive) {
      s.alive++;
      s.centroid[0] += u->pos[0]; s.centroid[1] += u->pos[1]; s.centroid[2] += u->pos[2];
      if (u->max_hp > 0.0f) hp_acc += u->hp / u->max_hp;
    }
  }
  if (s.alive > 0) {
    s.centroid[0] /= s.alive; s.centroid[1] /= s.alive; s.centroid[2] /= s.alive;
    s.hp_frac = hp_acc / (float)s.alive;
  }
  return s;
}

/* choose the FSM posture from doctrine + battlefield. Deterministic policy that
 * the LLM layer will later replace with nuance -- same inputs, same command
 * outputs. */
static ex_cmd_state choose_state(const ex_commander_t *c, const squad_survey *sq,
                                 const ex_cj_snapshot_t *snap) {
  const ex_cmd_doctrine_t *d = &c->doctrine;
  float squad_hurt = 1.0f - sq->hp_frac;                 /* 0 fresh .. 1 wrecked */
  float losses = (sq->total > 0) ? 1.0f - (float)sq->alive/(float)sq->total : 0.0f;
  float player_press = snap->signals.aggression;         /* how hard the player pushes */

  /* fall back when hurt/thinned beyond the doctrine's caution tolerance. */
  if ((squad_hurt * 0.6f + losses * 0.4f) > (1.0f - d->caution)) {
    /* cautious doctrines rally; reckless ones just keep pressing */
    return (d->regroup_bias > 0.5f) ? EX_CMD_STATE_RALLY : EX_CMD_STATE_FALLBACK;
  }
  /* encircle when we have the numbers and the doctrine likes it. */
  if (sq->alive >= 3 && d->encircle_bias > 0.6f)
    return EX_CMD_STATE_ENCIRCLE;
  /* baiter special: if the player is pressing hard, spring -> press. */
  if (c->archetype == EX_CMD_BAITER && player_press > 0.6f)
    return EX_CMD_STATE_PRESS;
  /* press when aggressive; otherwise probe/hold. */
  if (d->aggression > 0.6f)
    return EX_CMD_STATE_PRESS;
  return EX_CMD_STATE_PROBE;
}

/* emit the command(s) for a posture. */
static int emit_for_state(ex_cmd_state st, const ex_cj_snapshot_t *snap,
                          const squad_survey *sq, ex_cj_command_t *out, int max) {
  if (max < 1) return 0;
  ex_cj_command_t *c = &out[0];
  memset(c, 0, sizeof(*c));
  c->zone = -1;   /* whole squad; caller may narrow by zone */
  /* aim at the player's position by default. */
  c->point[0] = snap->player_pos[0];
  c->point[1] = snap->player_pos[1];
  c->point[2] = snap->player_pos[2];

  switch (st) {
    case EX_CMD_STATE_PRESS:
      c->type = EX_CJ_CMD_FOCUS;    c->param0 = 0.9f; break;
    case EX_CMD_STATE_ENCIRCLE:
      c->type = EX_CJ_CMD_FLOCK_SPREAD; c->param0 = 10.0f; break;  /* radius */
    case EX_CMD_STATE_FALLBACK:
      c->type = EX_CJ_CMD_RETREAT;  c->param0 = 0.8f; break;
    case EX_CMD_STATE_RALLY:
      /* rally toward the squad centroid, not the player. */
      c->type = EX_CJ_CMD_FLOCK_TO;
      c->point[0] = sq->centroid[0]; c->point[1] = sq->centroid[1]; c->point[2] = sq->centroid[2];
      c->param0 = 6.0f; break;
    case EX_CMD_STATE_PROBE:
    default:
      c->type = EX_CJ_CMD_HOLD; break;
  }
  return 1;
}

int ex_commander_think(ex_commander_t *c, const ex_cj_snapshot_t *snap,
                       float dt, ex_cj_command_t *out, int max_cmds) {
  if (!c || !snap || !out || max_cmds < 1) return 0;
  c->state_t += dt;
  c->think_cd -= dt;
  if (c->think_cd > 0.0f) return 0;      /* only decide on the cadence */
  c->think_cd = 0.6f;                    /* ~1.6 decisions/sec */

  squad_survey sq = survey_squad(snap, c->squad_id);
  /* if our commander is gone or the squad is wiped, nothing to command. */
  if (!sq.commander_alive || sq.alive <= 0) return 0;

  ex_cmd_state st = choose_state(c, &sq, snap);
  if (st != c->state) { c->state = st; c->state_t = 0.0f; }
  return emit_for_state(st, snap, &sq, out, max_cmds);
}
