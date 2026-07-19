/* bsgames_handoff.c -- see bsgames_handoff.h. Rules mirror the canonical Rust parser
 * (games/shared/netcode/src/handoff.rs) exactly; tests/test_handoff.c pins the same cases. */
#include "bsgames_handoff.h"

#include <stdlib.h>
#include <string.h>

static void copy_field(char dst[BS_HANDOFF_FIELD], const char *src, size_t n)
{
  if (n >= BS_HANDOFF_FIELD) n = BS_HANDOFF_FIELD - 1;
  memcpy(dst, src, n);
  dst[n] = '\0';
}

int bs_handoff_parse(const char *role, const char *me, const char *peer, bs_handoff_t *out)
{
  memset(out, 0, sizeof *out); /* NONE with clean fields is the default for every reject */

  /* the Rust parser evaluates `me` first (the `me?` in parse()): no me, or me without the
   * '|' id|name separator, rejects regardless of role */
  if (!me) return out->role;
  const char *pipe = strchr(me, '|');
  if (!pipe) return out->role;

  if (!role) return out->role;

  if (strcmp(role, "host") == 0) {
    out->role = BS_HANDOFF_HOST;
  } else if (strcmp(role, "join") == 0) {
    if (!peer || !peer[0]) return out->role; /* join REQUIRES a peer -- never defaulted */
    out->role = BS_HANDOFF_JOIN;
    copy_field(out->peer_id, peer, strlen(peer));
  } else {
    return out->role; /* unknown role ("spectate", ...) -> NONE */
  }

  copy_field(out->me_id, me, (size_t)(pipe - me));
  copy_field(out->me_name, pipe + 1, strlen(pipe + 1));
  return out->role;
}

int bs_handoff_from_env(bs_handoff_t *out)
{
  return bs_handoff_parse(getenv("BSGAMES_HANDOFF_ROLE"),
                          getenv("BSGAMES_HANDOFF_ME"),
                          getenv("BSGAMES_HANDOFF_PEER"), out);
}
