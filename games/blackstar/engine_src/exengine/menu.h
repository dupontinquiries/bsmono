/* menu
  Full game-flow state machine + character builder + settings/controls +
  campaign mission select + cutscene playback. Pure logic (no SDL/GL): the
  game feeds abstract nav inputs and draws whatever ex_menu_line reports.
*/

#ifndef EX_MENU_H
#define EX_MENU_H

#include "element.h"
#include "keybind.h"
#include "campaign.h"

typedef enum {
  EX_GS_TITLE,
  EX_GS_MODE,        // skirmish: pick mode
  EX_GS_MAP,         // skirmish: pick map
  EX_GS_MISSIONS,    // campaign: pick mission
  EX_GS_CUTSCENE,    // playing intro/outro slides
  EX_GS_BUILDER,     // pick size/element/layout
  EX_GS_CUSTOMIZE,   // bot customizer: proportions + accent
  EX_GS_SETTINGS,    // graphics
  EX_GS_CONTROLS,
  EX_GS_CODEX,    // view bindings
  EX_GS_PLAYING,
  EX_GS_PAUSED,
  EX_GS_RESULTS   // end-of-run summary
} ex_game_state;

typedef enum {
  EX_NAV_UP, EX_NAV_DOWN, EX_NAV_LEFT, EX_NAV_RIGHT, EX_NAV_CONFIRM, EX_NAV_BACK
} ex_nav_t;

typedef enum { EX_MODE_ARENA_FIGHT, EX_MODE_SURVIVAL, EX_MODE_STALKERS, EX_MODE_DEFEND, EX_MODE_ESCALATION, EX_MODE_ORBRUSH, EX_MODE_SKYDUEL, EX_MODE_PATROL, EX_MODE_RAID, EX_MODE_RACE, EX_MODE_EARTHDEF, EX_MODE_FLEET, EX_MODE_COUNT } ex_mode_t;

typedef struct {
  int          size_class;    // 0..3
  ex_element_t element;
  ex_kb_layout_t layout;
  // bot customizer (proportions, 1.0 = default; percent stored as int 50..160)
  int height, bulk, head, shoulders, arm, leg;
  ex_element_t accent;        // accent colour element
  int vehicle;                // ex_vehicle_type index
  int player_class;           // 0..3 profile/class: gates vehicles + builds + stats
  int chassis;                // 0..3 frame variant: silhouette + transform style + glow
  int abil[3];                // ex_ability_id per slot
  int wmod, wmod2, wmod3;     // ex_weapon_mod_id x3 addon slots
  int detail;                 // greeble detail level 0..3
  int paint;                  // 0..7 primary paint scheme
  int finish;                 // 0 matte, 1 metallic, 2 glossy
  int signature;              // -1 = class default, else ex_signature_t (BACK component)
  int spoiler;                // vehicle: 0 none, 1 low wing, 2 GT wing, 3 whale tail
  int exhaust;                // vehicle: 0 hidden, 1 twin side, 2 quad stack, 3 big single
  int stance;                 // vehicle ride height 0 slammed .. 100 raised (50 stock)
  int rimglow;                // vehicle: 0 off, 1 accent-lit running trim
  int hero_donor;             // 0 = none, else a campaign character whose parts you wear
} ex_loadout_t;

// graphics settings the game applies
typedef struct {
  int   res_index;     // into a preset table
  int   render_scale;  // 100 / 75 / 50 percent
  int   show_fps;
  int   volume;        // 0..100
  int   sensitivity;   // 1..20 (x0.01)
  int   brightness;    // 40..160 percent (scene exposure)
  int   frame_cap;     // 0=vsync, else target fps (60/120/144/240/0=unlimited via -1)
  int   gui_scale;     // 50..100 percent HUD/menu size
  int   debug;         // debug/profiler overlay on/off
  int   quality;       // 0 mobile, 1 balanced, 2 high, 3 ultra -- fidelity/perf preset
} ex_gfx_settings_t;

#define EX_BUILDER_ROWS 5
#define EX_SETTINGS_ROWS 12
#define EX_CUSTOMIZE_ROWS 25
#define EX_RES_PRESETS 4

typedef struct {
  ex_game_state state;
  ex_game_state cutscene_return;   // where to go after a cutscene
  int cursor;

  ex_loadout_t      loadout;
  ex_gfx_settings_t gfx;
  ex_mode_t         mode;
  int               map_index;     // into a map table
  int               map_count;

  ex_campaign_t    *campaign;      // not owned
  int               mission_index;
  int               in_campaign;   // 1 if the pending match is a campaign mission

  // cutscene playback
  const char (*slides)[120];
  int   slide_count;
  int   slide_i;

  int start_requested;   // begin a match
  int quit_requested;    // exit app (from title)
  int to_menu_requested; // quit match -> back to title
  int rebinding;         // >=0: action index awaiting a new key (controls screen)
  int controls_reset;
  int controls_store;    // set when the user asks to reset bindings
  int layout_dirty;      // set when the keyboard layout changed
  int codex_index;       // which lore entry is shown
  int codex_read_max;    // furthest entry ever reached (easter-egg tracking)
  int hybrid_unlocked;   // read the codex to the END and the hybrid protocol opens
  int hybrid_on;         // CLONE-HYBRID VARIANTS: organic-machine dressing on the player
  // end-of-run results
  int   res_victory;     // 1 win, 0 defeat
  int   res_waves;       // waves reached
  long  res_credits;     // energon credits earned this match
  int   res_kills;
  long  res_score;
  long  res_best;        // high score (persisted)
  int   res_new_best;
  int settings_dirty;    // gfx changed -> game should apply
} ex_menu_t;

void ex_menu_init(ex_menu_t *m, ex_campaign_t *campaign, int map_count);
void ex_menu_nav(ex_menu_t *m, ex_nav_t nav);
void ex_menu_pause_toggle(ex_menu_t *m);

/* mouse support: set the cursor to a hovered row (clamped to the current
 * screen's row count) and optionally activate it (click). */
void ex_menu_set_cursor(ex_menu_t *m, int row);
void ex_menu_click(ex_menu_t *m);
int  ex_menu_row_count(const ex_menu_t *m);

/**
 * [ex_menu_begin_cutscene queue a slide sequence, returning to `after` when done]
 */
void ex_menu_begin_cutscene(ex_menu_t *m, const char slides[][120], int count, ex_game_state after);

/**
 * [ex_menu_line text for row i of the current screen; returns row count]
 */
int ex_menu_line(const ex_menu_t *m, int i, char *buf, int buflen);

const char* ex_size_class_name(int sc);
const char* ex_mode_name(ex_mode_t mode);
const char* ex_mode_desc(ex_mode_t mode);
const char* ex_map_name(int index);
const char* ex_map_desc(int index);
void ex_res_preset(int index, int *w, int *h);

#endif // EX_MENU_H
