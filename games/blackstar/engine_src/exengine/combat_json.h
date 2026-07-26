/* combat_json -- a JSON boundary between the game's combat/movement state and an
 * external "brain" (intended for later LLM integration: e.g. Blackstar reading
 * how the player engages and issuing funky directives back).
 *
 * Two directions:
 *   SNAPSHOT  (game -> brain): serialize the current combat experience -- the
 *             player, allies following the player, and the troops/enemies with
 *             their owned-space zones + modes, plus ENGAGEMENT SIGNAL input
 *             variables (aggression, recent damage, kills, pressure) that a
 *             roleplaying brain reacts to.
 *   COMMANDS  (brain -> game): a parsed directive set -- flock/move orders for
 *             troop groups, and how allies should follow/engage -- enough to
 *             close the loop: game state -> JSON -> brain -> JSON -> game.
 *
 * Pure data + a minimal hand-rolled JSON writer/parser (no external deps), so
 * this is unit-testable in isolation and decoupled from render + netcode. The
 * caller owns actually applying commands to live units and reading live state
 * into the snapshot; this module only defines the wire format and the codecs.
 */
#ifndef EX_COMBAT_JSON_H
#define EX_COMBAT_JSON_H

#include <stddef.h>

#define EX_CJ_MAX_UNITS   64
#define EX_CJ_MAX_ALLIES   8
#define EX_CJ_MAX_CMDS    16
#define EX_CJ_NAME_MAX    24
#define EX_CJ_JSON_MAX  8192

/* -- snapshot: one combatant (troop/enemy) as the brain sees it -- */
typedef struct {
  int   id;
  float pos[3];
  float hp, max_hp;
  int   zone;          /* 0 front / 1 mid / 2 back (owned-space line), -1 unknown */
  int   kind;          /* ex_enemy_kind */
  int   element;       /* ex_element_t */
  int   alive;
  int   is_commander;  /* 1 if this unit leads a squad */
  int   squad;         /* commander id it answers to, -1 independent */
  int   scattering;    /* 1 if leaderless + panicking (broke formation) */
} ex_cj_unit_t;

/* -- snapshot: one ally following the player -- */
typedef struct {
  int   id;
  float pos[3];
  float hp, max_hp;
  int   mode;          /* ex_ally_mode_t: 0 follow / 1 engage / 2 down */
  int   target;        /* enemy id it is engaging, -1 none */
} ex_cj_ally_t;

/* -- snapshot: the ENGAGEMENT SIGNALS a brain reacts to. These are the tracked
 *    "input variables" that make the loop meaningful -- how the player is
 *    fighting, not just where things are. -- */
typedef struct {
  float aggression;    /* 0..1 -- how hard the player is pushing (fire rate + closing) */
  float recent_dmg;    /* damage the player has taken lately (decays) */
  float pressure;      /* 0..1 -- how surrounded/threatened the player is */
  int   kills;         /* kills so far this encounter */
  int   enemies_alive; /* live troop count */
  float player_hp_frac;/* 0..1 */
} ex_cj_signals_t;

/* -- the full snapshot the brain reads -- */
typedef struct {
  int             tick;               /* frame/sequence counter (for ordering) */
  float           player_pos[3];
  int             in_vehicle;
  ex_cj_signals_t signals;
  int             n_units;
  ex_cj_unit_t    units[EX_CJ_MAX_UNITS];
  int             n_allies;
  ex_cj_ally_t    allies[EX_CJ_MAX_ALLIES];
} ex_cj_snapshot_t;

/* -- commands the brain issues back -- */
typedef enum {
  EX_CJ_CMD_NONE = 0,
  EX_CJ_CMD_FLOCK_TO,     /* move a zone/group of troops toward a point (cohesive) */
  EX_CJ_CMD_FLOCK_SPREAD, /* spread a group out around a point (encircle) */
  EX_CJ_CMD_HOLD,         /* a group holds position / stops advancing */
  EX_CJ_CMD_FOCUS,        /* a group focuses the player (press the attack) */
  EX_CJ_CMD_RETREAT,      /* a group falls back from the player */
  EX_CJ_CMD_ALLY_FOLLOW,  /* set ally follow behavior: param0 = mode (0 follow/1 engage) */
  EX_CJ_CMD_COUNT
} ex_cj_cmd_type;

/* a command targets a GROUP: by zone (0/1/2) or -1 for "all", and carries a
 * point + scalar params the verb interprets. */
typedef struct {
  ex_cj_cmd_type type;
  int   zone;            /* target zone 0/1/2, or -1 = all units */
  float point[3];        /* target/rally point (for FLOCK_TO / SPREAD / etc.) */
  float param0, param1;  /* verb-specific (radius, mode, urgency 0..1, ...) */
} ex_cj_command_t;

typedef struct {
  int             tick;               /* echoes the snapshot tick it responds to */
  char            speaker[EX_CJ_NAME_MAX];  /* e.g. "blackstar" (flavor/routing) */
  int             n_cmds;
  ex_cj_command_t cmds[EX_CJ_MAX_CMDS];
} ex_cj_orders_t;

/* ---- codecs ---- */

/* Serialize a snapshot to JSON. Returns bytes written (excluding NUL), or -1 on
 * overflow. `out` is always NUL-terminated on success. */
int ex_cj_write_snapshot(const ex_cj_snapshot_t *s, char *out, size_t cap);

/* Parse an orders JSON blob into `o`. Returns the number of commands parsed
 * (>=0), or -1 on malformed input. Tolerant: unknown keys are skipped, missing
 * fields take sane defaults, so a chatty LLM response still decodes. */
int ex_cj_read_orders(const char *json, ex_cj_orders_t *o);

/* Serialize orders back to JSON (round-trip + testing + logging). */
int ex_cj_write_orders(const ex_cj_orders_t *o, char *out, size_t cap);

/* map a command verb to/from its wire string (stable names for the brain). */
const char*     ex_cj_cmd_name(ex_cj_cmd_type t);
ex_cj_cmd_type  ex_cj_cmd_from_name(const char *s);

/* ---- live integration helpers ---- */

/* Track the engagement signals across frames. The game feeds raw events; this
 * derives the smoothed 0..1 aggression/pressure a brain reads. Keep one of
 * these per active encounter. */
typedef struct {
  float aggression;    /* smoothed */
  float pressure;      /* smoothed */
  float _fire_accum;   /* internal: recent fire activity */
} ex_cj_signal_tracker_t;

void ex_cj_signals_reset(ex_cj_signal_tracker_t *t);
/* advance the tracker one frame:
 *   firing      = 1 if the player fired this frame
 *   closing     = player's speed toward the nearest enemy (>=0), units/s
 *   near_count  = enemies within a threat radius (drives pressure)
 *   dt          = frame time */
void ex_cj_signals_tick(ex_cj_signal_tracker_t *t, int firing, float closing,
                        int near_count, float dt);

/* Fill a snapshot from the game's parallel arrays. Any array pointer may be
 * NULL (that section is left empty). Counts are clamped to the EX_CJ_MAX_*.
 * `sig` (nullable) provides the smoothed aggression/pressure; the discrete
 * signals (kills, hp, alive count) are passed explicitly. */
void ex_cj_build_snapshot(ex_cj_snapshot_t *snap, int tick,
                          const float player_pos[3], int in_vehicle,
                          const ex_cj_signal_tracker_t *sig,
                          int kills, float player_hp_frac,
                          /* units */
                          int n_units, const int *ids, const float (*pos)[3],
                          const float *hp, const float *max_hp, const int *zone,
                          const int *kind, const int *element, const int *alive,
                          const int *is_commander, const int *squad, const int *scattering,
                          /* allies */
                          int n_allies, const int *ally_id, const float (*ally_pos)[3],
                          const float *ally_hp, const float *ally_max_hp,
                          const int *ally_mode, const int *ally_target);

#endif /* EX_COMBAT_JSON_H */
