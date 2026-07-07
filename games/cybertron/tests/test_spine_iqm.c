/* test_spine_iqm.c -- validates spine-grown robot-part IQMs (from
   tools/gen_spine.py) against the EXACT header/mesh/vertexarray layout in
   exengine/util/iqm.h, with NO GL -- the same headless guarantee test_arena_iqm
   gives the map, but for character parts. Also checks the part-slot invariant:
   the mesh is recentered and normalized so its longest axis half-extent ~= 1,
   so it drops into a robot part slot like box.iqm / plimb.iqm.

   Usage: test_spine_iqm <file.iqm> [<file.iqm> ...]
   Each file must pass or the test aborts (assert). */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <assert.h>
#include <math.h>
#define uint uint32_t

typedef struct { char magic[16]; uint version, filesize, flags;
  uint num_text, ofs_text, num_meshes, ofs_meshes;
  uint num_vertexarrays, num_vertexes, ofs_vertexarrays;
  uint num_triangles, ofs_triangles, ofs_adjacency;
  uint num_joints, ofs_joints, num_poses, ofs_poses, num_anims, ofs_anims;
  uint num_frames, num_framechannels, ofs_frames, ofs_bounds;
  uint num_comment, ofs_comment, num_extensions, ofs_extensions; } hdr_t;
typedef struct { uint type, flags, format, size, offset; } va_t;
typedef struct { uint name, material, first_vertex, num_vertexes, first_triangle, num_triangles; } mesh_t;

static void check_one(const char *path) {
  FILE *f = fopen(path, "rb");
  if (!f) { fprintf(stderr, "cannot open %s\n", path); assert(0 && "part iqm not found"); }
  fseek(f, 0, SEEK_END); long sz = ftell(f); fseek(f, 0, SEEK_SET);
  unsigned char *d = malloc(sz); assert(fread(d, 1, sz, f) == (size_t)sz); fclose(f);

  hdr_t h; memcpy(&h, d, sizeof(h));
  printf("[%s] version=%u filesize=%u (actual %ld)\n", path, h.version, h.filesize, sz);
  assert(strcmp(h.magic, "INTERQUAKEMODEL") == 0);
  assert(h.version == 2);
  assert(h.filesize == (uint)sz);
  assert(h.num_joints == 0 && h.num_anims == 0 && h.num_poses == 0); // static part

  assert(h.num_meshes >= 1 && h.num_vertexes > 0 && h.num_triangles > 0);
  assert(h.ofs_meshes + h.num_meshes*sizeof(mesh_t) <= (uint)sz);
  assert(h.ofs_vertexarrays + h.num_vertexarrays*sizeof(va_t) <= (uint)sz);
  assert(h.ofs_triangles + h.num_triangles*12 <= (uint)sz);

  // vertex arrays: POSITION(type0,float=7,size3) TEXCOORD(1,7,2) NORMAL(2,7,3)
  va_t *vas = (va_t*)&d[h.ofs_vertexarrays];
  float *pos = NULL; int seen[3] = {0,0,0};
  for (uint i = 0; i < h.num_vertexarrays; i++) {
    va_t *va = &vas[i];
    assert(va->offset + h.num_vertexes*va->size*4 <= (uint)sz);
    if (va->type == 0) { assert(va->format==7 && va->size==3); pos = (float*)&d[va->offset]; seen[0]=1; }
    if (va->type == 1) { assert(va->format==7 && va->size==2); seen[1]=1; }
    if (va->type == 2) { assert(va->format==7 && va->size==3); seen[2]=1; }
  }
  assert(seen[0] && seen[1] && seen[2] && pos);

  // meshes reference valid vertex/triangle ranges and a real material string
  char *text = (char*)&d[h.ofs_text];
  mesh_t *ms = (mesh_t*)&d[h.ofs_meshes];
  for (uint i = 0; i < h.num_meshes; i++) {
    assert(ms[i].first_vertex + ms[i].num_vertexes <= h.num_vertexes);
    assert(ms[i].first_triangle + ms[i].num_triangles <= h.num_triangles);
    assert(ms[i].material < h.num_text);
    assert(strlen(&text[ms[i].material]) > 0);
  }

  // triangles: indices in range and non-degenerate
  uint *tri = (uint*)&d[h.ofs_triangles];
  for (uint t = 0; t < h.num_triangles; t++) {
    uint a = tri[t*3], b = tri[t*3+1], c = tri[t*3+2];
    assert(a < h.num_vertexes && b < h.num_vertexes && c < h.num_vertexes);
    assert(a != b && b != c && a != c);
  }

  // part-slot invariant: recentered + normalized so longest-axis half-extent ~1
  float mn[3]={1e9f,1e9f,1e9f}, mx[3]={-1e9f,-1e9f,-1e9f};
  for (uint v = 0; v < h.num_vertexes; v++)
    for (int k=0;k<3;k++){ float p=pos[v*3+k]; if(p<mn[k])mn[k]=p; if(p>mx[k])mx[k]=p; }
  float hx=(mx[0]-mn[0])*0.5f, hy=(mx[1]-mn[1])*0.5f, hz=(mx[2]-mn[2])*0.5f;
  float cx=(mx[0]+mn[0])*0.5f, cy=(mx[1]+mn[1])*0.5f, cz=(mx[2]+mn[2])*0.5f;
  float longest = fmaxf(hx, fmaxf(hy, hz));
  printf("  verts=%u tris=%u  half-extent (%.3f,%.3f,%.3f) center (%.3f,%.3f,%.3f)\n",
         h.num_vertexes, h.num_triangles, hx,hy,hz, cx,cy,cz);
  assert(fabsf(longest - 1.0f) < 0.02f && "longest-axis half-extent must be ~1 (unit slot)");
  assert(fabsf(cx) < 0.02f && fabsf(cy) < 0.02f && fabsf(cz) < 0.02f && "mesh must be recentered");

  free(d);
  printf("  OK\n");
}

int main(int argc, char **argv) {
  const char *defaults[] = {
    "engine_src/data/spine_wing_cannon.iqm",
    "engine_src/data/spine_muscle.iqm",
    "engine_src/data/spine_cable.iqm",
    "engine_src/data/spine_crest.iqm",
  };
  if (argc > 1) { for (int i=1;i<argc;i++) check_one(argv[i]); }
  else          { for (int i=0;i<4;i++)    check_one(defaults[i]); }
  printf("\nSPINE PART IQMs STRUCTURALLY VALID (load-compatible + unit-slot normalized)\n");
  return 0;
}
