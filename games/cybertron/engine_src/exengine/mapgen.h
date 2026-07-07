/* mapgen
  Procedural arena generation, ported from early-fps mapgraph.rs + blockout.rs.

  Flow-first: a map is first a 2D GRAPH of nodes (places) and edges (routes).
  A graph is a few hundred bytes, so we can generate many candidates, gate on
  connectivity, score flow/route-choice/vantage/flank cheaply, and keep only
  the best seed. The winning graph is then compiled into a BLUEPRINT: a list of
  role-tagged volumes (Block / LowCover / HighCover / Channel / Expose /
  Elevate / Conceal) placed to break sightlines, grant vantage, and channel
  flow. The game realises each volume as BOTH a visual box AND a collidable
  chunk -- so procedural cover is solid, not decoration you walk through.

  Pure math + a hash rng; fully testable headless.
*/
#ifndef EX_MAPGEN_H
#define EX_MAPGEN_H

#include <stdint.h>

#define EX_MAP_MAX_NODES 16
#define EX_MAP_MAX_EDGES 40
#define EX_MAP_MAX_VOLS  150

typedef enum {
  EX_NODE_SPAWN = 0, EX_NODE_TOWER, EX_NODE_TRENCH,
  EX_NODE_ROOM, EX_NODE_JUNCTION, EX_NODE_COVER
} ex_node_role;

typedef enum {
  EX_VOL_BLOCK = 0,  // elongated mass breaking a sightline
  EX_VOL_LOWCOVER,   // crouch cover
  EX_VOL_HIGHCOVER,  // stand-height wall + peek edge
  EX_VOL_CHANNEL,    // wall that forces flow around it
  EX_VOL_EXPOSE,     // tall thin vantage mass
  EX_VOL_ELEVATE,    // raised platform
  EX_VOL_CONCEAL     // solid mass denying a pocket
} ex_vol_role;

typedef struct { float x, z, elev; ex_node_role role; int degree; } ex_map_node;
typedef struct { int a, b; } ex_map_edge;

typedef struct {
  ex_map_node nodes[EX_MAP_MAX_NODES];
  int n_nodes;
  ex_map_edge edges[EX_MAP_MAX_EDGES];
  int n_edges;
  float extent;
} ex_map_graph_t;

typedef struct {
  ex_vol_role role;
  float center[3];
  float half[3];
  int   material;   // 0=wall 1=rock 2=plate (theme picks textures)
} ex_map_vol_t;

typedef struct {
  ex_map_vol_t vols[EX_MAP_MAX_VOLS];
  int n_vols;
  float spawn[2][3];  // the two spawn points
} ex_map_blueprint_t;

typedef struct {
  int   connected;
  float flow, route_choice, sightlines, flank, vantage;
  float total;   // 0..100 (0 if disconnected)
} ex_map_score_t;

void  ex_mapgraph_generate(ex_map_graph_t *g, uint32_t seed, float extent);
int   ex_mapgraph_connected(const ex_map_graph_t *g);
ex_map_score_t ex_mapgraph_score(const ex_map_graph_t *g);

/**
 * [ex_mapgraph_select_best generate `budget` candidates, return the best seed
 *  and fill `out` with its graph]
 */
uint32_t ex_mapgraph_select_best(uint32_t base_seed, int budget, float extent, ex_map_graph_t *out);

/**
 * [ex_mapgraph_to_blueprint compile a graph into placed collidable volumes]
 */
void ex_mapgraph_to_blueprint(const ex_map_graph_t *g, ex_map_blueprint_t *bp, uint32_t seed);

const char* ex_node_role_name(ex_node_role r);
const char* ex_vol_role_name(ex_vol_role r);

#endif // EX_MAPGEN_H
