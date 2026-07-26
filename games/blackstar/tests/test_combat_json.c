/* test_combat_json -- the JSON boundary for (future) LLM combat integration.
 * Guards that the game->JSON->brain->JSON->game loop is completable: snapshots
 * serialize the signals a brain needs, and orders parse back robustly even from
 * a messy/chatty reply. */
#include "combat_json.h"
#include <stdio.h>
#include <string.h>
#include <math.h>
#include <assert.h>

static int has(const char *hay, const char *needle) { return strstr(hay, needle) != NULL; }

/* ---- snapshot serialization carries the state a brain reacts to ---- */
static void test_snapshot(void) {
  ex_cj_snapshot_t s;
  memset(&s, 0, sizeof(s));
  s.tick = 42;
  s.player_pos[0] = 10.0f; s.player_pos[2] = -5.0f;
  s.in_vehicle = 1;
  s.signals.aggression = 0.8f;
  s.signals.recent_dmg = 12.5f;
  s.signals.pressure = 0.4f;
  s.signals.kills = 7;
  s.signals.enemies_alive = 5;
  s.signals.player_hp_frac = 0.65f;
  s.n_allies = 1;
  s.allies[0].id = 900; s.allies[0].hp = 80; s.allies[0].max_hp = 100;
  s.allies[0].mode = 1; s.allies[0].target = 103;
  s.n_units = 2;
  s.units[0].id = 100; s.units[0].zone = 0; s.units[0].kind = 0; s.units[0].alive = 1;
  s.units[0].hp = 50; s.units[0].max_hp = 100;
  s.units[1].id = 101; s.units[1].zone = 2; s.units[1].kind = 3; s.units[1].alive = 1;
  s.units[1].hp = 900; s.units[1].max_hp = 1600;

  char buf[EX_CJ_JSON_MAX];
  int n = ex_cj_write_snapshot(&s, buf, sizeof(buf));
  assert(n > 0);
  /* the engagement signals + zones + ally mode must be present -- these are the
     things a roleplaying brain keys off. */
  assert(has(buf, "\"tick\":42"));
  assert(has(buf, "\"aggression\":0.800"));
  assert(has(buf, "\"kills\":7"));
  assert(has(buf, "\"in_vehicle\":1"));
  assert(has(buf, "\"zone\":0"));
  assert(has(buf, "\"zone\":2"));
  assert(has(buf, "\"mode\":1"));      /* ally engaging */
  assert(has(buf, "\"id\":900"));      /* ally id */
  printf("test_combat_json: snapshot carries signals + zones + allies\n");
}

/* ---- orders round-trip ---- */
static void test_orders_roundtrip(void) {
  ex_cj_orders_t o;
  memset(&o, 0, sizeof(o));
  o.tick = 42;
  strcpy(o.speaker, "blackstar");
  o.n_cmds = 2;
  o.cmds[0].type = EX_CJ_CMD_FLOCK_TO; o.cmds[0].zone = 1;
  o.cmds[0].point[0] = 20; o.cmds[0].point[2] = 30; o.cmds[0].param0 = 0.5f;
  o.cmds[1].type = EX_CJ_CMD_ALLY_FOLLOW; o.cmds[1].zone = -1; o.cmds[1].param0 = 1.0f;

  char buf[EX_CJ_JSON_MAX];
  int n = ex_cj_write_orders(&o, buf, sizeof(buf));
  assert(n > 0);
  assert(has(buf, "flock_to"));
  assert(has(buf, "ally_follow"));
  assert(has(buf, "blackstar"));

  ex_cj_orders_t back;
  int nc = ex_cj_read_orders(buf, &back);
  assert(nc == 2);
  assert(back.tick == 42);
  assert(strcmp(back.speaker, "blackstar") == 0);
  assert(back.cmds[0].type == EX_CJ_CMD_FLOCK_TO && back.cmds[0].zone == 1);
  assert(fabsf(back.cmds[0].point[0] - 20.0f) < 1e-3f);
  assert(back.cmds[1].type == EX_CJ_CMD_ALLY_FOLLOW && back.cmds[1].zone == -1);
  printf("test_combat_json: orders round-trip (2 cmds, fields intact)\n");
}

/* ---- the important one: a MESSY, chatty LLM-style reply still decodes ---- */
static void test_tolerant_parse(void) {
  /* prose before + after, unknown keys, odd whitespace, a reasoning field --
     exactly what a real model tends to emit. */
  const char *reply =
    "Blackstar considers the guardian's reckless push...\n"
    "{\n"
    "  \"speaker\": \"blackstar\",\n"
    "  \"reasoning\": \"the fool over-commits; encircle and punish\",\n"
    "  \"tick\": 99,\n"
    "  \"mood\": \"amused\",\n"
    "  \"commands\": [\n"
    "     { \"type\": \"flock_spread\", \"zone\": 0, \"point\": [12.5, 0, -8], \"radius\": 9, \"param0\": 0.9 },\n"
    "     { \"type\": \"focus\", \"zone\": 2, \"urgency\": \"high\", \"param1\": 0.7 },\n"
    "     { \"type\": \"teleport_everyone\", \"zone\": 1 }\n"   /* unknown verb -> skipped */
    "  ]\n"
    "}\n"
    "That should teach them.";

  ex_cj_orders_t o;
  int nc = ex_cj_read_orders(reply, &o);
  assert(nc == 2);                              /* unknown verb dropped, 2 valid */
  assert(o.tick == 99);
  assert(strcmp(o.speaker, "blackstar") == 0);
  assert(o.cmds[0].type == EX_CJ_CMD_FLOCK_SPREAD);
  assert(o.cmds[0].zone == 0);
  assert(fabsf(o.cmds[0].point[0] - 12.5f) < 1e-3f);
  assert(fabsf(o.cmds[0].param0 - 0.9f) < 1e-3f);
  /* field windowing: cmd[1]'s zone is 2 and param1 is 0.7, NOT bleeding from
     cmd[0] or the dropped cmd. */
  assert(o.cmds[1].type == EX_CJ_CMD_FOCUS);
  assert(o.cmds[1].zone == 2);
  assert(fabsf(o.cmds[1].param1 - 0.7f) < 1e-3f);
  assert(o.cmds[1].param0 == 0.0f);             /* not set -> default */
  printf("test_combat_json: tolerant parse of a chatty LLM reply (2 of 3, windowed)\n");
}

/* ---- observe-only reply (no commands) is valid ---- */
static void test_observe_only(void) {
  const char *reply = "{\"speaker\":\"blackstar\",\"reasoning\":\"watch, for now\"}";
  ex_cj_orders_t o;
  int nc = ex_cj_read_orders(reply, &o);
  assert(nc == 0);                              /* valid: zero commands */
  assert(strcmp(o.speaker, "blackstar") == 0);
  printf("test_combat_json: observe-only reply decodes to 0 commands\n");
}

/* ---- verb name mapping is stable both ways ---- */
static void test_verb_names(void) {
  for (int t = 1; t < EX_CJ_CMD_COUNT; t++) {
    const char *nm = ex_cj_cmd_name((ex_cj_cmd_type)t);
    assert(nm && nm[0]);
    assert(ex_cj_cmd_from_name(nm) == (ex_cj_cmd_type)t);   /* round-trips */
  }
  assert(ex_cj_cmd_from_name("nonsense") == EX_CJ_CMD_NONE);
  printf("test_combat_json: verb names round-trip\n");
}

/* ---- live integration: signal tracker + snapshot builder ---- */
static void test_live_build(void) {
  /* signal tracker climbs under sustained fire + closing */
  ex_cj_signal_tracker_t st; ex_cj_signals_reset(&st);
  for (int i = 0; i < 120; i++) ex_cj_signals_tick(&st, 1, 12.0f, 3, 1.0f/60.0f);
  assert(st.aggression > 0.5f);      /* pushing hard */
  assert(st.pressure > 0.4f);        /* somewhat surrounded */
  /* and falls when passive */
  for (int i = 0; i < 240; i++) ex_cj_signals_tick(&st, 0, 0.0f, 0, 1.0f/60.0f);
  assert(st.aggression < 0.2f);
  assert(st.pressure < 0.2f);

  /* build a snapshot from parallel arrays */
  int   ids[3]   = {100,101,102};
  float pos[3][3]= {{5,0,0},{10,0,0},{40,0,0}};
  float hp[3]    = {50,20,900};
  float mhp[3]   = {100,100,1600};
  int   zone[3]  = {0,1,2};
  int   kind[3]  = {0,0,3};
  int   elem[3]  = {1,1,4};
  int   alive[3] = {1,1,0};           /* one dead */
  int   aid[1]   = {900};
  float apos[1][3]={{3,0,3}};
  float ahp[1]={80}, amhp[1]={100};
  int   amode[1]={1}, atgt[1]={100};
  float ppos[3] = {0,1,0};

  ex_cj_snapshot_t snap;
  ex_cj_build_snapshot(&snap, 7, ppos, 1, &st, 4, 0.6f,
                       3, ids, pos, hp, mhp, zone, kind, elem, alive,
                       NULL, NULL, NULL,
                       1, aid, apos, ahp, amhp, amode, atgt);
  assert(snap.n_units == 3);
  assert(snap.n_allies == 1);
  assert(snap.signals.enemies_alive == 2);   /* one was dead */
  assert(snap.signals.kills == 4);
  assert(snap.units[2].zone == 2 && snap.units[2].kind == 3);

  char buf[EX_CJ_JSON_MAX];
  int n = ex_cj_write_snapshot(&snap, buf, sizeof(buf));
  assert(n > 0 && has(buf, "\"enemies_alive\":2"));
  printf("test_combat_json: live tracker + snapshot builder (loop closeable)\n");
}

int main(void) {
  test_snapshot();
  test_orders_roundtrip();
  test_tolerant_parse();
  test_observe_only();
  test_verb_names();
  test_live_build();
  printf("ALL ASSERTIONS PASSED\n");
  return 0;
}
