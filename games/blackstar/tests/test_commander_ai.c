/* test_commander_ai -- deterministic doctrine FSMs for squad commanders.
 * Guards that each archetype yields distinct, sensible combat_json commands and
 * that the FSM->command path (which the LLM layer will later share) works. */
#include "commander_ai.h"
#include "combat_json.h"
#include <stdio.h>
#include <string.h>
#include <math.h>
#include <assert.h>

/* build a snapshot: a commander (id=500) + n troops in a squad, at given hp
 * fraction, with a player-aggression signal. */
static void make_snap(ex_cj_snapshot_t *s, int squad_id, int n_alive, int n_total,
                      float troop_hp_frac, float player_aggr) {
  memset(s, 0, sizeof(*s));
  s->tick = 1;
  s->player_pos[0] = 0; s->player_pos[2] = 0;
  s->signals.aggression = player_aggr;
  /* commander unit */
  ex_cj_unit_t *cu = &s->units[s->n_units++];
  cu->id = squad_id; cu->is_commander = 1; cu->squad = -1; cu->alive = 1;
  cu->pos[0] = 20; cu->hp = 400; cu->max_hp = 400;
  /* troops */
  for (int i = 0; i < n_total; i++) {
    ex_cj_unit_t *u = &s->units[s->n_units++];
    u->id = 100 + i; u->squad = squad_id;
    u->alive = (i < n_alive) ? 1 : 0;
    u->max_hp = 100; u->hp = 100 * troop_hp_frac;
    u->pos[0] = 18 + i; u->pos[2] = (float)(i - n_total/2);
  }
}

/* run one decision (skip the think cadence by passing a big dt). */
static int decide(ex_commander_t *c, const ex_cj_snapshot_t *s, ex_cj_command_t *out) {
  return ex_commander_think(c, s, 10.0f, out, 4);
}

static void test_archetypes_differ(void) {
  ex_cj_snapshot_t s;
  ex_cj_command_t cmd[4];

  /* fresh, healthy squad of 5, player pushing moderately */
  make_snap(&s, 500, 5, 5, 1.0f, 0.5f);

  /* AGGRESSOR -> presses (focus) */
  ex_commander_t ag; ex_commander_init(&ag, EX_CMD_AGGRESSOR, 500);
  int n = decide(&ag, &s, cmd);
  assert(n == 1 && cmd[0].type == EX_CJ_CMD_FOCUS);
  printf("test_commander_ai: aggressor -> %s\n", ex_cj_cmd_name(cmd[0].type));

  /* SURROUNDER with numbers -> encircle (spread) */
  ex_commander_t su; ex_commander_init(&su, EX_CMD_SURROUNDER, 500);
  n = decide(&su, &s, cmd);
  assert(n == 1 && cmd[0].type == EX_CJ_CMD_FLOCK_SPREAD);
  printf("test_commander_ai: surrounder -> %s\n", ex_cj_cmd_name(cmd[0].type));

  /* WARDEN fresh -> holds (probe), not pressing */
  ex_commander_t wa; ex_commander_init(&wa, EX_CMD_WARDEN, 500);
  n = decide(&wa, &s, cmd);
  assert(n == 1 && cmd[0].type == EX_CJ_CMD_HOLD);
  printf("test_commander_ai: warden (fresh) -> %s\n", ex_cj_cmd_name(cmd[0].type));
}

static void test_falls_back_when_hurt(void) {
  ex_cj_snapshot_t s;
  ex_cj_command_t cmd[4];
  /* squad thinned to 1 of 5 and low hp -> cautious doctrines disengage */
  make_snap(&s, 500, 1, 5, 0.2f, 0.5f);

  ex_commander_t sk; ex_commander_init(&sk, EX_CMD_SKIRMISHER, 500);
  int n = decide(&sk, &s, cmd);
  assert(n == 1);
  /* skirmisher rallies or retreats -- either is a disengage, not a press */
  assert(cmd[0].type == EX_CJ_CMD_RETREAT || cmd[0].type == EX_CJ_CMD_FLOCK_TO);
  assert(cmd[0].type != EX_CJ_CMD_FOCUS);
  printf("test_commander_ai: hurt skirmisher -> %s (disengages)\n", ex_cj_cmd_name(cmd[0].type));

  /* warden hurt -> rallies (regroup_bias high) */
  ex_commander_t wa; ex_commander_init(&wa, EX_CMD_WARDEN, 500);
  n = decide(&wa, &s, cmd);
  assert(n == 1 && cmd[0].type == EX_CJ_CMD_FLOCK_TO);   /* rally to centroid */
  printf("test_commander_ai: hurt warden -> rally\n");
}

static void test_baiter_springs(void) {
  ex_cj_snapshot_t s;
  ex_cj_command_t cmd[4];
  ex_commander_t ba; ex_commander_init(&ba, EX_CMD_BAITER, 500);

  /* player calm -> baiter holds/probes (dangling the bait) */
  make_snap(&s, 500, 4, 5, 0.9f, 0.2f);
  int n = decide(&ba, &s, cmd);
  assert(n == 1);
  ex_cj_cmd_type calm = cmd[0].type;

  /* player pressing hard -> baiter springs to a focus press */
  make_snap(&s, 500, 4, 5, 0.9f, 0.9f);
  ba.think_cd = 0.0f;                 /* allow immediate re-decide */
  n = decide(&ba, &s, cmd);
  assert(n == 1 && cmd[0].type == EX_CJ_CMD_FOCUS);
  printf("test_commander_ai: baiter calm->%s, pressed->focus (springs)\n",
         ex_cj_cmd_name(calm));
}

static void test_cadence_and_empty(void) {
  ex_cj_snapshot_t s;
  ex_cj_command_t cmd[4];
  make_snap(&s, 500, 5, 5, 1.0f, 0.5f);
  ex_commander_t ag; ex_commander_init(&ag, EX_CMD_AGGRESSOR, 500);

  /* first tick decides; an immediate tiny-dt tick should NOT (cadence gate) */
  int n1 = ex_commander_think(&ag, &s, 10.0f, cmd, 4);
  int n2 = ex_commander_think(&ag, &s, 0.001f, cmd, 4);
  assert(n1 == 1 && n2 == 0);
  printf("test_commander_ai: think cadence gates decisions\n");

  /* dead commander -> no orders */
  s.units[0].alive = 0;               /* kill the commander unit */
  ag.think_cd = 0.0f;
  int n3 = ex_commander_think(&ag, &s, 10.0f, cmd, 4);
  assert(n3 == 0);
  printf("test_commander_ai: dead commander issues no orders\n");
}

static void test_names(void) {
  for (int a = 0; a < EX_CMD_ARCHETYPE_COUNT; a++) {
    const char *nm = ex_cmd_archetype_name((ex_cmd_archetype)a);
    assert(nm && nm[0]);
    assert(ex_cmd_archetype_from_name(nm) == (ex_cmd_archetype)a);
  }
  printf("test_commander_ai: archetype names round-trip\n");
}

int main(void) {
  test_archetypes_differ();
  test_falls_back_when_hurt();
  test_baiter_springs();
  test_cadence_and_empty();
  test_names();
  printf("ALL ASSERTIONS PASSED\n");
  return 0;
}
