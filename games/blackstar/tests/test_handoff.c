/* test_handoff.c -- the BSGAMES_HANDOFF_* parser, pinned to the SAME cases as the canonical
   Rust suite (games/shared/netcode/src/handoff.rs tests), so the two parsers can't drift:
   join with all three vars, host without a peer, join-without-peer rejected (not defaulted),
   missing/malformed me rejected, unknown role + nothing set both NONE. Plus the C-only
   concerns: field truncation stays NUL-terminated, and rejects leave zeroed fields. */
#include <assert.h>
#include <stdio.h>
#include <string.h>
#include "bsgames_handoff.h"

int main(void)
{
  bs_handoff_t h;

  /* parses_join_with_all_three_vars */
  assert(bs_handoff_parse("join", "me@0|Kessler", "bob@1", &h) == BS_HANDOFF_JOIN);
  assert(strcmp(h.me_id, "me@0") == 0);
  assert(strcmp(h.me_name, "Kessler") == 0);
  assert(strcmp(h.peer_id, "bob@1") == 0);

  /* parses_host_without_a_peer */
  assert(bs_handoff_parse("host", "me@0|Kessler", NULL, &h) == BS_HANDOFF_HOST);
  assert(strcmp(h.me_id, "me@0") == 0);
  assert(strcmp(h.me_name, "Kessler") == 0);
  assert(h.peer_id[0] == '\0');

  /* join_without_a_peer_id_is_rejected_not_defaulted (NULL and empty both) */
  assert(bs_handoff_parse("join", "me@0|Kessler", NULL, &h) == BS_HANDOFF_NONE);
  assert(bs_handoff_parse("join", "me@0|Kessler", "", &h) == BS_HANDOFF_NONE);

  /* missing_or_malformed_me_field_is_rejected */
  assert(bs_handoff_parse("join", NULL, "bob@1", &h) == BS_HANDOFF_NONE);
  assert(bs_handoff_parse("join", "no-pipe-here", "bob@1", &h) == BS_HANDOFF_NONE);

  /* unknown_role_and_nothing_set_both_yield_none */
  assert(bs_handoff_parse("spectate", "me@0|Kessler", NULL, &h) == BS_HANDOFF_NONE);
  assert(bs_handoff_parse(NULL, NULL, NULL, &h) == BS_HANDOFF_NONE);

  /* rejects leave clean zeroed fields (safe to read unconditionally) */
  bs_handoff_parse("spectate", "me@0|Kessler", "x", &h);
  assert(h.me_id[0] == '\0' && h.me_name[0] == '\0' && h.peer_id[0] == '\0');

  /* C-only: oversize fields truncate and stay NUL-terminated */
  {
    char big[400];
    memset(big, 'a', sizeof big);
    big[180] = '|';
    big[399] = '\0';
    assert(bs_handoff_parse("host", big, NULL, &h) == BS_HANDOFF_HOST);
    assert(strlen(h.me_id) == BS_HANDOFF_FIELD - 1);
    assert(strlen(h.me_name) == BS_HANDOFF_FIELD - 1);
  }

  /* pipe at position 0: empty id, full name -- mirrors Rust split_once("|") semantics */
  assert(bs_handoff_parse("host", "|JustAName", NULL, &h) == BS_HANDOFF_HOST);
  assert(h.me_id[0] == '\0');
  assert(strcmp(h.me_name, "JustAName") == 0);

  printf("test_handoff: all assertions passed\n");
  return 0;
}
