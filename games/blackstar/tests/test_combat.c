/* test_combat -- exercises the core hitscan math in combat.c.
 * This is the ray-resolution every ranged shot in the game runs through;
 * geometric edge cases (parallel rays, inside-sphere origins, cover ordering)
 * are easy to get subtly wrong, so pin them down. */
#include "combat.h"
#include "coll_chunk.h"
#include <stdio.h>
#include <math.h>
#include <assert.h>
#include <string.h>

#define EPS 1e-3f
static int approx(float a, float b) { return fabsf(a - b) < EPS; }

static void norm(vec3 v) {
    float l = sqrtf(v[0]*v[0] + v[1]*v[1] + v[2]*v[2]);
    if (l > 1e-9f) { v[0]/=l; v[1]/=l; v[2]/=l; }
}

/* ---- ray vs AABB ---- */
static void test_ray_aabb(void) {
    vec3 c = {5, 0, 0}, half = {1, 1, 1};
    float t;

    // straight hit down +x: enters at x=4
    { vec3 o={0,0,0}, d={1,0,0}; assert(ex_ray_aabb(o,d,c,half,100,&t)); assert(approx(t,4.0f)); }
    // miss: passes above the box
    { vec3 o={0,5,0}, d={1,0,0}; assert(!ex_ray_aabb(o,d,c,half,100,&t)); }
    // box behind the ray: no hit
    { vec3 o={0,0,0}, d={-1,0,0}; assert(!ex_ray_aabb(o,d,c,half,100,&t)); }
    // beyond max_dist: no hit (box entry at 4, cap at 3)
    { vec3 o={0,0,0}, d={1,0,0}; assert(!ex_ray_aabb(o,d,c,half,3.0f,&t)); }
    // parallel ray inside the y/z slabs still resolves on x
    { vec3 o={0,0.5f,-0.5f}, d={1,0,0}; assert(ex_ray_aabb(o,d,c,half,100,&t)); assert(approx(t,4.0f)); }
    // parallel ray OUTSIDE a slab: origin y=5 is outside [-1,1], dir has no y -> miss
    { vec3 o={0,5,0}, d={1,0,0}; assert(!ex_ray_aabb(o,d,c,half,100,&t)); }
    // diagonal grazing hit into the near corner
    { vec3 o={0,-2,0}, d={4,2,0}; norm(d); assert(ex_ray_aabb(o,d,c,half,100,&t)); }
    // origin inside the box: entry dist clamps to 0 (tmin<0 rejected? spec: tmin>=0)
    { vec3 o={5,0,0}, d={1,0,0}; int hit=ex_ray_aabb(o,d,c,half,100,&t);
      /* origin inside -> tmin becomes negative on the -x slab, spec returns 0 */
      assert(hit==0 || t>=0.0f); }
    printf("test_combat: ray/AABB slab cases (hit/miss/behind/range/parallel/graze) OK\n");
}

/* ---- ray vs sphere ---- */
static void test_ray_sphere(void) {
    vec3 c = {0, 0, 10}; float r = 2.0f, t;

    // head-on: enters at z=8
    { vec3 o={0,0,0}, d={0,0,1}; assert(ex_ray_sphere(o,d,c,r,100,&t)); assert(approx(t,8.0f)); }
    // miss: offset beyond radius
    { vec3 o={5,0,0}, d={0,0,1}; assert(!ex_ray_sphere(o,d,c,r,100,&t)); }
    // tangent: exactly grazes at radius (offset == r), should just touch
    { vec3 o={2,0,0}, d={0,0,1}; int hit=ex_ray_sphere(o,d,c,r,100,&t);
      assert(hit); assert(approx(t,10.0f)); }   // tangent point is at the center's z
    // origin INSIDE the sphere: uses far root (forward exit)
    { vec3 o={0,0,10}, d={0,0,1}; assert(ex_ray_sphere(o,d,c,r,100,&t)); assert(approx(t,2.0f)); }
    // sphere behind: no hit
    { vec3 o={0,0,0}, d={0,0,-1}; assert(!ex_ray_sphere(o,d,c,r,100,&t)); }
    // out of range: enters at 8, cap at 5
    { vec3 o={0,0,0}, d={0,0,1}; assert(!ex_ray_sphere(o,d,c,r,5.0f,&t)); }
    printf("test_combat: ray/sphere roots (near/far/tangent/inside/range) OK\n");
}

/* ---- combined raycast: nearest-of chunk vs enemy ---- */
static void test_raycast_resolution(void) {
    ex_chunk_world_t *w = ex_chunk_world_new();
    vec3 tris[6] = {{0,0,0},{1,0,0},{1,1,0},{0,0,0},{1,1,0},{0,1,0}};
    // a solid wall centered at x=6 (spans 5..7 on x)
    ex_chunk_add(w, "wall", ex_rect_new((vec3){5,-1,-1},(vec3){7,1,1}), tris, 2,
                 100.0f, EX_CHUNK_HIT_STANDARD, 1);

    vec3 o={0,0,0}, d={1,0,0};

    // enemy BEHIND the wall (x=10): wall should be hit first
    ex_target_t tgt_far = { {10,0,0}, 1.0f, 42, 1 };
    { ex_hit_t h = ex_combat_raycast(w, &tgt_far, 1, o, d, 100);
      assert(h.kind==EX_HIT_CHUNK); assert(approx(h.dist,5.0f)); }

    // enemy IN FRONT of the wall (x=3): enemy hit first
    ex_target_t tgt_near = { {3,0,0}, 1.0f, 7, 1 };
    { ex_hit_t h = ex_combat_raycast(w, &tgt_near, 1, o, d, 100);
      assert(h.kind==EX_HIT_ENEMY); assert(h.index==7); assert(approx(h.dist,2.0f)); }

    // dead enemy is skipped -> falls through to the wall
    ex_target_t tgt_dead = { {3,0,0}, 1.0f, 7, 0 };
    { ex_hit_t h = ex_combat_raycast(w, &tgt_dead, 1, o, d, 100);
      assert(h.kind==EX_HIT_CHUNK); }

    // impact point is origin + dir*dist
    { ex_hit_t h = ex_combat_raycast(w, &tgt_near, 1, o, d, 100);
      assert(approx(h.point[0], 2.0f)); assert(approx(h.point[1],0.0f)); }

    // nothing in the way -> EX_HIT_NONE, point at max range
    { vec3 up={0,1,0}; ex_hit_t h = ex_combat_raycast(w, NULL, 0, o, up, 50);
      assert(h.kind==EX_HIT_NONE); assert(approx(h.point[1],50.0f)); }

    printf("test_combat: combined raycast (nearest-of, cover blocks, dead skipped, point) OK\n");

    // destroyed cover no longer blocks: shot passes through to the enemy behind
    ex_coll_chunk_t *wall = ex_chunk_find(w, "wall");
    ex_chunk_set_state(wall, EX_CHUNK_STATE_DESTROYED);
    { ex_hit_t h = ex_combat_raycast(w, &tgt_far, 1, o, d, 100);
      assert(h.kind==EX_HIT_ENEMY); assert(h.index==42); }
    printf("test_combat: destroyed cover stops blocking (shot reaches enemy behind) OK\n");

    ex_chunk_world_free(w);
}

int main(void) {
    test_ray_aabb();
    test_ray_sphere();
    test_raycast_resolution();
    printf("ALL ASSERTIONS PASSED\n");
    return 0;
}
