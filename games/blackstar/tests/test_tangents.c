/* Headless verification of the load-time tangent generation (ex_compute_tangents),
 * the foundation of the normal-mapping wiring. Checks that generated tangents are
 * finite, unit-length, and orthogonal to the vertex normal -- on both a synthetic
 * quad (known answer) and a real generated IQM. This is the closest we can get to
 * verifying the normal-mapping work without a GPU. */
#include "util/tangents.h"
#include <assert.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

static void test_synthetic(void) {
    /* a unit quad in the XY plane, normal +Z, UVs aligned so the U axis -> +X */
    float pos[] = { 0,0,0,  1,0,0,  1,1,0,  0,1,0 };
    float uv[]  = { 0,0,    1,0,    1,1,    0,1 };
    float nrm[] = { 0,0,1,  0,0,1,  0,0,1,  0,0,1 };
    unsigned int idx[] = { 0,1,2,  0,2,3 };
    float tan[16];
    ex_compute_tangents(pos, uv, nrm, tan, 4, idx, 2);
    for (int i = 0; i < 4; i++) {
        float *t = &tan[i*4];
        assert(t[0]==t[0] && t[1]==t[1] && t[2]==t[2]);           /* finite */
        float len = sqrtf(t[0]*t[0]+t[1]*t[1]+t[2]*t[2]);
        assert(fabsf(len - 1.0f) < 1e-3f);                        /* unit */
        assert(fabsf(t[2]) < 1e-3f);                              /* orthogonal to +Z */
        assert(t[0] > 0.9f);                                      /* U axis -> +X */
        assert(t[3] == 1.0f);                                     /* handedness */
    }
    printf("  synthetic quad: unit, orthogonal, U->+X, handedness OK\n");
}

static void test_real(const char *path) {
    FILE *f = fopen(path, "rb");
    if (!f) { printf("  (skip real mesh: %s not found)\n", path); return; }
    fseek(f, 0, SEEK_END); long sz = ftell(f); fseek(f, 0, SEEK_SET);
    uint8_t *d = malloc(sz);
    size_t got = fread(d, 1, sz, f); fclose(f);
    if (got != (size_t)sz) { printf("  (read error on %s)\n", path); free(d); return; }

    uint32_t v[26]; memcpy(v, d + 16, sizeof(v));
    int nv = v[8], nva = v[7], ova = v[9], ntri = v[10], otri = v[11];
    float *pos = NULL, *uv = NULL, *nrm = NULL;
    for (int k = 0; k < nva; k++) {
        uint32_t a[5]; memcpy(a, d + ova + k*20, 20);
        if (a[0] == 0) pos = (float*)(d + a[4]);
        if (a[0] == 1) uv  = (float*)(d + a[4]);
        if (a[0] == 2) nrm = (float*)(d + a[4]);
    }
    assert(pos && uv && nrm && "mesh missing pos/uv/normal");
    unsigned int *idx = (unsigned int*)(d + otri);

    float *tan = malloc(sizeof(float) * nv * 4);
    ex_compute_tangents(pos, uv, nrm, tan, nv, idx, ntri);
    for (int i = 0; i < nv; i++) {
        float *t = &tan[i*4], *n = &nrm[i*3];
        assert(t[0]==t[0] && t[1]==t[1] && t[2]==t[2] && t[3]==t[3]);  /* finite */
        float len = sqrtf(t[0]*t[0] + t[1]*t[1] + t[2]*t[2]);
        assert(len > 0.9f && len < 1.1f && "tangent not unit length");
        float dot = t[0]*n[0] + t[1]*n[1] + t[2]*n[2];
        assert(fabsf(dot) < 1e-2f && "tangent not orthogonal to normal");
        assert(t[3] == 1.0f);
    }
    printf("  %s: %d verts, all tangents finite + unit + orthogonal OK\n", path, nv);
    free(d); free(tan);
}

int main(int argc, char **argv) {
    test_synthetic();
    test_real(argc > 1 ? argv[1] : "engine_src/data/pchest.iqm");
    printf("ALL ASSERTIONS PASSED\n");
    return 0;
}
