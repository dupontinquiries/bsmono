/* test_mapfsm.c
   Verifies (1) the pure-C mapfsm port and (2) the Rust FFI bridge agree,
   and that grown features actually spawn chunks into a chunk world.
   Built with EX_HAVE_RUST_FSM when the Rust lib is linked. */
#include "mapfsm.h"
#include "coll_chunk.h"
#include <stdio.h>
#include <assert.h>
#include <string.h>

#ifdef EX_HAVE_RUST_FSM
// declared by the Rust cdylib (repr(C), matches ex_fsm_result_t/ex_site_t)
extern ex_fsm_result_t ex_rust_fsm_grow(const ex_site_t *site);
#endif

static ex_site_t mk(float sight, float slope, int high, int indoor) {
  ex_site_t s; memset(&s, 0, sizeof(s));
  s.sightline = sight; s.slope = slope; s.is_high_point = high;
  s.indoor = indoor; s.high_traffic = sight > 30.0f; s.danger_yaw = 0.7f; s.seed = 7;
  return s;
}

int main(void) {
  struct { const char *label; ex_site_t s; ex_feature_t expect; } cases[] = {
    { "medium lane",      mk(20.0f, 0.0f, 0, 0), EX_FEATURE_SIGHTLINE_BREAKER },
    { "long open lane",   mk(40.0f, 0.0f, 0, 0), EX_FEATURE_WRECKAGE },
    { "outdoor highpoint",mk(20.0f, 0.0f, 1, 0), EX_FEATURE_LOGISTICS_SPINE },
    { "indoor highpoint", mk(20.0f, 0.0f, 1, 1), EX_FEATURE_VANTAGE },
    { "slope over open",  mk(30.0f, 0.3f, 0, 0), EX_FEATURE_SENTINEL },
    { "steep dead spot",  mk(6.0f,  0.9f, 0, 0), EX_FEATURE_NONE },
  };
  int n = sizeof(cases)/sizeof(cases[0]);

  printf("=== pure-C mapfsm ===\n");
  for (int i = 0; i < n; i++) {
    ex_fsm_result_t r = ex_fsm_grow(&cases[i].s);
    printf("  %-18s -> %-9s (expect %s)\n", cases[i].label,
           ex_feature_name(r.feature), ex_feature_name(cases[i].expect));
    assert(r.feature == cases[i].expect);
  }

#ifdef EX_HAVE_RUST_FSM
  printf("=== Rust FFI bridge (real fsm.rs gating over C ABI) ===\n");
  for (int i = 0; i < n; i++) {
    ex_fsm_result_t r = ex_rust_fsm_grow(&cases[i].s);
    ex_fsm_result_t c = ex_fsm_grow(&cases[i].s);
    printf("  %-18s -> %-9s  (C says %s)\n", cases[i].label,
           ex_feature_name(r.feature), ex_feature_name(c.feature));
    assert(r.feature == c.feature);                 // Rust and C agree
    assert(r.hp == c.hp);                           // same toughness
    assert(r.half_extents[1] == c.half_extents[1]); // same size
  }
  printf("  C and Rust FSM agree on all cases\n");
#else
  printf("(Rust FFI not linked -- build with the rust_fsm bridge to test it)\n");
#endif

  // spawn into a chunk world
  printf("=== grow into chunk world ===\n");
  ex_chunk_world_t *w = ex_chunk_world_new();
  int spawned = 0;
  for (int i = 0; i < n; i++) {
    ex_feature_t f = ex_fsm_grow_into_world(w, &cases[i].s, i);
    if (f != EX_FEATURE_NONE) spawned++;
  }
  printf("  spawned %d chunks from %d sites\n", spawned, n);
  assert(spawned == 5); // all but the dead spot
  ex_chunk_world_free(w);

  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
