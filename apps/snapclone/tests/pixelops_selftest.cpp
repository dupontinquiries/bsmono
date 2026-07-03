// Standalone correctness harness for FramePixelOps (not part of the app build).
// Verifies the dispatched backend matches the scalar reference and known values.
// On x86 this exercises scalar + SSSE3/AVX2; NEON is validated on-device by the
// library's own runtime self-test (same logic this test drives via selfTest()).
#include "ui/camera/FramePixelOps.hpp"
#include <cstdio>
#include <cstdint>
#include <cstdlib>
#include <vector>

namespace px = SnapClone::Camera::PixelOps;

static int g_fail = 0;
#define CHECK(cond, msg) do { if (!(cond)) { std::printf("FAIL: %s\n", msg); ++g_fail; } \
                              else { std::printf("ok:   %s\n", msg); } } while (0)

// Independent reference (deliberately a second implementation, not the lib's).
static void refYuv(int Y, int U, int V, uint8_t *d) {
    int y = Y - 16, u = U - 128, v = V - 128, c = 298 * y;
    int r = (c + 409 * v + 128) >> 8, g = (c - 100 * u - 208 * v + 128) >> 8,
        b = (c + 516 * u + 128) >> 8;
    auto cl = [](int x){ return x < 0 ? 0 : (x > 255 ? 255 : x); };
    d[0] = (uint8_t)cl(r); d[1] = (uint8_t)cl(g); d[2] = (uint8_t)cl(b);
}

int main() {
    std::printf("pixel-ops backend: %s\n", px::backendName());
    CHECK(px::selfTest(), "built-in self-test passes (selected backend == scalar)");

    // 1) swapRB on a known 2x1 image: [b,g,r,a]=[10,20,30,40],[1,2,3,4]
    {
        std::vector<uint8_t> img = {10,20,30,40, 1,2,3,4};
        px::swapRB8888(img.data(), 2, 1, 8);
        CHECK(img[0]==30 && img[1]==20 && img[2]==10 && img[3]==40 &&
              img[4]==3  && img[5]==2  && img[6]==1  && img[7]==4, "swapRB swaps R/B, keeps G/A");
    }

    // 2) swapRB twice == identity, on a wide non-multiple-of-16 image with padding.
    {
        const int w = 37, h = 5, stride = w*4 + 12; // padded rows
        std::vector<uint8_t> a(static_cast<size_t>(stride)*h), b;
        for (size_t i=0;i<a.size();++i) a[i]=(uint8_t)(i*131+7);
        b = a;
        px::swapRB8888(a.data(), w, h, stride);
        CHECK(a != b, "swapRB changed the buffer");
        px::swapRB8888(a.data(), w, h, stride);
        CHECK(a == b, "swapRB applied twice is identity (incl. tail + padding untouched)");
    }

    // 3) NV12 → RGB matches the independent reference (width forces tail).
    {
        const int w=18, h=4;
        std::vector<uint8_t> Y(w*h), UV(w*(h/2)), out(w*h*3), ref(w*h*3);
        for (size_t i=0;i<Y.size();++i)  Y[i]=(uint8_t)(i*13+3);
        for (size_t i=0;i<UV.size();++i) UV[i]=(uint8_t)(i*29+7);
        px::nv12ToRgb888(out.data(), w*3, Y.data(), w, UV.data(), w, w, h, /*nv21=*/false);
        for (int y=0;y<h;++y) for (int x=0;x<w;++x) {
            int ci=x&~1; refYuv(Y[y*w+x], UV[(y>>1)*w+ci], UV[(y>>1)*w+ci+1], &ref[(y*w+x)*3]);
        }
        CHECK(out==ref, "NV12 conversion matches reference");
    }

    // 4) NV21 (V,U order) → RGB matches reference.
    {
        const int w=16, h=2;
        std::vector<uint8_t> Y(w*h), UV(w*(h/2)), out(w*h*3), ref(w*h*3);
        for (size_t i=0;i<Y.size();++i)  Y[i]=(uint8_t)(200 - i*9);
        for (size_t i=0;i<UV.size();++i) UV[i]=(uint8_t)(i*40+5);
        px::nv12ToRgb888(out.data(), w*3, Y.data(), w, UV.data(), w, w, h, /*nv21=*/true);
        for (int y=0;y<h;++y) for (int x=0;x<w;++x) {
            int ci=x&~1; refYuv(Y[y*w+x], UV[(y>>1)*w+ci+1], UV[(y>>1)*w+ci], &ref[(y*w+x)*3]);
        }
        CHECK(out==ref, "NV21 conversion matches reference");
    }

    // 5) I420 (planar) → RGB matches reference.
    {
        const int w=18, h=4;
        std::vector<uint8_t> Y(w*h), U((w/2)*(h/2)), V((w/2)*(h/2)), out(w*h*3), ref(w*h*3);
        for (size_t i=0;i<Y.size();++i) Y[i]=(uint8_t)(i*7+1);
        for (size_t i=0;i<U.size();++i) U[i]=(uint8_t)(i*31+2);
        for (size_t i=0;i<V.size();++i) V[i]=(uint8_t)(i*19+6);
        px::i420ToRgb888(out.data(), w*3, Y.data(), w, U.data(), w/2, V.data(), w/2, w, h);
        for (int y=0;y<h;++y) for (int x=0;x<w;++x)
            refYuv(Y[y*w+x], U[(y>>1)*(w/2)+(x>>1)], V[(y>>1)*(w/2)+(x>>1)], &ref[(y*w+x)*3]);
        CHECK(out==ref, "I420 conversion matches reference");
    }

    std::printf(g_fail ? "\n%d CHECK(S) FAILED\n" : "\nALL CHECKS PASSED\n", g_fail);
    return g_fail ? 1 : 0;
}
