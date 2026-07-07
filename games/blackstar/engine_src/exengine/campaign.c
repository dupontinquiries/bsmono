#include "campaign.h"
#include <string.h>
#include <stdio.h>

static void slide(ex_mission_t *m, int intro, const char *text)
{
  if (intro) {
    if (m->intro_slides < EX_MAX_SLIDES)
      snprintf(m->intro[m->intro_slides++], EX_SLIDE_LEN, "%s", text);
  } else {
    if (m->outro_slides < EX_MAX_SLIDES)
      snprintf(m->outro[m->outro_slides++], EX_SLIDE_LEN, "%s", text);
  }
}

/* The single-player arc follows this fan project's own "Doomsday" storyline.
   All wording here is ORIGINAL, terse summary prose -- it names the story's
   beats and characters but reproduces no external script text. */
void ex_campaign_init(ex_campaign_t *c)
{
  memset(c, 0, sizeof(*c));
  c->unlocked = 0;

  // ---- Mission 1: the Pluto cold open (Cryotek's ice faction) ----
  ex_mission_t *m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "1 _ the pluto line");
  snprintf(m->map, sizeof(m->map), "frostline");
  m->grunts = 3; m->corrupted = 0; m->objective = EX_OBJ_ELIMINATE;
  slide(m,1,"the system's edge -- a ring of autobot iron, and the long cold beyond it.");
  slide(m,1,"witwicky holds the near flank. you take the ice.");
  slide(m,1,"cryotek's crew are mining the dead for something. end it here.");
  slide(m,0,"cryotek falls. what he clawed for out of the ice is yours now.");
  slide(m,0,"but prime's unease does not lift. the frontier is bigger than the fleet.");

  // ---- Mission 2: the Outer Rim scavenger warlord (Blackstar + Spearcons) ----
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "2 _ the scavenger king");
  snprintf(m->map, sizeof(m->map), "outer_rim");
  m->grunts = 4; m->corrupted = 0; m->objective = EX_OBJ_SURVIVE; m->survive_seconds = 60.0f;
  slide(m,1,"a shattered megastructure orbits a dark star. someone built a kingdom of the wreckage.");
  slide(m,1,"he does not serve the devourer. he collects what it leaves behind.");
  slide(m,1,"his lieutenants stick you where you stand. so do not stand still.");
  slide(m,0,"you held the ambush. the warlord withdraws -- unhurried, unafraid.");
  slide(m,0,"'empires burn,' he says across the comm. 'i collect the ashes.'");

  // ---- Mission 3: Earth, the beach opens (Unicron stirs; protect civilians) ----
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "3 _ the beach opens");
  snprintf(m->map, sizeof(m->map), "wastes");
  m->grunts = 2; m->corrupted = 2; m->objective = EX_OBJ_PROTECT;
  slide(m,1,"a sunlit coast -- recovered, hopeful, unaware. then the ground takes a breath.");
  slide(m,1,"the sea peels back. something vast turns over beneath the pacific.");
  slide(m,1,"get the people clear. the first of the corrupted are already surfacing.");
  slide(m,0,"the civilians are out. the fissure does not close.");
  slide(m,0,"prime hears it now -- not with ears, through spark. 'the architects.'");

  // ---- Mission 4: what Jhiaxus grew (the Reassembler reveal; Corrupted drift) ----
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "4 _ what jhiaxus grew");
  snprintf(m->map, sizeof(m->map), "vault");
  m->grunts = 1; m->corrupted = 3; m->objective = EX_OBJ_ELIMINATE;
  slide(m,1,"the architect meant to study it. to catalogue it. to keep it.");
  slide(m,1,"each generation drifts further from the last -- and from his control.");
  slide(m,1,"there is no containment left. only the ones already loose.");
  slide(m,0,"the room goes quiet. that is not the same as safe.");
  slide(m,0,"the truth surfaces: the devourer is no beast. it is a machine -- and it was built.");

  // ---- Mission 5: the mantle dive (Hot Rod -> Rodimus, the Matrix) ----
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "5 _ the mantle dive");
  snprintf(m->map, sizeof(m->map), "foundry");
  m->grunts = 2; m->corrupted = 2; m->objective = EX_OBJ_CONTROL;
  slide(m,1,"down through the ruptured mantle, into corridors of living metal and energon light.");
  slide(m,1,"a reckless young scout dives beside you -- he idolizes prime, and it shows.");
  slide(m,1,"hold the descent. whatever you open down here will not close gently.");
  slide(m,0,"at the worst moment the matrix opens -- for the one who reached without wanting to.");
  slide(m,0,"the scout rises out of the light changed. rodimus, now. the tide turns.");

  // ---- Mission 6: beneath the Pacific (Unicron, boss) ----
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "6 _ beneath the pacific");
  snprintf(m->map, sizeof(m->map), "cybertron");
  m->grunts = 2; m->corrupted = 1; m->objective = EX_OBJ_ELIMINATE;
  slide(m,1,"everything scattered across the system gathers over one drowned wound in the world.");
  slide(m,1,"the reassembler wakes fully -- continental limbs, a mind older than the makers.");
  slide(m,1,"it was built to break dead worlds and forge new ones. tonight it settles for this one.");
  slide(m,0,"the avatar goes dark beneath the waves. the machine sleeps -- not dead, only denied.");
  slide(m,0,"megatron lowers his cannon. a truce, for now. beyond the shield, the wider cosmos waits.");
}

void ex_campaign_complete(ex_campaign_t *c, int mission_index)
{
  if (mission_index + 1 > c->unlocked && mission_index + 1 < c->count)
    c->unlocked = mission_index + 1;
}

const char* ex_objective_name(ex_objective_t o)
{
  switch (o) {
    case EX_OBJ_ELIMINATE: return "eliminate all";
    case EX_OBJ_SURVIVE:   return "survive";
    case EX_OBJ_BREACH:    return "breach";
    case EX_OBJ_CONTROL:   return "hold the point";
    case EX_OBJ_PROTECT:   return "protect civilians";
    default:               return "?";
  }
}
