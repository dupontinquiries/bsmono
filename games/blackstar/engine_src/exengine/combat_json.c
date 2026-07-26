#include "combat_json.h"
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <stdarg.h>

/* ---- command verb <-> stable wire name ---- */
const char* ex_cj_cmd_name(ex_cj_cmd_type t) {
  switch (t) {
    case EX_CJ_CMD_FLOCK_TO:     return "flock_to";
    case EX_CJ_CMD_FLOCK_SPREAD: return "flock_spread";
    case EX_CJ_CMD_HOLD:         return "hold";
    case EX_CJ_CMD_FOCUS:        return "focus";
    case EX_CJ_CMD_RETREAT:      return "retreat";
    case EX_CJ_CMD_ALLY_FOLLOW:  return "ally_follow";
    default:                     return "none";
  }
}
ex_cj_cmd_type ex_cj_cmd_from_name(const char *s) {
  if (!s) return EX_CJ_CMD_NONE;
  for (int t = 1; t < EX_CJ_CMD_COUNT; t++)
    if (strcmp(s, ex_cj_cmd_name((ex_cj_cmd_type)t)) == 0) return (ex_cj_cmd_type)t;
  return EX_CJ_CMD_NONE;
}

/* ---- tiny append helper: writes into out[*off..cap], guards overflow ---- */
typedef struct { char *buf; size_t cap; size_t off; int ok; } wbuf;
static void wputs(wbuf *w, const char *s) {
  if (!w->ok) return;
  size_t n = strlen(s);
  if (w->off + n >= w->cap) { w->ok = 0; return; }
  memcpy(w->buf + w->off, s, n); w->off += n;
}
static void wfmt(wbuf *w, const char *fmt, ...) {
  if (!w->ok) return;
  char tmp[128]; va_list ap; va_start(ap, fmt);
  int n = vsnprintf(tmp, sizeof(tmp), fmt, ap); va_end(ap);
  if (n < 0) { w->ok = 0; return; }
  wputs(w, tmp);
}

/* ---- snapshot -> JSON ---- */
int ex_cj_write_snapshot(const ex_cj_snapshot_t *s, char *out, size_t cap) {
  wbuf w = { out, cap, 0, 1 };
  wfmt(&w, "{\"tick\":%d,", s->tick);
  wfmt(&w, "\"player\":{\"pos\":[%.2f,%.2f,%.2f],\"in_vehicle\":%d},",
       s->player_pos[0], s->player_pos[1], s->player_pos[2], s->in_vehicle ? 1 : 0);
  /* the engagement signals a roleplaying brain reacts to */
  wfmt(&w, "\"signals\":{\"aggression\":%.3f,\"recent_dmg\":%.2f,\"pressure\":%.3f,"
           "\"kills\":%d,\"enemies_alive\":%d,\"player_hp_frac\":%.3f},",
       s->signals.aggression, s->signals.recent_dmg, s->signals.pressure,
       s->signals.kills, s->signals.enemies_alive, s->signals.player_hp_frac);
  /* allies following the player */
  wputs(&w, "\"allies\":[");
  for (int i = 0; i < s->n_allies; i++) {
    const ex_cj_ally_t *a = &s->allies[i];
    wfmt(&w, "%s{\"id\":%d,\"pos\":[%.2f,%.2f,%.2f],\"hp\":%.1f,\"max_hp\":%.1f,\"mode\":%d,\"target\":%d}",
         i ? "," : "", a->id, a->pos[0], a->pos[1], a->pos[2], a->hp, a->max_hp, a->mode, a->target);
  }
  wputs(&w, "],");
  /* troops/enemies with their owned-space zone + mode */
  wputs(&w, "\"units\":[");
  for (int i = 0; i < s->n_units; i++) {
    const ex_cj_unit_t *u = &s->units[i];
    wfmt(&w, "%s{\"id\":%d,\"pos\":[%.2f,%.2f,%.2f],\"hp\":%.1f,\"max_hp\":%.1f,"
             "\"zone\":%d,\"kind\":%d,\"element\":%d,\"alive\":%d,"
             "\"is_commander\":%d,\"squad\":%d,\"scattering\":%d}",
         i ? "," : "", u->id, u->pos[0], u->pos[1], u->pos[2], u->hp, u->max_hp,
         u->zone, u->kind, u->element, u->alive ? 1 : 0,
         u->is_commander ? 1 : 0, u->squad, u->scattering ? 1 : 0);
  }
  wputs(&w, "]}");
  if (!w.ok) return -1;
  out[w.off] = '\0';
  return (int)w.off;
}

/* ---- orders -> JSON (round-trip / logging) ---- */
int ex_cj_write_orders(const ex_cj_orders_t *o, char *out, size_t cap) {
  wbuf w = { out, cap, 0, 1 };
  wfmt(&w, "{\"tick\":%d,\"speaker\":\"%s\",\"commands\":[", o->tick,
       o->speaker[0] ? o->speaker : "");
  for (int i = 0; i < o->n_cmds; i++) {
    const ex_cj_command_t *c = &o->cmds[i];
    wfmt(&w, "%s{\"type\":\"%s\",\"zone\":%d,\"point\":[%.2f,%.2f,%.2f],\"param0\":%.3f,\"param1\":%.3f}",
         i ? "," : "", ex_cj_cmd_name(c->type), c->zone,
         c->point[0], c->point[1], c->point[2], c->param0, c->param1);
  }
  wputs(&w, "]}");
  if (!w.ok) return -1;
  out[w.off] = '\0';
  return (int)w.off;
}

/* ---- live integration helpers ---- */

void ex_cj_signals_reset(ex_cj_signal_tracker_t *t) {
  if (t) memset(t, 0, sizeof(*t));
}

static float clamp01(float v) { return v < 0 ? 0 : (v > 1 ? 1 : v); }

void ex_cj_signals_tick(ex_cj_signal_tracker_t *t, int firing, float closing,
                        int near_count, float dt) {
  if (!t) return;
  /* aggression: fire activity + forward pressure toward the enemy, smoothed. */
  t->_fire_accum += (firing ? 1.0f : 0.0f) * dt * 3.0f;
  t->_fire_accum -= t->_fire_accum * dt * 1.2f;            /* decay */
  float closing_norm = clamp01(closing / 12.0f);          /* ~12 u/s = full press */
  float target_aggr = clamp01(clamp01(t->_fire_accum) * 0.6f + closing_norm * 0.4f);
  t->aggression += (target_aggr - t->aggression) * clamp01(dt * 4.0f);
  /* pressure: how surrounded, smoothed (4+ nearby = maxed). */
  float target_pres = clamp01((float)near_count / 4.0f);
  t->pressure += (target_pres - t->pressure) * clamp01(dt * 3.0f);
}

void ex_cj_build_snapshot(ex_cj_snapshot_t *snap, int tick,
                          const float player_pos[3], int in_vehicle,
                          const ex_cj_signal_tracker_t *sig,
                          int kills, float player_hp_frac,
                          int n_units, const int *ids, const float (*pos)[3],
                          const float *hp, const float *max_hp, const int *zone,
                          const int *kind, const int *element, const int *alive,
                          const int *is_commander, const int *squad, const int *scattering,
                          int n_allies, const int *ally_id, const float (*ally_pos)[3],
                          const float *ally_hp, const float *ally_max_hp,
                          const int *ally_mode, const int *ally_target) {
  if (!snap) return;
  memset(snap, 0, sizeof(*snap));
  snap->tick = tick;
  if (player_pos) { snap->player_pos[0]=player_pos[0]; snap->player_pos[1]=player_pos[1]; snap->player_pos[2]=player_pos[2]; }
  snap->in_vehicle = in_vehicle ? 1 : 0;
  snap->signals.aggression = sig ? sig->aggression : 0.0f;
  snap->signals.pressure   = sig ? sig->pressure   : 0.0f;
  snap->signals.kills = kills;
  snap->signals.player_hp_frac = player_hp_frac;

  int live = 0;
  int nu = n_units; if (nu > EX_CJ_MAX_UNITS) nu = EX_CJ_MAX_UNITS;
  for (int i = 0; i < nu; i++) {
    ex_cj_unit_t *u = &snap->units[snap->n_units++];
    u->id = ids ? ids[i] : i;
    if (pos) { u->pos[0]=pos[i][0]; u->pos[1]=pos[i][1]; u->pos[2]=pos[i][2]; }
    u->hp = hp ? hp[i] : 0.0f;
    u->max_hp = max_hp ? max_hp[i] : 0.0f;
    u->zone = zone ? zone[i] : -1;
    u->kind = kind ? kind[i] : 0;
    u->element = element ? element[i] : 0;
    u->alive = alive ? (alive[i] ? 1 : 0) : 1;
    u->is_commander = is_commander ? (is_commander[i] ? 1 : 0) : 0;
    u->squad = squad ? squad[i] : -1;
    u->scattering = scattering ? (scattering[i] ? 1 : 0) : 0;
    if (u->alive) live++;
  }
  snap->signals.enemies_alive = live;

  int na = n_allies; if (na > EX_CJ_MAX_ALLIES) na = EX_CJ_MAX_ALLIES;
  for (int i = 0; i < na; i++) {
    ex_cj_ally_t *a = &snap->allies[snap->n_allies++];
    a->id = ally_id ? ally_id[i] : (900+i);
    if (ally_pos) { a->pos[0]=ally_pos[i][0]; a->pos[1]=ally_pos[i][1]; a->pos[2]=ally_pos[i][2]; }
    a->hp = ally_hp ? ally_hp[i] : 0.0f;
    a->max_hp = ally_max_hp ? ally_max_hp[i] : 0.0f;
    a->mode = ally_mode ? ally_mode[i] : 0;
    a->target = ally_target ? ally_target[i] : -1;
  }
}

/* ---- tolerant orders parser ---------------------------------------------- *
 * We don't need a full JSON parser: we scan for the fields we care about. This
 * is deliberately forgiving so a chatty LLM reply (extra prose, unknown keys,
 * whitespace) still yields the commands. */

static const char* skip_ws(const char *p) {
  while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') p++;
  return p;
}
/* find the next occurrence of "key" as a JSON key (\"key\") from p, or NULL */
static const char* find_key(const char *p, const char *key) {
  char pat[64];
  snprintf(pat, sizeof(pat), "\"%s\"", key);
  return p ? strstr(p, pat) : NULL;
}
/* read a number appearing after a ':' following the key at `at`. Returns the
 * char after the number, writes value to *out. `at` must point at the key. */
static const char* read_num_after(const char *at, float *out) {
  const char *c = strchr(at, ':');
  if (!c) return NULL;
  c = skip_ws(c + 1);
  char *end = NULL;
  double v = strtod(c, &end);
  if (end == c) return NULL;
  *out = (float)v;
  return end;
}
/* read an int the same way */
static const char* read_int_after(const char *at, int *out) {
  float f; const char *e = read_num_after(at, &f);
  if (e) *out = (int)f;
  return e;
}
/* read a quoted string value after the key into buf (bounded). */
static const char* read_str_after(const char *at, char *buf, size_t cap) {
  const char *c = strchr(at, ':');
  if (!c) return NULL;
  c = skip_ws(c + 1);
  if (*c != '"') return NULL;
  c++;
  size_t k = 0;
  while (*c && *c != '"' && k + 1 < cap) buf[k++] = *c++;
  buf[k] = '\0';
  return (*c == '"') ? c + 1 : NULL;
}
/* read a [x,y,z] array after the key into pt[3]. Missing entries stay 0. */
static const char* read_vec_after(const char *at, float pt[3]) {
  const char *c = strchr(at, ':');
  if (!c) return NULL;
  c = skip_ws(c + 1);
  if (*c != '[') return NULL;
  c++;
  for (int i = 0; i < 3; i++) {
    c = skip_ws(c);
    char *end = NULL;
    double v = strtod(c, &end);
    if (end == c) break;
    pt[i] = (float)v;
    c = skip_ws(end);
    if (*c == ',') c++;
  }
  const char *close = strchr(c, ']');
  return close ? close + 1 : c;
}

int ex_cj_read_orders(const char *json, ex_cj_orders_t *o) {
  if (!json || !o) return -1;
  memset(o, 0, sizeof(*o));

  const char *t = find_key(json, "tick");
  if (t) read_int_after(t, &o->tick);
  const char *sp = find_key(json, "speaker");
  if (sp) read_str_after(sp, o->speaker, sizeof(o->speaker));

  /* find the commands array; if absent, zero commands (valid -- a brain may
   * just observe and say nothing). */
  const char *arr = find_key(json, "commands");
  if (!arr) return 0;
  arr = strchr(arr, '[');
  if (!arr) return 0;
  /* find the MATCHING close bracket (point arrays nest one level), not just the
     first ']' -- otherwise an inner "point":[..] truncates the scan. */
  const char *end = arr + 1;
  int depth = 1;
  while (*end && depth > 0) {
    if (*end == '[') depth++;
    else if (*end == ']') depth--;
    if (depth == 0) break;
    end++;
  }
  if (!*end) end = arr + strlen(arr);

  /* iterate objects between arr and end by scanning for each "type" key. Each
   * command is anchored by its "type" field; we read the sibling fields that
   * follow it up to the next "type" or the array end. */
  const char *p = arr;
  while (o->n_cmds < EX_CJ_MAX_CMDS) {
    const char *ty = find_key(p, "type");
    if (!ty || ty >= end) break;
    const char *next_ty = find_key(ty + 1, "type");
    if (!next_ty || next_ty > end) next_ty = end;   /* this command's field window */

    ex_cj_command_t *c = &o->cmds[o->n_cmds];
    memset(c, 0, sizeof(*c));
    c->zone = -1;   /* default: all units */

    char verb[EX_CJ_NAME_MAX] = {0};
    read_str_after(ty, verb, sizeof(verb));
    c->type = ex_cj_cmd_from_name(verb);

    /* only look for sibling fields within this command's window [ty, next_ty) */
    const char *kz = find_key(ty, "zone");
    if (kz && kz < next_ty) read_int_after(kz, &c->zone);
    const char *kp = find_key(ty, "point");
    if (kp && kp < next_ty) read_vec_after(kp, c->point);
    const char *k0 = find_key(ty, "param0");
    if (k0 && k0 < next_ty) read_num_after(k0, &c->param0);
    const char *k1 = find_key(ty, "param1");
    if (k1 && k1 < next_ty) read_num_after(k1, &c->param1);

    if (c->type != EX_CJ_CMD_NONE) o->n_cmds++;   /* skip unknown verbs */
    p = next_ty;
  }
  return o->n_cmds;
}
