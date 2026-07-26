/* tangents.h -- pure per-vertex tangent generation from UV gradients.
 * No GL / no engine deps, so it can be unit-tested headlessly. Used by the IQM
 * loader to give normal-mapping a valid tangent basis on meshes that ship without
 * one (all of our procedurally-generated IQMs). Lengyel accumulation + Gram-Schmidt
 * orthonormalization against the vertex normal; tangent.w carries handedness (+1). */
#ifndef EX_TANGENTS_H
#define EX_TANGENTS_H
#include <math.h>
#include <stdlib.h>

/* pos:  nv*3 floats, uv: nv*2, nrm: nv*3  (inputs)
 * tan4: nv*4 floats                        (output; tangent xyz + handedness w)
 * idx:  ntri*3 indices                                                          */
static inline void ex_compute_tangents(const float *pos, const float *uv,
                                       const float *nrm, float *tan4,
                                       int nv, const unsigned int *idx, int ntri)
{
    if (nv <= 0 || ntri <= 0) return;
    float (*acc)[3] = (float (*)[3])calloc((size_t)nv, sizeof(*acc));
    if (!acc) return;

    for (int t = 0; t < ntri * 3; t += 3) {
        unsigned int a = idx[t], b = idx[t+1], c = idx[t+2];
        if ((int)a >= nv || (int)b >= nv || (int)c >= nv) continue;
        const float *p0 = &pos[a*3], *p1 = &pos[b*3], *p2 = &pos[c*3];
        const float *w0 = &uv[a*2],  *w1 = &uv[b*2],  *w2 = &uv[c*2];
        float e1[3] = { p1[0]-p0[0], p1[1]-p0[1], p1[2]-p0[2] };
        float e2[3] = { p2[0]-p0[0], p2[1]-p0[1], p2[2]-p0[2] };
        float du1 = w1[0]-w0[0], dv1 = w1[1]-w0[1];
        float du2 = w2[0]-w0[0], dv2 = w2[1]-w0[1];
        float det = du1*dv2 - du2*dv1;
        float f = (fabsf(det) < 1e-10f) ? 0.0f : 1.0f/det;
        float tx = f*(dv2*e1[0] - dv1*e2[0]);
        float ty = f*(dv2*e1[1] - dv1*e2[1]);
        float tz = f*(dv2*e1[2] - dv1*e2[2]);
        unsigned int tri[3] = { a, b, c };
        for (int k = 0; k < 3; k++) {
            acc[tri[k]][0] += tx; acc[tri[k]][1] += ty; acc[tri[k]][2] += tz;
        }
    }

    for (int x = 0; x < nv; x++) {
        const float *nin = &nrm[x*3];
        float nl = sqrtf(nin[0]*nin[0] + nin[1]*nin[1] + nin[2]*nin[2]);
        if (nl < 1e-8f) nl = 1.0f;
        float n[3] = { nin[0]/nl, nin[1]/nl, nin[2]/nl };   /* work with a unit normal */
        float *tv = acc[x];
        float d = n[0]*tv[0] + n[1]*tv[1] + n[2]*tv[2];      /* Gram-Schmidt vs normal */
        float ox = tv[0] - n[0]*d, oy = tv[1] - n[1]*d, oz = tv[2] - n[2]*d;
        float len = sqrtf(ox*ox + oy*oy + oz*oz);
        if (len > 1e-6f) {
            ox/=len; oy/=len; oz/=len;
        } else {
            /* degenerate UVs: pick ANY axis not parallel to n, then project it
               onto n's tangent plane so the result is still orthogonal to n. */
            float ax = (fabsf(n[0]) < 0.9f) ? 1.0f : 0.0f;
            float ay = (fabsf(n[0]) < 0.9f) ? 0.0f : 1.0f;
            float ad = ax*n[0] + ay*n[1];
            ox = ax - n[0]*ad; oy = ay - n[1]*ad; oz = -n[2]*ad;
            float l2 = sqrtf(ox*ox+oy*oy+oz*oz); if (l2 < 1e-8f) l2 = 1.0f;
            ox/=l2; oy/=l2; oz/=l2;
        }
        tan4[x*4+0] = ox; tan4[x*4+1] = oy; tan4[x*4+2] = oz; tan4[x*4+3] = 1.0f;
    }
    free(acc);
}
#endif
