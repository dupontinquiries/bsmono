/* test_mapgen.c -- flow-first graph gen, connectivity gate, scoring, selection,
   and blueprint compilation into collidable volumes. */
#include "mapgen.h"
#include <stdio.h>
#include <assert.h>
#include <string.h>

int main(void){
  printf("=== graph generation ===\n");
  ex_map_graph_t g; ex_mapgraph_generate(&g, 12345, 60.0f);
  printf("  nodes=%d edges=%d\n", g.n_nodes, g.n_edges);
  assert(g.n_nodes >= 8 && g.n_edges >= g.n_nodes-1);   // at least a spanning tree
  // spanning tree guarantees connectivity
  assert(ex_mapgraph_connected(&g));
  // two spawns at opposite ends
  assert(g.nodes[0].role==EX_NODE_SPAWN && g.nodes[1].role==EX_NODE_SPAWN);
  assert(g.nodes[0].x < 0 && g.nodes[1].x > 0);

  // deterministic
  ex_map_graph_t g2; ex_mapgraph_generate(&g2, 12345, 60.0f);
  assert(memcmp(&g,&g2,sizeof(g))==0);

  printf("=== scoring ===\n");
  ex_map_score_t sc = ex_mapgraph_score(&g);
  printf("  connected=%d flow=%.2f route=%.2f sight=%.2f flank=%.2f vantage=%.2f total=%.1f\n",
    sc.connected, sc.flow, sc.route_choice, sc.sightlines, sc.flank, sc.vantage, sc.total);
  assert(sc.connected && sc.total > 0.0f && sc.total <= 100.0f);

  // a disconnected graph scores 0 (hard gate)
  ex_map_graph_t d = g; d.n_edges = 1; // sever almost everything
  ex_map_score_t ds = ex_mapgraph_score(&d);
  printf("  severed graph: connected=%d total=%.1f\n", ds.connected, ds.total);
  assert(!ds.connected && ds.total == 0.0f);

  printf("=== selection (best of N) ===\n");
  ex_map_graph_t best;
  uint32_t bseed = ex_mapgraph_select_best(1000, 200, 60.0f, &best);
  ex_map_score_t bs = ex_mapgraph_score(&best);
  printf("  best seed=%u total=%.1f\n", bseed, bs.total);
  assert(bs.connected && bs.total > 0.0f);
  // best of 200 should beat a typical single sample most of the time
  ex_map_graph_t one; ex_mapgraph_generate(&one, 1001, 60.0f);
  ex_map_score_t os = ex_mapgraph_score(&one);
  assert(bs.total >= os.total);

  printf("=== blueprint compilation ===\n");
  ex_map_blueprint_t bp; ex_mapgraph_to_blueprint(&best, &bp, bseed);
  printf("  volumes=%d\n", bp.n_vols);
  assert(bp.n_vols > 0 && bp.n_vols <= EX_MAP_MAX_VOLS);
  // volumes have positive extents and sane heights
  int roles[7]={0};
  for (int i=0;i<bp.n_vols;i++){
    assert(bp.vols[i].half[0]>0 && bp.vols[i].half[1]>0 && bp.vols[i].half[2]>0);
    assert(bp.vols[i].center[1] >= 0);
    roles[bp.vols[i].role]++;
  }
  printf("  vol roles: block=%d low=%d high=%d chan=%d expose=%d elev=%d conceal=%d\n",
    roles[0],roles[1],roles[2],roles[3],roles[4],roles[5],roles[6]);
  // spawns recorded
  assert(bp.spawn[0][0] < 0 && bp.spawn[1][0] > 0);

  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
