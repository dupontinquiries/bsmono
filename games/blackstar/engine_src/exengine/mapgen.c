#include "mapgen.h"
#include <math.h>
#include <string.h>

static uint32_t xr(uint32_t *s){ *s^=*s<<13; *s^=*s>>17; *s^=*s<<5; return *s; }
static float rf(uint32_t *s){ return (float)(xr(s)&0xFFFFFF)/(float)0x1000000; }

const char* ex_node_role_name(ex_node_role r){
  switch(r){case EX_NODE_SPAWN:return"spawn";case EX_NODE_TOWER:return"tower";
    case EX_NODE_TRENCH:return"trench";case EX_NODE_ROOM:return"room";
    case EX_NODE_JUNCTION:return"junction";default:return"cover";}
}
const char* ex_vol_role_name(ex_vol_role r){
  switch(r){case EX_VOL_BLOCK:return"block";case EX_VOL_LOWCOVER:return"lowcover";
    case EX_VOL_HIGHCOVER:return"highcover";case EX_VOL_CHANNEL:return"channel";
    case EX_VOL_EXPOSE:return"expose";case EX_VOL_ELEVATE:return"elevate";default:return"conceal";}
}

// -------- generation: scatter nodes, spanning tree + loop edges, assign roles
void ex_mapgraph_generate(ex_map_graph_t *g, uint32_t seed, float extent)
{
  memset(g, 0, sizeof(*g));
  g->extent = extent;
  uint32_t s = seed ? seed : 1u;
  int n = 8 + (int)(rf(&s) * 5.0f);   // 8..12 nodes
  if (n > EX_MAP_MAX_NODES) n = EX_MAP_MAX_NODES;
  g->n_nodes = n;

  // spawns at opposite ends
  g->nodes[0].x = -extent*0.8f; g->nodes[0].z = 0; g->nodes[0].role = EX_NODE_SPAWN;
  g->nodes[1].x =  extent*0.8f; g->nodes[1].z = 0; g->nodes[1].role = EX_NODE_SPAWN;
  for (int i = 2; i < n; i++) {
    g->nodes[i].x = (rf(&s)*2.0f-1.0f) * extent * 0.85f;
    g->nodes[i].z = (rf(&s)*2.0f-1.0f) * extent * 0.85f;
  }
  for (int i = 0; i < n; i++) g->nodes[i].elev = rf(&s);

  // nearest-neighbour spanning tree (guarantees connectivity)
  int in_tree[EX_MAP_MAX_NODES] = {0};
  in_tree[0] = 1; int tree_count = 1;
  while (tree_count < n && g->n_edges < EX_MAP_MAX_EDGES) {
    int ba=-1, bb=-1; float bd=1e18f;
    for (int i = 0; i < n; i++) if (in_tree[i])
      for (int j = 0; j < n; j++) if (!in_tree[j]) {
        float dx=g->nodes[i].x-g->nodes[j].x, dz=g->nodes[i].z-g->nodes[j].z;
        float d=dx*dx+dz*dz; if (d<bd){bd=d;ba=i;bb=j;}
      }
    if (bb<0) break;
    g->edges[g->n_edges++] = (ex_map_edge){ba,bb};
    in_tree[bb]=1; tree_count++;
  }
  // a few extra links -> loops -> route choice (shortest non-tree pairs)
  int extra = 2 + (int)(rf(&s)*3.0f);
  for (int k = 0; k < extra && g->n_edges < EX_MAP_MAX_EDGES; k++) {
    int ba=-1, bb=-1; float bd=1e18f;
    for (int i = 0; i < n; i++) for (int j = i+1; j < n; j++) {
      int exists=0; for (int e=0;e<g->n_edges;e++){ ex_map_edge*E=&g->edges[e];
        if ((E->a==i&&E->b==j)||(E->a==j&&E->b==i)){exists=1;break;} }
      if (exists) continue;
      float dx=g->nodes[i].x-g->nodes[j].x, dz=g->nodes[i].z-g->nodes[j].z;
      float d=dx*dx+dz*dz;
      // bias toward medium-length links (avoid trivially short duplicates)
      if (d>bd) continue; bd=d; ba=i; bb=j;
    }
    // pick a *random* medium link instead of always the shortest, for variety
    if (ba>=0){ g->edges[g->n_edges++] = (ex_map_edge){ba,bb}; bd=1e18f; }
  }

  // degrees
  for (int e=0;e<g->n_edges;e++){ g->nodes[g->edges[e].a].degree++; g->nodes[g->edges[e].b].degree++; }
  // roles from elevation + connectivity (spawns keep their role)
  for (int i = 2; i < n; i++) {
    ex_map_node *nd = &g->nodes[i];
    if (nd->elev > 0.7f)        nd->role = EX_NODE_TOWER;
    else if (nd->elev < 0.25f)  nd->role = EX_NODE_TRENCH;
    else if (nd->degree >= 4)   nd->role = EX_NODE_JUNCTION;
    else if (nd->degree <= 1)   nd->role = EX_NODE_COVER;
    else                        nd->role = EX_NODE_ROOM;
  }
}

// -------- connectivity: flood fill from node 0
int ex_mapgraph_connected(const ex_map_graph_t *g)
{
  int seen[EX_MAP_MAX_NODES] = {0};
  int stack[EX_MAP_MAX_NODES], sp=0;
  stack[sp++]=0; seen[0]=1; int cnt=1;
  while (sp>0) {
    int cur=stack[--sp];
    for (int e=0;e<g->n_edges;e++){
      int o=-1; if(g->edges[e].a==cur)o=g->edges[e].b; else if(g->edges[e].b==cur)o=g->edges[e].a;
      if(o>=0 && !seen[o]){ seen[o]=1; cnt++; stack[sp++]=o; }
    }
  }
  return cnt == g->n_nodes;
}

// hop distance between the two spawns (BFS)
static int spawn_distance(const ex_map_graph_t *g)
{
  int dist[EX_MAP_MAX_NODES]; for(int i=0;i<g->n_nodes;i++) dist[i]=-1;
  int q[EX_MAP_MAX_NODES], head=0, tail=0;
  dist[0]=0; q[tail++]=0;
  while(head<tail){ int c=q[head++];
    for(int e=0;e<g->n_edges;e++){ int o=-1;
      if(g->edges[e].a==c)o=g->edges[e].b; else if(g->edges[e].b==c)o=g->edges[e].a;
      if(o>=0 && dist[o]<0){ dist[o]=dist[c]+1; q[tail++]=o; } } }
  return dist[1];
}

ex_map_score_t ex_mapgraph_score(const ex_map_graph_t *g)
{
  ex_map_score_t s; memset(&s,0,sizeof(s));
  s.connected = ex_mapgraph_connected(g);
  if (!s.connected) { s.total = 0.0f; return s; }

  // flow: spawn-to-spawn hops best in 3..5
  int sd = spawn_distance(g);
  if (sd < 0) { s.total=0; return s; }
  s.flow = (sd>=3 && sd<=5) ? 1.0f : (sd==2||sd==6) ? 0.6f : 0.3f;

  // route choice: extra edges beyond the spanning tree (n-1)
  int extra = g->n_edges - (g->n_nodes - 1);
  s.route_choice = extra <= 0 ? 0.0f : extra >= 4 ? 1.0f : extra/4.0f;

  // vantage: fraction of towers
  int towers=0; for(int i=0;i<g->n_nodes;i++) if(g->nodes[i].role==EX_NODE_TOWER) towers++;
  s.vantage = fminf(1.0f, towers / 2.0f);

  // flank: towers reachable by 2+ edges
  int flankable=0; for(int i=0;i<g->n_nodes;i++)
    if(g->nodes[i].role==EX_NODE_TOWER && g->nodes[i].degree>=2) flankable++;
  s.flank = towers>0 ? (float)flankable/towers : 0.5f;

  // sightlines: variety of node roles present
  int have[6]={0}; for(int i=0;i<g->n_nodes;i++) have[g->nodes[i].role]=1;
  int variety=0; for(int i=0;i<6;i++) variety+=have[i];
  s.sightlines = variety/6.0f;

  float w = s.flow*0.26f + s.route_choice*0.22f + s.sightlines*0.22f + s.flank*0.16f + s.vantage*0.14f;
  s.total = w * 100.0f;
  return s;
}

uint32_t ex_mapgraph_select_best(uint32_t base_seed, int budget, float extent, ex_map_graph_t *out)
{
  uint32_t best_seed = base_seed; float best = -1.0f;
  ex_map_graph_t g;
  for (int i = 0; i < budget; i++) {
    uint32_t seed = base_seed + (uint32_t)i*2654435761u + 1u;
    ex_mapgraph_generate(&g, seed, extent);
    ex_map_score_t sc = ex_mapgraph_score(&g);
    if (sc.total > best) { best = sc.total; best_seed = seed; if(out)*out=g; }
  }
  return best_seed;
}

// -------- blueprint: compile nodes/edges into placed collidable volumes
static void add_vol(ex_map_blueprint_t *bp, ex_vol_role role, float x,float y,float z,
                    float hx,float hy,float hz, int mat)
{
  if (bp->n_vols >= EX_MAP_MAX_VOLS) return;
  ex_map_vol_t *v = &bp->vols[bp->n_vols++];
  v->role = role;
  v->center[0]=x; v->center[1]=y; v->center[2]=z;
  v->half[0]=hx; v->half[1]=hy; v->half[2]=hz; v->material=mat;
}

void ex_mapgraph_to_blueprint(const ex_map_graph_t *g, ex_map_blueprint_t *bp, uint32_t seed)
{
  memset(bp, 0, sizeof(*bp));
  uint32_t s = seed ? seed : 7u;

  bp->spawn[0][0]=g->nodes[0].x; bp->spawn[0][2]=g->nodes[0].z; bp->spawn[0][1]=0;
  bp->spawn[1][0]=g->nodes[1].x; bp->spawn[1][2]=g->nodes[1].z; bp->spawn[1][1]=0;

  // Each node becomes a structure. The world is MASSIVE and vertical, so towers
  // rise tens of units with tiered platforms and walkable stepped ramps, and
  // there are floating platforms only reachable by flight or a big jump.
  for (int i = 0; i < g->n_nodes; i++) {
    const ex_map_node *nd = &g->nodes[i];
    float x=nd->x, z=nd->z;
    switch (nd->role) {
      case EX_NODE_TOWER: {
        // giant multi-tier tower: base + 3 rising platforms + a tall spire,
        // with a long stepped ramp climbing to the first tier.
        float th = 14.0f + nd->elev*26.0f;         // tower height 14..40
        add_vol(bp, EX_VOL_ELEVATE, x, th*0.25f, z, 7.0f, th*0.25f, 7.0f, 2);   // base block
        add_vol(bp, EX_VOL_ELEVATE, x, th*0.55f, z, 5.0f, 1.0f, 5.0f, 2);       // mid platform
        add_vol(bp, EX_VOL_ELEVATE, x, th*0.85f, z, 3.5f, 1.0f, 3.5f, 1);       // top platform
        add_vol(bp, EX_VOL_EXPOSE,  x, th*1.15f, z, 1.4f, th*0.3f, 1.4f, 1);    // crowning spire
        // stepped approach ramp (walkable staircase up one side)
        int steps = 6;
        for (int k=0;k<steps;k++){
          float hh = (th*0.25f) * (k+1)/steps;
          float zz = z + 9.0f + (steps-k)*3.0f;
          add_vol(bp, EX_VOL_ELEVATE, x, hh*0.5f, zz, 4.0f, hh*0.5f, 1.8f, 2);
        }
        break;
      }
      case EX_NODE_ROOM: {
        // a real enclosed room: four walls with a doorway gap on one side, a
        // roof you can stand on, and a cover column inside. Reads as a room with
        // defined bounds instead of three loose slabs.
        float rw = 6.0f + rf(&s)*2.0f;      // half-width of the room
        float wh = 4.5f;                    // wall height
        int door = (int)(rf(&s)*4.0f);      // which side gets the doorway (0..3)
        // side -Z / +Z walls (split around a doorway if that's the door side)
        for (int side=0; side<2; side++){
          float zz = z + (side? rw : -rw);
          if ((side?2:0)==door || (side?3:1)==door){
            // doorway: two short wall segments leaving a gap in the middle
            add_vol(bp, EX_VOL_CONCEAL, x-rw*0.6f, wh*0.5f, zz, rw*0.4f, wh*0.5f, 0.8f, 0);
            add_vol(bp, EX_VOL_CONCEAL, x+rw*0.6f, wh*0.5f, zz, rw*0.4f, wh*0.5f, 0.8f, 0);
          } else
            add_vol(bp, EX_VOL_CONCEAL, x, wh*0.5f, zz, rw, wh*0.5f, 0.8f, 0);
        }
        // side -X / +X walls
        for (int side=0; side<2; side++){
          float xx = x + (side? rw : -rw);
          if ((side?1:0)==door){
            add_vol(bp, EX_VOL_CONCEAL, xx, wh*0.5f, z-rw*0.6f, 0.8f, wh*0.5f, rw*0.4f, 0);
            add_vol(bp, EX_VOL_CONCEAL, xx, wh*0.5f, z+rw*0.6f, 0.8f, wh*0.5f, rw*0.4f, 0);
          } else
            add_vol(bp, EX_VOL_CONCEAL, xx, wh*0.5f, z, 0.8f, wh*0.5f, rw, 0);
        }
        // walkable roof + an interior cover column + a rooftop vantage platform
        add_vol(bp, EX_VOL_ELEVATE, x, wh+0.4f, z, rw, 0.5f, rw, 1);
        add_vol(bp, EX_VOL_LOWCOVER, x+rw*0.35f, 1.2f, z-rw*0.35f, 1.2f, 1.2f, 1.2f, 1);
        add_vol(bp, EX_VOL_ELEVATE, x, wh+8.0f + rf(&s)*8.0f, z, 3.5f, 0.7f, 3.5f, 1);  // sky reward
        break;
      }
      case EX_NODE_JUNCTION:
        add_vol(bp, EX_VOL_ELEVATE, x,1.5f,z, 6.0f,1.5f,6.0f, 0);               // raised deck
        add_vol(bp, EX_VOL_LOWCOVER, x,3.6f,z, 2.0f,0.6f,2.0f, 1);              // cover on top
        break;
      case EX_NODE_COVER: {
        float ch = 2.0f + rf(&s)*4.0f;
        add_vol(bp, (rf(&s)<0.5f)?EX_VOL_LOWCOVER:EX_VOL_HIGHCOVER,
                x, ch*0.5f, z, 3.0f, ch*0.5f, 2.5f, (rf(&s)<0.5f)?0:1);
        break;
      }
      case EX_NODE_TRENCH:
        add_vol(bp, EX_VOL_LOWCOVER, x,1.2f,z, 6.0f,1.2f,1.6f, 1);              // long berm
        break;
      default: break; // spawns stay clear
    }
  }
  // a colossal central megastructure -- a landmark spire you can climb/fly,
  // with wide tiered platforms, dominating the middle of the world.
  {
    float cx=0, cz=0;
    add_vol(bp, EX_VOL_ELEVATE, cx, 10.0f, cz, 14.0f, 10.0f, 14.0f, 2);        // massive plinth
    add_vol(bp, EX_VOL_ELEVATE, cx, 24.0f, cz, 9.0f, 4.0f, 9.0f, 2);          // tier 2
    add_vol(bp, EX_VOL_ELEVATE, cx, 36.0f, cz, 5.0f, 4.0f, 5.0f, 1);          // tier 3
    add_vol(bp, EX_VOL_EXPOSE,  cx, 60.0f, cz, 2.5f, 20.0f, 2.5f, 1);         // colossal spire
  }
  // ---- two-level SKY DECKS: big walkable platforms held up on giant pillars,
  //      reached by a long stepped ramp. These create a real upper level with
  //      corners, cover, and vantage over the fields below. ----
  {
    float dpos[2][2] = { {-285.0f, 255.0f}, {270.0f, -285.0f} };
    for (int d=0; d<2 && bp->n_vols < EX_MAP_MAX_VOLS-14; d++) {
      float cx=dpos[d][0], cz=dpos[d][1], dh=20.0f, hw=15.0f;
      // the deck itself (wide, thin, walkable)
      add_vol(bp, EX_VOL_ELEVATE, cx, dh, cz, hw, 1.0f, hw, 2);
      // four giant support pillars at the corners (tall thick columns)
      add_vol(bp, EX_VOL_BLOCK, cx-hw+2, dh*0.5f, cz-hw+2, 2.2f, dh*0.5f, 2.2f, 0);
      add_vol(bp, EX_VOL_BLOCK, cx+hw-2, dh*0.5f, cz-hw+2, 2.2f, dh*0.5f, 2.2f, 0);
      add_vol(bp, EX_VOL_BLOCK, cx-hw+2, dh*0.5f, cz+hw-2, 2.2f, dh*0.5f, 2.2f, 0);
      add_vol(bp, EX_VOL_BLOCK, cx+hw-2, dh*0.5f, cz+hw-2, 2.2f, dh*0.5f, 2.2f, 0);
      // low cover walls on top of the deck (corners to fight around)
      add_vol(bp, EX_VOL_LOWCOVER, cx-hw*0.5f, dh+1.6f, cz, 0.6f, 1.4f, 4.0f, 1);
      add_vol(bp, EX_VOL_LOWCOVER, cx+hw*0.5f, dh+1.6f, cz, 0.6f, 1.4f, 4.0f, 1);
      // a stepped ramp climbing to the deck from one side
      int steps = 8;
      for (int k=0;k<steps && bp->n_vols < EX_MAP_MAX_VOLS; k++){
        float hh = dh * (k+1)/steps;
        float zz = cz + hw + (steps-k)*3.5f;
        add_vol(bp, EX_VOL_ELEVATE, cx, hh*0.5f, zz, 4.5f, hh*0.5f, 2.0f, 2);
      }
    }
  }
  // ---- standalone GIANT PILLARS: thick columns scattered to carve the open
  //      fields into corners and sightline breaks. ----
  {
    float px[6]={ -150, 165, -60, 120, -225, 210 };
    float pz[6]={  120, -90, -210, 225, -30, -180 };
    for (int i=0;i<6 && bp->n_vols < EX_MAP_MAX_VOLS; i++)
      add_vol(bp, EX_VOL_BLOCK, px[i], 14.0f, pz[i], 3.5f, 14.0f, 3.5f, 0);
  }
  // ---- VANTAGE PERCHES: small elevated platforms at the arena edges, a
  //      commanding view for snipers (reach them by flight or a launch pad). ----
  {
    float vx[4]={ -450, 450, -450, 450 };
    float vz[4]={ -450,-450, 450, 450 };
    for (int i=0;i<4 && bp->n_vols < EX_MAP_MAX_VOLS-1; i++){
      add_vol(bp, EX_VOL_ELEVATE, vx[i], 26.0f, vz[i], 6.0f, 1.0f, 6.0f, 1);
      add_vol(bp, EX_VOL_LOWCOVER, vx[i], 27.6f, vz[i], 4.0f, 0.8f, 0.6f, 1);
    }
  }
  // ---- INTERIOR ROOMS: small enclosed structures with a doorway gap and a
  //      control panel inside (marked for game-side console placement). Walls
  //      are BLOCK volumes; one side is split to leave a corridor entrance. ----
  {
    float rpos[3][2] = { {-200.0f, -150.0f}, {210.0f, 140.0f}, {30.0f, -260.0f} };
    for (int r=0; r<3 && bp->n_vols < EX_MAP_MAX_VOLS-6; r++){
      float cx=rpos[r][0], cz=rpos[r][1], hs=18.0f, wh=7.0f, wt=1.2f;
      // back + two sides fully; front split into two stubs leaving a doorway
      add_vol(bp, EX_VOL_BLOCK, cx,        wh, cz-hs, hs, wh, wt, 0);   // back wall
      add_vol(bp, EX_VOL_BLOCK, cx-hs,     wh, cz,    wt, wh, hs, 0);   // left wall
      add_vol(bp, EX_VOL_BLOCK, cx+hs,     wh, cz,    wt, wh, hs, 0);   // right wall
      add_vol(bp, EX_VOL_BLOCK, cx-hs*0.6f,wh, cz+hs, hs*0.4f, wh, wt, 0); // front-left stub
      add_vol(bp, EX_VOL_BLOCK, cx+hs*0.6f,wh, cz+hs, hs*0.4f, wh, wt, 0); // front-right stub
      // low cover crate inside next to the console
      add_vol(bp, EX_VOL_LOWCOVER, cx+hs*0.4f, 1.4f, cz, 2.0f, 1.4f, 2.0f, 1);
    }
  }
  // each edge midpoint may get a Block/Channel to break the lane sightline
  for (int e = 0; e < g->n_edges && bp->n_vols < EX_MAP_MAX_VOLS; e++) {
    const ex_map_node *a=&g->nodes[g->edges[e].a], *b=&g->nodes[g->edges[e].b];
    if (a->role==EX_NODE_SPAWN || b->role==EX_NODE_SPAWN) continue; // keep spawns open
    if (rf(&s) < 0.5f) {
      float mx=(a->x+b->x)*0.5f, mz=(a->z+b->z)*0.5f;
      // orient the block across the lane
      float dx=b->x-a->x, dz=b->z-a->z; float len=sqrtf(dx*dx+dz*dz); if(len<0.01f)len=1;
      int across_x = fabsf(dx) < fabsf(dz);
      add_vol(bp, EX_VOL_BLOCK, mx,1.0f,mz,
              across_x?2.2f:0.5f, 1.0f, across_x?0.5f:2.2f, 0);
    }
  }
}
