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
  ex_mission_t *m;

  /* ============ TRILOGY ONE ============ */

  /* ---- FILM I, ch.1: the quiet before fracture (Pluto) -- duty, and an old soldier ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "I.1 _ the pluto line");
  snprintf(m->map, sizeof(m->map), "pluto");
  m->grunts = 3; m->corrupted = 0; m->objective = EX_OBJ_ELIMINATE;
  slide(m,1,"the system's edge. an old soldier in an older suit walks the wall he helped build.");
  slide(m,1,"witwicky doesn't talk about earth much. out here, quiet is what safe sounds like.");
  slide(m,1,"cryotek's crew are mining the dead beyond the wire. end it before the quiet ends first.");
  slide(m,0,"the ice line holds. witwicky exhales for the first time in an hour.");
  slide(m,0,"'you get used to the cold,' he says. 'you never get used to the waiting.'");

  /* ---- FILM I, ch.2: the beach opens (Earth) -- a civilian watches the world break ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "I.2 _ the beach opens");
  snprintf(m->map, sizeof(m->map), "beach");
  m->grunts = 2; m->corrupted = 2; m->objective = EX_OBJ_PROTECT;
  slide(m,1,"a mars shard came down hollow -- its trajectory burned sunward. to los angeles.");
  slide(m,1,"aiden is nobody: a kid with a skateboard on a los angeles beach. then the ground takes a breath.");
  slide(m,1,"he doesn't run. he starts pulling people up the dunes. get the rest clear -- cover him.");
  slide(m,0,"the civilians are out. aiden is still standing at the fissure edge, shaking, staying.");
  slide(m,0,"prime hears it through spark, not sound. and far away, something old turns toward earth.");

  /* ---- FILM I, ch.3: outrun the sea (the beach) -- the drill becomes real ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "I.3 _ outrun the sea");
  snprintf(m->map, sizeof(m->map), "beach");
  m->grunts = 1; m->corrupted = 1; m->objective = EX_OBJ_SURVIVE; m->survive_seconds = 75.0f;
  slide(m,1,"the LA shorefront is gutted -- the pier down, the overpass snapped, fires in the sand.");
  slide(m,1,"the fissure breathes in slow pulses -- every exhale drags the ocean OUT. aiden knows this drill.");
  slide(m,1,"when the sea pulls back: SPRINT. climb the wreck-towers. do not be low when it returns.");
  slide(m,0,"three waves. three climbs. the coast is scoured clean -- and you are still standing.");
  slide(m,0,"aiden, hoarse from counting heads: 'the drill works. tell everyone. THE DRILL WORKS.'");

  /* ---- FILM II, ch.1: the truce (the wastes) -- an enemy asks for peace ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "II.1 _ the truce");
  snprintf(m->map, sizeof(m->map), "wastes");
  m->grunts = 3; m->corrupted = 2; m->objective = EX_OBJ_CONTROL;
  slide(m,1,"megatron returns -- drawn by the pulse, but changed. harder. quieter. he asks to TALK.");
  slide(m,1,"'the devourer eats decepticons too,' he says. prime studies the hand he has fought forever.");
  slide(m,1,"the corrupted crash the parley. hold the ground -- and decide what a truce is worth.");
  slide(m,0,"the line holds, side by side. neither of them names what just happened.");
  slide(m,0,"witwicky, on comms, very quietly: 'never thought i'd cover HIS flank.'");

  /* ---- FILM II, ch.2: the mantle dive (the core) -- guilt becomes a Prime ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "II.2 _ the mantle dive");
  snprintf(m->map, sizeof(m->map), "core");
  m->grunts = 2; m->corrupted = 2; m->objective = EX_OBJ_CONTROL;
  slide(m,1,"down the ruptured mantle: living-metal corridors, energon rivers, cathedral dark.");
  slide(m,1,"hot rod dives beside you -- reckless, loud, terrified of one thing: failing prime.");
  slide(m,1,"hold the descent. the avatar is forming out of the magma, and it is looking at optimus.");
  slide(m,0,"unicron impales prime. hot rod doesn't cause the fall this time -- he catches it.");
  slide(m,0,"the matrix opens for the one who reached without wanting it. rodimus rises, burning.");

  /* ---- FILM III, ch.1: the drowned approach (the trench) -- what victory costs ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "III.1 _ the drowned approach");
  snprintf(m->map, sizeof(m->map), "trench");
  m->grunts = 1; m->corrupted = 4; m->objective = EX_OBJ_SURVIVE; m->survive_seconds = 70.0f;
  slide(m,1,"rodimus leads now, and hates how the word sounds. prime recovers in the fleet above.");
  slide(m,1,"below the pacific, the corrupted converge on a light that should not be down here.");
  slide(m,1,"survive the descent past the vent spires. every one that reaches the rift feeds it.");
  slide(m,0,"the seabed splits. what was buried is not rock. it is a door, and it is open.");
  slide(m,0,"aiden's voice from the bunker: 'the old site under us -- it's humming the same note.'");

  /* ---- FILM III, ch.2: beneath the pacific -- megatron's answer ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "III.2 _ beneath the pacific");
  snprintf(m->map, sizeof(m->map), "unicron");
  m->grunts = 5; m->corrupted = 4; m->objective = EX_OBJ_ELIMINATE;
  slide(m,1,"the plates begin to close over the only road to the core. someone has to hold them.");
  slide(m,1,"megatron plants himself under the collapsing world. 'go,' he says. nothing else.");
  slide(m,1,"reach the spark chamber. rodimus carries the matrix -- and everyone megatron bought time for.");
  slide(m,0,"the avatar goes dark. the machine sleeps -- not dead, only denied. the plates settle.");
  slide(m,0,"they never find megatron. prime marks the spot anyway. the war's oldest debt, paid last.");

  /* ============ TRILOGY TWO ============ */

  /* ---- FILM IV, ch.1: the scavenger king (outer rim) -- cynicism meets belief ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "IV.1 _ the scavenger king");
  snprintf(m->map, sizeof(m->map), "outer_rim");
  m->grunts = 4; m->corrupted = 0; m->objective = EX_OBJ_SURVIVE; m->survive_seconds = 60.0f;
  slide(m,1,"years on. the frontier is full of cybertron's scattered children -- and their king.");
  slide(m,1,"blackstar doesn't serve the devourer. he collects what it leaves. he finds hope FUNNY.");
  slide(m,1,"his lieutenants pin you where you stand. so do not stand still.");
  slide(m,0,"you held. the warlord withdraws, unhurried. 'empires burn. i collect the ashes.'");
  slide(m,0,"rodimus, low: 'he's not wrong about the burning. he's wrong about what's left.'");

  /* ---- FILM IV, ch.2: the carrion court (carrion) -- the clone clans at home ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "IV.2 _ the carrion court");
  snprintf(m->map, sizeof(m->map), "carrion");
  m->grunts = 4; m->corrupted = 1; m->objective = EX_OBJ_BREACH;
  slide(m,1,"blackstar's homeworld is a wreck-pile with a heartbeat. his line are CLONES -- all of them.");
  slide(m,1,"every body a patchwork of salvage; grief worn openly, as armour. they bury nothing.");
  slide(m,1,"break the tithe-engine at the meet-ring. the clans will not thank you. they may respect you.");
  slide(m,0,"the engine falls. an old clone watches it burn and says: 'he was the first of us to laugh.'");
  slide(m,0,"rodimus walks the wreck-streets a long time. 'they made a LIFE out here. out of nothing.'");

  /* ---- FILM V, ch.1: what jhiaxus grew (the vault) -- the architect's fear ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "V.1 _ what jhiaxus grew");
  snprintf(m->map, sizeof(m->map), "roots");   /* his under-garden: the cave the horror was built for */
  m->grunts = 1; m->corrupted = 3; m->objective = EX_OBJ_ELIMINATE;
  slide(m,1,"the architect meant to study the corruption. to catalogue it. to KEEP it.");
  slide(m,1,"each generation drifts further from his control -- and he cannot admit he is afraid.");
  slide(m,1,"there is no containment left. only the ones already loose. put them down gently, if you can.");
  slide(m,0,"the room goes quiet. that is not the same as safe.");
  slide(m,0,"in the wreckage, his notes: the devourer is no beast. it is a machine. it was BUILT.");

  /* ---- FILM V, ch.2: the garden of helix -- which of us is the copy? ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "V.2 _ the garden of helix");
  snprintf(m->map, sizeof(m->map), "helix");
  m->grunts = 2; m->corrupted = 3; m->objective = EX_OBJ_CONTROL;
  slide(m,1,"helix is beautiful and wrong: ordered vats, mirrored avenues, bodies perfected past identity.");
  slide(m,1,"a clone steps from the row, seamless and calm, and asks rodimus: 'which of us is the copy?'");
  slide(m,1,"hold the dais while the vats are sealed. the architect's PRIME is grown, and it is coming.");
  slide(m,0,"jhiaxus prime falls without anger. its last act is to straighten the row it grew from.");
  slide(m,0,"rodimus, after: 'a spark isn't the shape it wears.' he says it like he finally believes it.");

  /* ---- FILM VI, ch.1: the vault of ice (glaciem) -- memory, and the long grief ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "VI.1 _ the vault of ice");
  snprintf(m->map, sizeof(m->map), "glaciem");
  m->grunts = 3; m->corrupted = 2; m->objective = EX_OBJ_PROTECT;
  slide(m,1,"glaciem keeps the first war in clear ice: every fallen name, every broken frame, preserved.");
  slide(m,1,"the ice-clones live slow and speak rarely. cryotek's line are not conquerors. they are MOURNERS.");
  slide(m,1,"the corrupted have come to eat the archive. defend the memory of the dead. all of it.");
  slide(m,0,"the archive stands. cryotek prime bows -- glacier-deep, centuries slow -- and opens the vault.");
  slide(m,0,"inside: the reassembler's build-record. its heart has a keyhole. the matrix is the key.");

  /* ---- FILM VI, ch.2: the heart of the god-machine -- the flame passes on ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "VI.2 _ the heart of the god-machine");
  snprintf(m->map, sizeof(m->map), "unicron");
  m->grunts = 5; m->corrupted = 5; m->objective = EX_OBJ_ELIMINATE;
  slide(m,1,"everyone comes: the fleet, the clans, the mourners, the perfected. even the ashes' king.");
  slide(m,1,"witwicky flies the breach run he's rehearsed for twenty years. aiden calls it from the bunker.");
  slide(m,1,"cut a road to the spark chamber. rodimus goes in alone -- the way prime once did for him.");
  slide(m,0,"the matrix pours into the heart. not destruction -- QUIET. the god-machine lies down.");
  slide(m,0,"on pluto, the wall watches a calm sky. witwicky: 'now THIS is what safe sounds like.'");
  slide(m,0,"prime, quiet: 'leadership is not the absence of fear... but the will to rise beyond it.'");

  /* ============ THE BRIDGE SET (unlocked from patrol, via the space bridge) ============ */

  /* ---- VII.1: the far gate (the hollow) -- what the bridge charted first ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "VII.1 _ the far gate");
  snprintf(m->map, sizeof(m->map), "hollow");
  m->grunts = 4; m->corrupted = 3; m->objective = EX_OBJ_ELIMINATE;
  slide(m,1,"the bridge did not ask where you wanted to go. it took you where the signal was.");
  slide(m,1,"a cavern world off every chart -- and the corrupted were here FIRST, digging.");
  slide(m,1,"clear the hollow. whatever they were digging toward, they do not get to finish.");
  slide(m,0,"the dig site goes quiet. at its bottom: a door, older than the war, still sealed.");
  slide(m,0,"pale vector's voice, from nowhere: 'now you have seen a far gate. there are more.'");

  /* ---- VII.2: the ashes court (the dreadnought) -- board the king's own ship ---- */
  m = &c->missions[c->count++];
  snprintf(m->title, sizeof(m->title), "VII.2 _ the ashes court");
  snprintf(m->map, sizeof(m->map), "dreadnought");
  m->grunts = 5; m->corrupted = 2; m->objective = EX_OBJ_BREACH;
  slide(m,1,"the bridge's second jump drops you ON HIS DECK. blackstar's flagship, under way.");
  slide(m,1,"corsairs in the cradles, raiders on the rails, and the bridge tower burning cold.");
  slide(m,1,"BREACH the command ward. take the court from the king in his own throne room.");
  slide(m,0,"the ward shatters. blackstar, over the ship's bones: 'kept. every plate of you.'");
  slide(m,0,"he does not pursue. kings who survive know which battles buy nothing.");
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
