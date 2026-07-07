/* campaign
  Single-player campaign: an ordered list of missions, each with a map, an
  enemy makeup, an objective, and intro/outro cutscene slides.

  Cutscenes are text-slide sequences (shown over a dim backdrop with the VGA
  text system) -- not rendered cinematics, but they carry the story between
  missions and can be advanced/skipped. All narrative text here is ORIGINAL
  writing for this game; it is inspired by the general themes (cosmic
  consequence, elemental factions, a relentless corruption) but does not
  reproduce any external script.

  Pure data + a tiny cursor, so the flow is fully testable headless.
*/

#ifndef EX_CAMPAIGN_H
#define EX_CAMPAIGN_H

#include "element.h"

#define EX_MAX_SLIDES   6
#define EX_MAX_MISSIONS 6
#define EX_SLIDE_LEN    120

typedef enum {
  EX_OBJ_ELIMINATE,   // destroy all enemies
  EX_OBJ_SURVIVE,     // last N seconds
  EX_OBJ_BREACH,      // destroy a target structure
  EX_OBJ_CONTROL,     // hold objective pads to reach a control score
  EX_OBJ_PROTECT      // keep the human survivors alive while clearing the threat
} ex_objective_t;

typedef struct {
  char title[48];
  char map[32];              // map id -> data/<map>.iqm
  int  grunts, corrupted;    // enemy makeup
  ex_objective_t objective;
  float survive_seconds;     // for EX_OBJ_SURVIVE
  char intro[EX_MAX_SLIDES][EX_SLIDE_LEN];
  int  intro_slides;
  char outro[EX_MAX_SLIDES][EX_SLIDE_LEN];
  int  outro_slides;
} ex_mission_t;

typedef struct {
  ex_mission_t missions[EX_MAX_MISSIONS];
  int count;
  int unlocked;              // highest unlocked mission index (progression)
} ex_campaign_t;

/**
 * [ex_campaign_default build the built-in campaign]
 */
void ex_campaign_init(ex_campaign_t *c);

/**
 * [ex_campaign_complete mark a mission cleared, unlocking the next]
 */
void ex_campaign_complete(ex_campaign_t *c, int mission_index);

const char* ex_objective_name(ex_objective_t o);

#endif // EX_CAMPAIGN_H
