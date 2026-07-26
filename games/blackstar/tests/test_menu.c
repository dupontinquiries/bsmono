/* test_menu.c -- full menu tree + campaign flow + cutscenes, headless. */
#include "menu.h"
#include "lore.h"
#include "campaign.h"
#include "robot_parts.h"
#include <stdio.h>
#include <assert.h>
#include <string.h>
#include <math.h>

int main(void) {
  ex_campaign_t camp; ex_campaign_init(&camp);
  printf("campaign: %d missions, unlocked=%d\n", camp.count, camp.unlocked);
  assert(camp.count > 0 && camp.count <= EX_MAX_MISSIONS &&
         camp.missions[0].intro_slides > 0 && camp.missions[0].outro_slides > 0);

  ex_menu_t m; ex_menu_init(&m, &camp, 2);
  assert(m.state == EX_GS_TITLE);

  // --- skirmish path: title->mode->map->builder->start ---
  m.cursor = 1; ex_menu_nav(&m, EX_NAV_CONFIRM);
  assert(m.state == EX_GS_MODE);
  ex_menu_nav(&m, EX_NAV_RIGHT);                 // mode -> survival
  assert(m.mode == EX_MODE_SURVIVAL);
  ex_menu_nav(&m, EX_NAV_CONFIRM);
  assert(m.state == EX_GS_MAP);
  ex_menu_nav(&m, EX_NAV_RIGHT);                 // map -> ice
  assert(m.map_index == 1);
  ex_menu_nav(&m, EX_NAV_CONFIRM);
  assert(m.state == EX_GS_BUILDER && m.in_campaign == 0);
  // configure + start
  ex_menu_nav(&m, EX_NAV_RIGHT);                 // size -> Heavy
  for (int i=0;i<EX_BUILDER_ROWS-1;i++) ex_menu_nav(&m, EX_NAV_DOWN);
  ex_menu_nav(&m, EX_NAV_CONFIRM);               // START
  assert(m.state == EX_GS_PLAYING && m.start_requested == 1);
  printf("skirmish: mode=%s map=%s start ok\n", ex_mode_name(m.mode), ex_map_name(m.map_index));

  // --- settings ---
  ex_menu_init(&m, &camp, 2);
  m.cursor = 2; ex_menu_nav(&m, EX_NAV_CONFIRM);
  assert(m.state == EX_GS_SETTINGS);
  int r0 = m.gfx.res_index;
  ex_menu_nav(&m, EX_NAV_RIGHT);                 // change resolution
  assert(m.gfx.res_index != r0 && m.settings_dirty == 1);
  ex_menu_nav(&m, EX_NAV_DOWN); ex_menu_nav(&m, EX_NAV_LEFT); // render scale down
  assert(m.gfx.render_scale != 100);
  int w,h; ex_res_preset(m.gfx.res_index,&w,&h);
  printf("settings: res=%dx%d scale=%d%%\n", w, h, m.gfx.render_scale);

  // --- campaign path with intro cutscene ---
  ex_menu_init(&m, &camp, 2);
  ex_menu_nav(&m, EX_NAV_CONFIRM);               // campaign
  assert(m.state == EX_GS_MISSIONS);
  ex_menu_nav(&m, EX_NAV_CONFIRM);               // mission 0 -> intro cutscene
  assert(m.state == EX_GS_CUTSCENE && m.slide_count > 0);
  printf("cutscene slide 0: \"%s\"\n", m.slides[0]);
  // advance through all slides -> builder
  int guard=0;
  while (m.state == EX_GS_CUTSCENE && guard++ < 20) ex_menu_nav(&m, EX_NAV_CONFIRM);
  assert(m.state == EX_GS_BUILDER && m.in_campaign == 1);
  // locked mission can't be entered
  ex_menu_init(&m, &camp, 2);
  ex_menu_nav(&m, EX_NAV_CONFIRM);               // campaign
  m.cursor = 3;                                   // mission 4 (locked, unlocked=0)
  ex_menu_nav(&m, EX_NAV_CONFIRM);
  assert(m.state == EX_GS_MISSIONS);              // stayed put (locked)
  printf("locked missions gated correctly\n");
  // completing unlocks next
  ex_campaign_complete(&camp, 0);
  assert(camp.unlocked == 1);

  // --- dvorak physical binds still correct ---
  ex_keybind_t kb; ex_keybind_preset(&kb, EX_LAYOUT_DVORAK);
  assert(ex_keybind_get(&kb, EX_ACT_FORWARD)==SDL_SCANCODE_E && ex_keybind_get(&kb,EX_ACT_RIGHT)==SDL_SCANCODE_F);

  // --- robot layout sanity ---
  ex_part_pose_t rp[EX_ROBOT_PARTS], vp[EX_ROBOT_PARTS];
  ex_robot_layout(rp,0,EX_BODY_PLAYER,1.0f,0.0f,0.0f,1);
  ex_robot_layout(vp,1,EX_BODY_PLAYER,1.0f,0.0f,0.0f,1);
  assert(vp[EX_PART_HEAD].offset[1] < rp[EX_PART_HEAD].offset[1]);


  // --- bot customizer + quit-match ---
  ex_menu_init(&m, &camp, 2);
  m.cursor=1; ex_menu_nav(&m,EX_NAV_CONFIRM);   // skirmish->mode
  ex_menu_nav(&m,EX_NAV_CONFIRM);               // ->map
  ex_menu_nav(&m,EX_NAV_CONFIRM);               // ->builder
  assert(m.state==EX_GS_BUILDER);
  // go to 'customize bot' row (index 3) and enter
  m.cursor=3; ex_menu_nav(&m,EX_NAV_CONFIRM);
  assert(m.state==EX_GS_CUSTOMIZE);
  int h0=m.loadout.height; ex_menu_nav(&m,EX_NAV_RIGHT);  // taller
  assert(m.loadout.height>h0);
  m.cursor=EX_CUSTOMIZE_ROWS-1; ex_menu_nav(&m,EX_NAV_CONFIRM); // done->builder
  assert(m.state==EX_GS_BUILDER);
  printf("customizer: height %d->%d, back to builder ok\n", h0, m.loadout.height);

  // quit-match from pause returns to title
  m.state=EX_GS_PLAYING; ex_menu_pause_toggle(&m);
  assert(m.state==EX_GS_PAUSED);
  m.cursor=1; ex_menu_nav(&m,EX_NAV_CONFIRM);   // 'quit match'
  assert(m.state==EX_GS_TITLE && m.to_menu_requested==1);
  printf("quit-match returns to title\n");

  // settings: volume/sensitivity/brightness adjust
  ex_menu_init(&m,&camp,2);
  m.cursor=2; ex_menu_nav(&m,EX_NAV_CONFIRM);   // settings
  m.cursor=3; int v0=m.gfx.volume; ex_menu_nav(&m,EX_NAV_RIGHT); assert(m.gfx.volume!=v0);
  m.cursor=5; int b0=m.gfx.brightness; ex_menu_nav(&m,EX_NAV_LEFT); assert(m.gfx.brightness!=b0);
  printf("settings: volume+brightness adjust ok\n");


  // --- rebindable keys ---
  ex_keybind_t rkb; ex_keybind_preset(&rkb, EX_LAYOUT_QWERTY);
  SDL_Scancode orig = ex_keybind_get(&rkb, EX_ACT_FIRE);
  ex_keybind_set(&rkb, EX_ACT_FIRE, SDL_SCANCODE_H);
  assert(ex_keybind_get(&rkb, EX_ACT_FIRE) == SDL_SCANCODE_H);
  assert(ex_keybind_get(&rkb, EX_ACT_FIRE) != orig);
  // reset restores the preset
  ex_keybind_preset(&rkb, EX_LAYOUT_QWERTY);
  assert(ex_keybind_get(&rkb, EX_ACT_FIRE) == orig);
  // controls screen enters rebinding on confirm over an action row
  ex_menu_init(&m, &camp, 2);
  m.state = EX_GS_CONTROLS; m.cursor = EX_ACT_JUMP;
  assert(m.rebinding == -1);
  ex_menu_nav(&m, EX_NAV_CONFIRM);
  assert(m.rebinding == EX_ACT_JUMP);   // now listening for a key
  // action names exist
  assert(ex_action_name(EX_ACT_FIRE) != 0);
  printf("rebindable keys: set/reset + controls listening ok\n");


  // --- codex ---
  assert(ex_lore_count() >= 6);
  for(int i=0;i<ex_lore_count();i++){ const ex_lore_entry_t*le=ex_lore_entry(i);
    assert(le && le->title && le->line_count>0); }
  ex_menu_init(&m,&camp,2); m.state=EX_GS_TITLE; m.cursor=4;
  ex_menu_nav(&m,EX_NAV_CONFIRM);   // enter codex
  assert(m.state==EX_GS_CODEX);
  int c0=m.codex_index; ex_menu_nav(&m,EX_NAV_RIGHT); assert(m.codex_index!=c0);
  ex_menu_nav(&m,EX_NAV_BACK); assert(m.state==EX_GS_TITLE);
  printf("codex: %d entries, menu nav ok\n", ex_lore_count());


  // --- results screen ---
  ex_menu_init(&m,&camp,2);
  m.state=EX_GS_RESULTS; m.res_victory=1; m.res_waves=8; m.res_score=12000;
  ex_menu_nav(&m, EX_NAV_CONFIRM);          // continue -> builder
  assert(m.state==EX_GS_BUILDER);
  printf("results screen nav ok\n");

  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
