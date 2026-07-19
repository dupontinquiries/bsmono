#include <stdio.h>
#include "exengine/engine.h"
#include "exengine/bsgames_handoff.h"
#include "inc/game.h"

int main(int argc, char **argv)
{
  /* bsgames handoff: the game centre's "join a friend" spawns us with BSGAMES_HANDOFF_* set.
   * Parse + surface it before the engine starts; a normal standalone launch parses to NONE and
   * changes nothing. Marked follow-up (see bsgames_handoff.h): resolve the peer's dial address
   * from the shared control-plane store and feed ex_net_client_open() so the join skips the
   * menu entirely, like the fleet's Rust games do. */
  bs_handoff_t handoff;
  switch (bs_handoff_from_env(&handoff)) {
    case BS_HANDOFF_HOST:
      printf("[handoff] host as %s (%s)\n", handoff.me_name, handoff.me_id);
      break;
    case BS_HANDOFF_JOIN:
      printf("[handoff] join %s as %s (%s)\n", handoff.peer_id, handoff.me_name, handoff.me_id);
      break;
    default:
      break;
  }

  ex_init_ptr = game_init;
  ex_update_ptr = game_update;
  ex_draw_ptr = game_draw;
  ex_exit_ptr = game_exit;
  ex_keypressed_ptr = game_keypressed;
  ex_mousepressed_ptr = game_mousepressed;
  ex_mousemotion_ptr = game_mousemoition;
  ex_mousewheel_ptr = game_mousewheel;
  ex_resize_ptr = game_resize;

  exengine(argv, "exengine-dev", EX_ENGINE_SOUND);

  return EXIT_SUCCESS;
}