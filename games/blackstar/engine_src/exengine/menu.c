#include "menu.h"
#include "pclass.h"
#include "chassis.h"
#include "signature.h"
#include "lore.h"
#include <stdio.h>
#include <string.h>

static const char *MAP_IDS[]  = { "wastes", "frostline", "emberpit", "vault", "foundry", "shield_edge", "mining", "outer_rim", "cybertron", "reclaim", "trench", "pluto", "beach", "mars", "core", "kuiper", "unicron", "europa", "io", "titan", "glaciem", "carrion", "helix", "enceladus" };
static const char *MAP_NAMES[] = { "rusted wastes", "frostline", "ember pit", "crystal vault", "the foundry", "shield boundary", "mining station", "the outer rim", "cybertron", "the reclaim", "the trench", "pluto station", "the beach", "mars dig", "the core", "kuiper belt", "unicron", "europa", "io", "titan", "glaciem", "carrion drift", "helix", "enceladus geysers" };
static const char *MAP_DESC[] = {
  "outer colonies - scoured earth",
  "the frozen reaches - ice-clone country",
  "volcanic depths - where magma clones forge",
  "the sealed archive - crystal light",
  "war-foundry - hazard and steel",
  "the boundary lattice - edge of the system",
  "deep mining rig - drifting ore",
  "the outer rim - low-gravity debris fields",
  "cybertron - the homeworld, at last",
  "the reclaim - lanes through organic-metal jungle",
  "the trench - beneath the pacific, the drowned wound",
  "pluto station - the outer containment line, the system's edge",
  "the beach - Earth's shore, torn open as energon breaches",
  "mars dig - a cybertron fragment, torn out of the red rock",
  "the core - cybertron's molten heart; the forge of the matrix",
  "kuiper belt - blackstar's scavenger fleet, adrift in the asteroids",
  "unicron - inside the god-machine; the heart pulses at the center",
  "europa - jupiter's ocean moon; something answered from below the ice",
  "io - the tide-forges; where the magma-clone fires never cool",
  "titan - saturn's haze moon; the only world where fuel falls as rain",
  "glaciem - the ice-clones' vault-world; carved down, preserved in clear ice",
  "carrion drift - the scavenger-clones' wreck-pile world; worn salvage is status",
  "helix - the architect's garden-world; bodies perfected past identity",
};
const char* ex_map_desc(int i){ return (i>=0 && i<(int)(sizeof(MAP_DESC)/sizeof(MAP_DESC[0]))) ? MAP_DESC[i] : ""; }
#define MAP_TABLE_COUNT ((int)(sizeof(MAP_IDS)/sizeof(MAP_IDS[0])))

static const struct { int w, h; } RES[EX_RES_PRESETS] = {
  {1280,720}, {1600,900}, {1920,1080}, {2560,1440}
};

const char* ex_map_name(int i){ return (i>=0 && i<MAP_TABLE_COUNT) ? MAP_NAMES[i] : "?"; }
const char* ex_mode_name(ex_mode_t m){
  switch(m){ case EX_MODE_SURVIVAL: return "survival";
             case EX_MODE_STALKERS: return "stalkers";
             case EX_MODE_DEFEND:   return "defense";
             case EX_MODE_ESCALATION: return "escalation";
             case EX_MODE_ORBRUSH: return "orb rush";
             case EX_MODE_SKYDUEL: return "sky duel";
             case EX_MODE_PATROL: return "patrol";
             case EX_MODE_RAID: return "raid";
             case EX_MODE_RACE: return "time trial";
             case EX_MODE_EARTHDEF: return "defense of earth";
             case EX_MODE_FLEET: return "blackstar's fleet";
             default: return "arena fight"; }
}
const char* ex_mode_desc(ex_mode_t m){
  switch(m){ case EX_MODE_SURVIVAL: return "endless waves - last as long as you can";
             case EX_MODE_STALKERS: return "endless, in fog - they freeze when watched, close in when not";
             case EX_MODE_DEFEND:   return "hold the line - defend the core against endless waves";
             case EX_MODE_ESCALATION: return "endless escalation - each wave you clear forges you stronger on energon";
             case EX_MODE_ORBRUSH: return "orb rush - grifball melee brawl: carry/dribble the Orb into the enemy forge. no guns, power axes only. vehicle ram legal.";
             case EX_MODE_SKYDUEL: return "sky duel - a Starscream free-fall brawl: trade blows while plummeting. beat him before you hit the ground or you both crater.";
             case EX_MODE_PATROL: return "patrol - roam the planets (Destiny-style): fight roaming foes, trigger public events for LOOT, space-bridge between worlds. [B] to bridge.";
             case EX_MODE_RACE: return "time trial - a calm, enemy-free run + parkour course through a beautiful landscape. hit every checkpoint fast; the glowing gates parkour up high.";
             case EX_MODE_EARTHDEF: return "defense of earth - a 3-phase squad raid vs warping HIJACK clones: moon skirmishes, the mars line, then billy routes you THE SUN. fight as clone (balanced) or transformer (harder, unique role).";
             case EX_MODE_FLEET: return "the kuiper warlord raids: DRIFTCUTTERS steal your energon, CORSAIRS harpoon-tether you, HULLBREAKERS crack the deck. survive 3 boarding waves under his dreadnought.";
             case EX_MODE_RAID: return "raid - the endgame: 3 gated phases (Descent -> Gauntlet -> Unicron's Heart). clear every encounter to advance; kill Unicron to win.";
             default: return "8 escalating waves - clear them all"; }
}
void ex_res_preset(int i, int *w, int *h){ if(i<0)i=0; if(i>=EX_RES_PRESETS)i=EX_RES_PRESETS-1; *w=RES[i].w; *h=RES[i].h; }

const char* ex_size_class_name(int sc){
  switch(sc){case 0:return "Mini";case 1:return "Standard";case 2:return "Heavy";case 3:return "Titan";default:return "?";}
}

void ex_menu_init(ex_menu_t *m, ex_campaign_t *campaign, int map_count)
{
  memset(m,0,sizeof(*m));
  m->state = EX_GS_TITLE;
  m->rebinding = -1;   // -1 = not rebinding (0 would disable mouse + look like a live rebind)
  m->loadout.size_class = 1;
  m->loadout.element = EX_ELEM_ICE;
  m->loadout.layout = EX_LAYOUT_QWERTY;
  m->loadout.height=100; m->loadout.bulk=100; m->loadout.head=100;
  m->loadout.shoulders=100; m->loadout.arm=100; m->loadout.leg=100;
  m->loadout.accent = EX_ELEM_ELECTRIC; m->loadout.paint = 0; m->loadout.finish = 1;
  m->loadout.spoiler = 0; m->loadout.exhaust = 1; m->loadout.stance = 50; m->loadout.rimglow = 1;
  m->loadout.vehicle = 0; m->loadout.abil[0]=3; m->loadout.abil[1]=4; m->loadout.abil[2]=1; // shield,dash,hover
  m->loadout.wmod = 0; m->loadout.wmod2 = 0; m->loadout.wmod3 = 0; m->loadout.detail = 3;
  m->loadout.signature = -1;   // -1 = use the class's default signature component
  m->loadout.hero_donor = 0;
  m->codex_read_max = 0; m->hybrid_unlocked = 0; m->hybrid_on = 0;
  m->gfx.res_index = 0; m->gfx.render_scale = 100; m->gfx.show_fps = 1; m->gfx.debug = 0;
  m->gfx.volume = 35; m->gfx.sensitivity = 10; m->gfx.brightness = 100;
  m->gfx.quality = 2;   // high by default; mobile users drop it to 0
  m->mode = EX_MODE_ARENA_FIGHT;
  m->campaign = campaign;
  m->map_count = map_count > 0 ? map_count : MAP_TABLE_COUNT;
}

void ex_menu_begin_cutscene(ex_menu_t *m, const char slides[][120], int count, ex_game_state after)
{
  m->slides = slides; m->slide_count = count; m->slide_i = 0;
  m->cutscene_return = after;
  m->state = EX_GS_CUTSCENE;
}

static void wrap(int *v, int lo, int hi){ if(*v<lo)*v=hi; if(*v>hi)*v=lo; }

static void builder_option(ex_menu_t *m, int dir)
{
  switch(m->cursor){
    case 0:{ int v=m->loadout.size_class+dir; wrap(&v,0,3); m->loadout.size_class=v; break; }
    case 1:{ int v=(int)m->loadout.element+dir; if(v<EX_ELEM_ICE)v=EX_ELEM_COUNT-1; if(v>=EX_ELEM_COUNT)v=EX_ELEM_ICE; m->loadout.element=(ex_element_t)v; break; }
    case 2: m->loadout.layout = m->loadout.layout==EX_LAYOUT_QWERTY?EX_LAYOUT_DVORAK:EX_LAYOUT_QWERTY; break;
    default: break;
  }
}

static int clampi(int v,int lo,int hi){ return v<lo?lo:(v>hi?hi:v); }
static const char *VEH_NAMES[16]={"racer","tank","jet","hover","insect","spider","dragon","dino","raptor","mech","hover-cycle","gunship","scorpion","wolf","hawk","crab"};
static const char *ABIL_NAMES[7]={"none","hover","turret","shield","dash","cloak","overclock"};
static const char *WMOD_NAMES[17]={"none","long barrel","scope","ext mag","underbarrel","stabilizer","amplifier","chain coil","energon cell","rapid coil","pierce core","scatter barrel","ripple coil","cryo rounds","flak core","plasma core","suppress core"};
static const char *DETAIL_NAMES[4]={"low","medium","high","ultra"};
// HERO PARTS: wear pieces from a campaign character -- their signature component + accent
// + a proportion silhouette. A donor is a curated loadout preset drawn from the cast.
typedef struct { const char *name; int sig, accent, height, bulk, shoulders, arm, leg; } ex_hero_donor_t;
static const ex_hero_donor_t EX_HERO_DONORS[] = {
  { "none",            -1, -1, 0,0,0,0,0 },
  { "Witwicky suit",    8,  3, 95, 95, 100, 105, 100 },   // recon fins, the nimble old suit
  { "Rodimus",          6,  1, 108,100,110,105,110 },     // solar core, tall + flame
  { "the Watch bearer", 5,  0, 112,108,120,100,108 },     // sentinel halo, broad + heavy
  { "Megatron (truce)", 1,  7, 115,120,125,110,105 },     // heavy cannon, hulking
  { "Cryotek Prime",    2,  0, 118,122,128,105,110 },     // crest, glacier-massive
  { "Blackstar",        4,  6, 104,110,112,118,102 },     // harpoon, long-armed scavenger
  { "Jhiaxus",          3,  4, 106, 96,108,104,116 },     // cable, tall + spindly-elegant
};
#define EX_HERO_DONOR_COUNT ((int)(sizeof(EX_HERO_DONORS)/sizeof(EX_HERO_DONORS[0])))
static void apply_hero_donor(ex_menu_t *m, int d){
  if (d <= 0 || d >= EX_HERO_DONOR_COUNT){ m->loadout.hero_donor = 0; return; }
  const ex_hero_donor_t *h = &EX_HERO_DONORS[d];
  m->loadout.hero_donor = d;
  m->loadout.signature = h->sig;
  m->loadout.accent = (ex_element_t)h->accent;
  m->loadout.height=h->height; m->loadout.bulk=h->bulk; m->loadout.shoulders=h->shoulders;
  m->loadout.arm=h->arm; m->loadout.leg=h->leg;
  m->loadout.bulk = ex_class_clamp_bulk((ex_class_t)m->loadout.player_class, m->loadout.bulk);
}
static void settings_option(ex_menu_t *m, int dir)
{
  switch(m->cursor){
    case 0:{ int v=m->gfx.res_index+dir; wrap(&v,0,EX_RES_PRESETS-1); m->gfx.res_index=v; m->settings_dirty=1; break; }
    case 1:{ int s=m->gfx.render_scale + dir*25; if(s<50)s=150; if(s>150)s=50; m->gfx.render_scale=s; m->settings_dirty=1; break; }
    case 2: m->gfx.show_fps = !m->gfx.show_fps; break;
    case 3: m->gfx.volume = clampi(m->gfx.volume + dir*5, 0, 100); m->settings_dirty=1; break;
    case 4: m->gfx.sensitivity = clampi(m->gfx.sensitivity + dir, 1, 20); m->settings_dirty=1; break;
    case 5: m->gfx.brightness = clampi(m->gfx.brightness + dir*10, 40, 160); m->settings_dirty=1; break;
    case 6:{ // frame cap: vsync(0) -> 60 -> 120 -> 144 -> 240 -> unlimited(-1)
      static const int caps[6]={0,60,120,144,240,-1};
      int idx=0; for(int k=0;k<6;k++) if(caps[k]==m->gfx.frame_cap){ idx=k; break; }
      idx=(idx+dir+6)%6; m->gfx.frame_cap=caps[idx]; m->settings_dirty=1; break; }
    case 7: m->gfx.gui_scale = clampi(m->gfx.gui_scale + dir*5, 25, 100); m->settings_dirty=1; break;
    case 8: m->gfx.debug = !m->gfx.debug; break;
    case 9:{ int v=m->gfx.quality+dir; wrap(&v,0,3); m->gfx.quality=v;
             // sensible preset: the tier picks a render scale too (user can still
             // override render scale afterward on its own row).
             static const int rs[4]={50,75,100,100};  // mobile,balanced,high,ultra
             m->gfx.render_scale = rs[v]; m->settings_dirty=1; break; }
    case 10: if (m->campaign) m->campaign->unlocked = m->campaign->count - 1; break;   // testing: open every chapter
    default: break;
  }
}
static void customize_option(ex_menu_t *m, int dir){
  int *f[6]={&m->loadout.height,&m->loadout.bulk,&m->loadout.head,&m->loadout.shoulders,&m->loadout.arm,&m->loadout.leg};
  if(m->cursor>=0 && m->cursor<6){ *f[m->cursor]=clampi(*f[m->cursor]+dir*5,50,160);
    if(m->cursor==1) m->loadout.bulk=ex_class_clamp_bulk((ex_class_t)m->loadout.player_class, m->loadout.bulk); } // bulk gated by class
  else if(m->cursor==6){ int v=(int)m->loadout.accent+dir; if(v<EX_ELEM_ICE)v=EX_ELEM_COUNT-1; if(v>=EX_ELEM_COUNT)v=EX_ELEM_ICE; m->loadout.accent=(ex_element_t)v; }
  else if(m->cursor==7){ // vehicle gated to the class's allowed forms
    m->loadout.vehicle=ex_class_cycle_vehicle((ex_class_t)m->loadout.player_class, m->loadout.vehicle, dir>0?1:-1); }
  else if(m->cursor>=8 && m->cursor<=10){ int sl=m->cursor-8; int v=m->loadout.abil[sl]+dir; if(v<0)v=6; if(v>6)v=0; m->loadout.abil[sl]=v; }
  else if(m->cursor==11){ int v=m->loadout.wmod+dir; if(v<0)v=16; if(v>16)v=0; m->loadout.wmod=v; }
  else if(m->cursor==17){ int v=m->loadout.wmod2+dir; if(v<0)v=16; if(v>16)v=0; m->loadout.wmod2=v; }
  else if(m->cursor==18){ int v=m->loadout.wmod3+dir; if(v<0)v=16; if(v>16)v=0; m->loadout.wmod3=v; }
  else if(m->cursor==19){ int v=m->loadout.hero_donor+dir; if(v<0)v=EX_HERO_DONOR_COUNT-1; if(v>=EX_HERO_DONOR_COUNT)v=0; apply_hero_donor(m, v); }
  else if(m->cursor==12){ // class/profile: switching re-gates the vehicle + seeds proportions
    int v=m->loadout.player_class+dir; if(v<0)v=EX_CLASS_COUNT-1; if(v>=EX_CLASS_COUNT)v=0; m->loadout.player_class=v;
    const ex_class_info_t *ci=ex_class_info((ex_class_t)v);
    if(!ex_class_allows_vehicle((ex_class_t)v, m->loadout.vehicle))
      m->loadout.vehicle=ci->vehicles[0];
    m->loadout.bulk   = ex_class_clamp_bulk((ex_class_t)v, ci->base_bulk);  // start at the class's default build
    m->loadout.height = ci->base_height; }
  else if(m->cursor==13){ int v=m->loadout.chassis+dir; if(v<0)v=EX_CHASSIS_COUNT-1; if(v>=EX_CHASSIS_COUNT)v=0; m->loadout.chassis=v; }
  else if(m->cursor==14){ int v=m->loadout.paint+dir; if(v<0)v=7; if(v>7)v=0; m->loadout.paint=v; }
  else if(m->cursor==15){ int v=m->loadout.finish+dir; if(v<0)v=2; if(v>2)v=0; m->loadout.finish=v; }
  else if(m->cursor==16){ int v=m->loadout.signature+dir; if(v<-1)v=EX_SIG_COUNT-1; if(v>=EX_SIG_COUNT)v=-1; m->loadout.signature=v; }
  else if(m->cursor==20){ int v=m->loadout.spoiler+dir; if(v<0)v=3; if(v>3)v=0; m->loadout.spoiler=v; }
  else if(m->cursor==21){ int v=m->loadout.exhaust+dir; if(v<0)v=3; if(v>3)v=0; m->loadout.exhaust=v; }
  else if(m->cursor==22){ m->loadout.stance=clampi(m->loadout.stance+dir*10,0,100); }
  else if(m->cursor==23){ m->loadout.rimglow=!m->loadout.rimglow; }
}

static void cycle(ex_menu_t *m, int n, ex_nav_t nav){
  if(nav==EX_NAV_UP)   m->cursor=(m->cursor+n-1)%n;
  if(nav==EX_NAV_DOWN) m->cursor=(m->cursor+1)%n;
}

void ex_menu_nav(ex_menu_t *m, ex_nav_t nav)
{
  switch(m->state){
    case EX_GS_TITLE: {
      cycle(m,6,nav); // campaign, skirmish, settings, controls, codex, quit
      if(nav==EX_NAV_CONFIRM){
        switch(m->cursor){
          case 0: m->state=EX_GS_MISSIONS; m->cursor=0; break;
          case 1: m->state=EX_GS_MODE;     m->cursor=0; break;
          case 2: m->state=EX_GS_SETTINGS; m->cursor=0; break;
          case 3: m->state=EX_GS_CONTROLS; m->cursor=0; break;
          case 4: m->state=EX_GS_CODEX;    m->codex_index=0; break;   // read on, archivist
          case 5: m->quit_requested=1; break;
        }
      }
      break;
    }
    case EX_GS_MODE: {
      if(nav==EX_NAV_LEFT||nav==EX_NAV_RIGHT){ int v=(int)m->mode+(nav==EX_NAV_RIGHT?1:-1); wrap(&v,0,EX_MODE_COUNT-1); m->mode=(ex_mode_t)v; }
      if(nav==EX_NAV_BACK){ m->state=EX_GS_TITLE; m->cursor=1; }
      if(nav==EX_NAV_CONFIRM){ m->state=EX_GS_MAP; m->cursor=0; }
      break;
    }
    case EX_GS_MAP: {
      if(nav==EX_NAV_LEFT||nav==EX_NAV_RIGHT){ int v=m->map_index+(nav==EX_NAV_RIGHT?1:-1); wrap(&v,0,m->map_count-1); m->map_index=v; }
      if(nav==EX_NAV_BACK){ m->state=EX_GS_MODE; }
      if(nav==EX_NAV_CONFIRM){ m->in_campaign=0; m->state=EX_GS_BUILDER; m->cursor=0; }
      break;
    }
    case EX_GS_MISSIONS: {
      int n = m->campaign ? m->campaign->count : 1;
      cycle(m,n,nav);
      if(nav==EX_NAV_BACK){ m->state=EX_GS_TITLE; m->cursor=0; }
      if(nav==EX_NAV_CONFIRM && m->campaign){
        if(m->cursor <= m->campaign->unlocked){
          m->mission_index = m->cursor;
          m->in_campaign = 1;
          ex_mission_t *ms = &m->campaign->missions[m->cursor];
          ex_menu_begin_cutscene(m, ms->intro, ms->intro_slides, EX_GS_BUILDER);
        }
      }
      break;
    }
    case EX_GS_CUTSCENE: {
      if(nav==EX_NAV_CONFIRM || nav==EX_NAV_BACK){
        m->slide_i++;
        if(m->slide_i >= m->slide_count || nav==EX_NAV_BACK){
          m->state = m->cutscene_return;
          m->cursor = 0;
          if(m->state==EX_GS_PLAYING) m->start_requested=1;
        }
      }
      break;
    }
    case EX_GS_BUILDER: {
      cycle(m,EX_BUILDER_ROWS,nav);
      if(nav==EX_NAV_LEFT)  builder_option(m,-1);
      if(nav==EX_NAV_RIGHT) builder_option(m,+1);
      if(nav==EX_NAV_BACK){ m->state = m->in_campaign?EX_GS_MISSIONS:EX_GS_MAP; m->cursor=0; }
      if(nav==EX_NAV_CONFIRM){
        if(m->cursor==EX_BUILDER_ROWS-2){ m->state=EX_GS_CUSTOMIZE; m->cursor=0; } // 'customize'
        else if(m->cursor==EX_BUILDER_ROWS-1){ m->start_requested=1; m->state=EX_GS_PLAYING; } // START
        else builder_option(m,+1);
      }
      break;
    }
    case EX_GS_CUSTOMIZE: {
      cycle(m,EX_CUSTOMIZE_ROWS,nav);
      if(nav==EX_NAV_LEFT)  customize_option(m,-1);
      if(nav==EX_NAV_RIGHT) customize_option(m,+1);
      if(nav==EX_NAV_BACK || (nav==EX_NAV_CONFIRM && m->cursor==EX_CUSTOMIZE_ROWS-1)){ m->state=EX_GS_BUILDER; m->cursor=0; }
      break;
    }
    case EX_GS_SETTINGS: {
      cycle(m,EX_SETTINGS_ROWS,nav);
      if(nav==EX_NAV_LEFT)  settings_option(m,-1);
      if(nav==EX_NAV_RIGHT) settings_option(m,+1);
      if(nav==EX_NAV_BACK || (nav==EX_NAV_CONFIRM && m->cursor==EX_SETTINGS_ROWS-1)){ m->state=EX_GS_TITLE; m->cursor=2; }
      break;
    }
    case EX_GS_CONTROLS: {
      // rows: EX_ACT_COUNT action bindings, then layout, reset, store, back
      int nrows = EX_ACT_COUNT + 4;
      if (m->rebinding >= 0) break; // game captures the key; ignore nav
      cycle(m, nrows, nav);
      int layout_row = EX_ACT_COUNT, reset_row = EX_ACT_COUNT+1, store_row = EX_ACT_COUNT+2, back_row = EX_ACT_COUNT+3;
      if (nav==EX_NAV_LEFT || nav==EX_NAV_RIGHT) {
        if (m->cursor==layout_row) { m->loadout.layout = m->loadout.layout==EX_LAYOUT_QWERTY?EX_LAYOUT_DVORAK:EX_LAYOUT_QWERTY; m->layout_dirty=1; }
      }
      if (nav==EX_NAV_CONFIRM) {
        if (m->cursor < EX_ACT_COUNT) m->rebinding = m->cursor;        // capture next key
        else if (m->cursor==layout_row) { m->loadout.layout = m->loadout.layout==EX_LAYOUT_QWERTY?EX_LAYOUT_DVORAK:EX_LAYOUT_QWERTY; m->layout_dirty=1; }
        else if (m->cursor==reset_row) m->controls_reset = 1;
        else if (m->cursor==store_row) m->controls_store = 1;
        else if (m->cursor==back_row) { m->state=EX_GS_TITLE; m->cursor=3; }
      }
      if (nav==EX_NAV_BACK) { m->state=EX_GS_TITLE; m->cursor=3; }
      break;
    }
    case EX_GS_CODEX: {
      int n = ex_lore_count();
      // THE EASTER EGG: read every entry to the END and the last page becomes a switch.
      if (m->codex_index == n-1 && m->hybrid_unlocked && nav==EX_NAV_CONFIRM){
        m->hybrid_on = !m->hybrid_on;                       // HYBRID PROTOCOL toggles
      } else {
        if(nav==EX_NAV_LEFT)  m->codex_index = (m->codex_index + n - 1) % n;
        if(nav==EX_NAV_RIGHT || nav==EX_NAV_CONFIRM) m->codex_index = (m->codex_index + 1) % n;
      }
      if (m->codex_index + 1 > m->codex_read_max) m->codex_read_max = m->codex_index + 1;
      if (m->codex_read_max >= n) m->hybrid_unlocked = 1;   // the archive rewards patience
      if(nav==EX_NAV_BACK){ m->state=EX_GS_TITLE; m->cursor=4; }
      break;
    }
    case EX_GS_RESULTS: {
      if (nav==EX_NAV_CONFIRM || nav==EX_NAV_BACK) { m->state = EX_GS_BUILDER; m->cursor = 0; }
      break;
    }
    case EX_GS_PAUSED: {
      cycle(m,3,nav); // resume, quit match, quit game
      if(nav==EX_NAV_CONFIRM){
        if(m->cursor==0) m->state=EX_GS_PLAYING;
        else if(m->cursor==1){ m->to_menu_requested=1; m->state=EX_GS_TITLE; m->cursor=0; }
        else m->quit_requested=1;
      }
      if(nav==EX_NAV_BACK) m->state=EX_GS_PLAYING;
      break;
    }
    default: break;
  }
}

void ex_menu_pause_toggle(ex_menu_t *m){
  if(m->state==EX_GS_PLAYING){ m->state=EX_GS_PAUSED; m->cursor=0; }
  else if(m->state==EX_GS_PAUSED) m->state=EX_GS_PLAYING;
}

int ex_menu_line(const ex_menu_t *m, int i, char *buf, int buflen)
{
  const char *sel = (i==m->cursor)?"> ":"  ";
  switch(m->state){
    case EX_GS_TITLE:{
      const char *r[6]={"campaign","skirmish","settings","controls","codex","quit"};
      if(i>=0&&i<6) snprintf(buf,buflen,"%s%s",sel,r[i]); return 6;
    }
    case EX_GS_MODE:
      if(i==0) snprintf(buf,buflen,"> mode  < %s >",ex_mode_name(m->mode));
      else if(i==1) snprintf(buf,buflen,"  %s", ex_mode_desc(m->mode));
      else snprintf(buf,buflen,"  enter_confirm  back");
      return 3;
    case EX_GS_MAP:
      if(i==0) snprintf(buf,buflen,"> destination < %s >",ex_map_name(m->map_index));
      else if(i==1) snprintf(buf,buflen,"  %s", ex_map_desc(m->map_index));
      else snprintf(buf,buflen,"  enter to travel  -  back");
      return 3;
    case EX_GS_MISSIONS:{
      int n=m->campaign?m->campaign->count:0;
      if(i>=0&&i<n){
        int locked = m->campaign && i > m->campaign->unlocked;
        if (locked && i >= 13)   /* the bridge set stays a MYSTERY until charted */
          snprintf(buf,buflen,"%s?? _ beyond the bridge (chart it from patrol)",sel);
        else
          snprintf(buf,buflen,"%s%s%s",sel,m->campaign->missions[i].title, locked?" _locked":"");
      }
      return n;
    }
    case EX_GS_CUTSCENE:
      if(i==0 && m->slide_i < m->slide_count) snprintf(buf,buflen,"%s",m->slides[m->slide_i]);
      else if(i==1) snprintf(buf,buflen,"[enter] continue");
      return 2;
    case EX_GS_BUILDER:
      switch(i){
        case 0: snprintf(buf,buflen,"%ssize    < %s >",sel,ex_size_class_name(m->loadout.size_class)); break;
        case 1: snprintf(buf,buflen,"%selement < %s >",sel,ex_element_name(m->loadout.element)); break;
        case 2: snprintf(buf,buflen,"%skeys    < %s >",sel,m->loadout.layout==EX_LAYOUT_DVORAK?"dvorak":"qwerty"); break;
        case 3: snprintf(buf,buflen,"%scustomize bot >",sel); break;
        case 4: snprintf(buf,buflen,"%sSTART",sel); break;
      }
      return EX_BUILDER_ROWS;
    case EX_GS_CUSTOMIZE:{
      switch(i){
        case 0: snprintf(buf,buflen,"%sheight    < %d%% >",sel,m->loadout.height); break;
        case 1: snprintf(buf,buflen,"%sbulk      < %d%% >",sel,m->loadout.bulk); break;
        case 2: snprintf(buf,buflen,"%shead      < %d%% >",sel,m->loadout.head); break;
        case 3: snprintf(buf,buflen,"%sshoulders < %d%% >",sel,m->loadout.shoulders); break;
        case 4: snprintf(buf,buflen,"%sarms      < %d%% >",sel,m->loadout.arm); break;
        case 5: snprintf(buf,buflen,"%slegs      < %d%% >",sel,m->loadout.leg); break;
        case 6: snprintf(buf,buflen,"%saccent    < %s >",sel,ex_element_name(m->loadout.accent)); break;
        case 7: snprintf(buf,buflen,"%svehicle   < %s >",sel,VEH_NAMES[m->loadout.vehicle%16]); break;
        case 8: snprintf(buf,buflen,"%sability 1 < %s >",sel,ABIL_NAMES[m->loadout.abil[0]%7]); break;
        case 9: snprintf(buf,buflen,"%sability 2 < %s >",sel,ABIL_NAMES[m->loadout.abil[1]%7]); break;
        case 10:snprintf(buf,buflen,"%sability 3 < %s >",sel,ABIL_NAMES[m->loadout.abil[2]%7]); break;
        case 11:snprintf(buf,buflen,"%sweapon addon 1 < %s >",sel,WMOD_NAMES[m->loadout.wmod%17]); break;
        case 12:snprintf(buf,buflen,"%sprofile  < %s >",sel,ex_class_info((ex_class_t)m->loadout.player_class)->name); break;
        case 13:snprintf(buf,buflen,"%sframe    < %s >",sel,ex_chassis_info((ex_chassis_t)m->loadout.chassis)->name); break;
        case 14:{ static const char *PN[8]={"steel","crimson","cobalt","emerald","amber","violet","gunmetal","pearl"};
                  snprintf(buf,buflen,"%spaint    < %s >",sel,PN[m->loadout.paint&7]); break; }
        case 15:{ static const char *FN[3]={"matte","metallic","glossy"};
                  snprintf(buf,buflen,"%sfinish   < %s >",sel,FN[m->loadout.finish%3]); break; }
        case 16:{ static const char *SN[EX_SIG_COUNT]={"wing-cannon","heavy cannon","crest","cable","harpoon","sentinel halo","solar core","aegis","recon fins","siege mortar","razor fans"};
                  if(m->loadout.signature<0) snprintf(buf,buflen,"%ssignature < class default >",sel);
                  else snprintf(buf,buflen,"%ssignature < %s >",sel,SN[m->loadout.signature%EX_SIG_COUNT]); break; }
        case 17:snprintf(buf,buflen,"%sweapon addon 2 < %s >",sel,WMOD_NAMES[m->loadout.wmod2%17]); break;
        case 18:snprintf(buf,buflen,"%sweapon addon 3 < %s >",sel,WMOD_NAMES[m->loadout.wmod3%17]); break;
        case 19:snprintf(buf,buflen,"%shero parts < %s >",sel,EX_HERO_DONORS[m->loadout.hero_donor%EX_HERO_DONOR_COUNT].name); break;
        case 20:{ static const char *SPN[4]={"none","low wing","GT wing","whale tail"};
                  snprintf(buf,buflen,"%sspoiler  < %s >",sel,SPN[m->loadout.spoiler&3]); break; }
        case 21:{ static const char *EXN[4]={"hidden","twin side","quad stack","big single"};
                  snprintf(buf,buflen,"%sexhaust  < %s >",sel,EXN[m->loadout.exhaust&3]); break; }
        case 22:snprintf(buf,buflen,"%sstance   < %d%% >",sel,m->loadout.stance); break;
        case 23:snprintf(buf,buflen,"%srim glow < %s >",sel,m->loadout.rimglow?"accent-lit":"off"); break;
        case 24:snprintf(buf,buflen,"%sdone",sel); break;
      }
      return EX_CUSTOMIZE_ROWS;
    }
    case EX_GS_SETTINGS:{
      int w,h; ex_res_preset(m->gfx.res_index,&w,&h);
      switch(i){
        case 0: snprintf(buf,buflen,"%sresolution   < %dx%d >",sel,w,h); break;
        case 1: snprintf(buf,buflen,"%srender scale < %d%%%s >",sel,m->gfx.render_scale, m->gfx.render_scale>100?" SSAA":""); break;
        case 2: snprintf(buf,buflen,"%sshow fps     < %s >",sel,m->gfx.show_fps?"on":"off"); break;
        case 3: snprintf(buf,buflen,"%svolume       < %d >",sel,m->gfx.volume); break;
        case 4: snprintf(buf,buflen,"%ssensitivity  < %d >",sel,m->gfx.sensitivity); break;
        case 5: snprintf(buf,buflen,"%sbrightness   < %d%% >",sel,m->gfx.brightness); break;
        case 6:{ const char *fc = m->gfx.frame_cap==0?"vsync":m->gfx.frame_cap<0?"unlimited":NULL;
                 if(fc) snprintf(buf,buflen,"%sframe cap    < %s >",sel,fc);
                 else   snprintf(buf,buflen,"%sframe cap    < %d fps >",sel,m->gfx.frame_cap); break; }
        case 7: snprintf(buf,buflen,"%sgui scale    < %d%% >",sel,m->gfx.gui_scale); break;
        case 8: snprintf(buf,buflen,"%sdebug/prof   < %s >",sel,m->gfx.debug?"on":"off"); break;
        case 9:{ static const char *qn[4]={"mobile","balanced","high","ultra"};
                 snprintf(buf,buflen,"%squality      < %s >",sel,qn[m->gfx.quality&3]); break; }
        case 10:{ int un = m->campaign ? m->campaign->unlocked+1 : 0, ct = m->campaign ? m->campaign->count : 0;
                 snprintf(buf,buflen,"%scampaign     < %d/%d open%s >",sel,un,ct,(un>=ct)?" (ALL)":" - press to unlock all"); break; }
        case 11: snprintf(buf,buflen,"%sback",sel); break;
      }
      return EX_SETTINGS_ROWS;
    }
    case EX_GS_CONTROLS:{
      const char *rows[7]={
        "move     . o e u  (or w a s d)",
        "jump     space",
        "transform  r",
        "fire     g  /  left mouse",
        "element  b",
        "pause    esc",
        ""};
      if(i==0) snprintf(buf,buflen,"layout < %s >  (arrows toggle)",m->loadout.layout==EX_LAYOUT_DVORAK?"dvorak":"qwerty");
      else if(i>=1&&i<=6) snprintf(buf,buflen,"%s",rows[i-1]);
      return 7;
    }
    case EX_GS_CODEX:{
      const ex_lore_entry_t *le = ex_lore_entry(m->codex_index);
      int n = ex_lore_count();
      if (m->codex_index == n-1 && !m->hybrid_unlocked){
        snprintf(buf,buflen,"[ sealed record ]"); return 1;
      }
      if (m->codex_index == n-1 && m->hybrid_unlocked){
        // the reward, hiding at the end of the archive
        snprintf(buf,buflen,"%s   [HYBRID PROTOCOL < %s > -- enter]", le?le->title:"", m->hybrid_on?"ENGAGED":"dormant");
        return 1;
      }
      if(le) snprintf(buf,buflen,"%s", le->title); return 1;
    }
    case EX_GS_RESULTS:{ snprintf(buf,buflen,"continue"); return 1; }
    case EX_GS_PAUSED:{
      const char *r[3]={"resume","quit match","quit game"};
      if(i>=0&&i<3) snprintf(buf,buflen,"%s%s",sel,r[i]); return 3;
    }
    default: return 0;
  }
}


int ex_menu_row_count(const ex_menu_t *m){
  char tmp[8]; return ex_menu_line(m, 0, tmp, sizeof(tmp));
}
void ex_menu_set_cursor(ex_menu_t *m, int row){
  int n = ex_menu_row_count(m);
  if(n<=0) return;
  if(row<0) row=0; if(row>=n) row=n-1;
  m->cursor = row;
}
void ex_menu_click(ex_menu_t *m){ ex_menu_nav(m, EX_NAV_CONFIRM); }
