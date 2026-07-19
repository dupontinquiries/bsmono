/* bsgames_handoff.h -- the BSGAMES_HANDOFF_* env protocol, C side.
 *
 * The bsgames game centre's "join a friend" spawns the friend's game with three env vars set
 * (producer: bsgames::centre::handoff_env; canonical parser + rules: the Rust
 * games/shared/netcode/src/handoff.rs -- this C port mirrors those rules EXACTLY, and
 * tests/test_handoff.c pins the same cases as the Rust suite):
 *
 *   BSGAMES_HANDOFF_ROLE  "host" | "join"
 *   BSGAMES_HANDOFF_ME    "<id>|<display name>"  (the identity bsgames already published,
 *                          so this process presents as the SAME player, not a new one)
 *   BSGAMES_HANDOFF_PEER  the friend's profile id (join only)
 *
 * Entirely additive: with no handoff env set (a normal standalone launch), bs_handoff_from_env
 * yields role BS_HANDOFF_NONE and nothing about menu-driven startup changes. Anything
 * missing/malformed also yields NONE -- a bad handoff falls back to the menu, never crashes.
 *
 * What consumes this: main.c parses at startup and logs; the marked follow-up is game-side --
 * resolving the peer's dial address from the shared control-plane store and feeding it to
 * ex_net_client_open() (net_transport.c), the engine's existing data-plane entry.
 */
#ifndef BSGAMES_HANDOFF_H
#define BSGAMES_HANDOFF_H

#define BS_HANDOFF_NONE 0
#define BS_HANDOFF_HOST 1
#define BS_HANDOFF_JOIN 2

#define BS_HANDOFF_FIELD 128

typedef struct {
  int role; /* BS_HANDOFF_NONE / HOST / JOIN */
  char me_id[BS_HANDOFF_FIELD];
  char me_name[BS_HANDOFF_FIELD];
  char peer_id[BS_HANDOFF_FIELD]; /* JOIN only; empty otherwise */
} bs_handoff_t;

/* Pure parse of the three variables (any may be NULL). Returns the role written to out->role;
 * out is fully zeroed first, so NONE always leaves clean fields. Mirrors the Rust rules:
 *   - me must contain '|' separating id|name, both copied (truncated to the field size);
 *   - role "host" ignores peer; role "join" REQUIRES a non-empty peer (else NONE);
 *   - unknown roles are NONE, never defaulted. */
int bs_handoff_parse(const char *role, const char *me, const char *peer, bs_handoff_t *out);

/* The startup path: parse from getenv(). */
int bs_handoff_from_env(bs_handoff_t *out);

#endif
