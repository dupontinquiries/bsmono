#include "ui/camera/FramePixelOps.hpp"

#include <QLoggingCategory>
#include <cstddef>
#include <cstring>
#include <mutex>
#include <vector>

// Arch gates. NEON is baseline on ARMv8 (aarch64) and present on armv7 + the
// neon flag. SNAPCLONE_PIXELOPS_SCALAR_ONLY forces the portable path everywhere
// (escape hatch if a NEON intrinsic ever fails to compile on the target).
#if (defined(__aarch64__) || defined(__ARM_NEON)) && !defined(SNAPCLONE_PIXELOPS_SCALAR_ONLY)
#  define SC_PIXELOPS_NEON 1
#  include <arm_neon.h>
#endif
#if (defined(__x86_64__) || defined(__i386__)) && !defined(SNAPCLONE_PIXELOPS_SCALAR_ONLY)
#  define SC_PIXELOPS_X86 1
#  include <immintrin.h>
#endif

Q_LOGGING_CATEGORY(lcPixelOps, "snapclone.camera.pixelops")

namespace SnapClone::Camera::PixelOps {
namespace {

// ── scalar reference (always present; also the self-test oracle) ──────────────

void scalar_swapRB(uint8_t *data, int w, int h, int stride) noexcept
{
    for (int y = 0; y < h; ++y) {
        uint8_t *row = data + static_cast<size_t>(y) * stride;
        for (int x = 0; x < w; ++x) {
            uint8_t *px = row + static_cast<size_t>(x) * 4;
            const uint8_t t = px[0];
            px[0] = px[2];
            px[2] = t;
        }
    }
}

// BT.601 limited-range integer YUV→RGB, identical coefficients to the loops the
// backends previously inlined (298/409/100/208/516, +128 rounding, >>8, clamp).
inline void yuvToRgbScalar(int Y, int U, int V, uint8_t *d) noexcept
{
    const int y = Y - 16;
    const int u = U - 128;
    const int v = V - 128;
    const int c = 298 * y;
    int r = (c + 409 * v + 128) >> 8;
    int g = (c - 100 * u - 208 * v + 128) >> 8;
    int b = (c + 516 * u + 128) >> 8;
    if (r < 0) r = 0; else if (r > 255) r = 255;
    if (g < 0) g = 0; else if (g > 255) g = 255;
    if (b < 0) b = 0; else if (b > 255) b = 255;
    d[0] = static_cast<uint8_t>(r);
    d[1] = static_cast<uint8_t>(g);
    d[2] = static_cast<uint8_t>(b);
}

void scalar_nv12(uint8_t *dst, int dstStride, const uint8_t *yP, int yStride,
                 const uint8_t *uvP, int uvStride, int w, int h, bool nv21) noexcept
{
    for (int y = 0; y < h; ++y) {
        const uint8_t *yRow = yP + static_cast<size_t>(y) * yStride;
        const uint8_t *cRow = uvP + static_cast<size_t>(y >> 1) * uvStride;
        uint8_t *d = dst + static_cast<size_t>(y) * dstStride;
        for (int x = 0; x < w; ++x) {
            const int ci = x & ~1;
            int U, V;
            if (nv21) { V = cRow[ci]; U = cRow[ci + 1]; }
            else      { U = cRow[ci]; V = cRow[ci + 1]; }
            yuvToRgbScalar(yRow[x], U, V, d + static_cast<size_t>(x) * 3);
        }
    }
}

void scalar_i420(uint8_t *dst, int dstStride, const uint8_t *yP, int yStride,
                 const uint8_t *uP, int uStride, const uint8_t *vP, int vStride,
                 int w, int h) noexcept
{
    for (int y = 0; y < h; ++y) {
        const uint8_t *yRow = yP + static_cast<size_t>(y) * yStride;
        const uint8_t *uRow = uP + static_cast<size_t>(y >> 1) * uStride;
        const uint8_t *vRow = vP + static_cast<size_t>(y >> 1) * vStride;
        uint8_t *d = dst + static_cast<size_t>(y) * dstStride;
        for (int x = 0; x < w; ++x) {
            yuvToRgbScalar(yRow[x], uRow[x >> 1], vRow[x >> 1],
                           d + static_cast<size_t>(x) * 3);
        }
    }
}

// ── NEON (aarch64 / armv7+neon) ───────────────────────────────────────────────
#ifdef SC_PIXELOPS_NEON

void neon_swapRB(uint8_t *data, int w, int h, int stride) noexcept
{
    const int w16 = w & ~15;
    for (int y = 0; y < h; ++y) {
        uint8_t *row = data + static_cast<size_t>(y) * stride;
        int x = 0;
        for (; x < w16; x += 16) {
            uint8_t *p = row + static_cast<size_t>(x) * 4;
            uint8x16x4_t px = vld4q_u8(p);          // deinterleave B,G,R,A
            const uint8x16_t t = px.val[0];
            px.val[0] = px.val[2];                  // B <-> R
            px.val[2] = t;
            vst4q_u8(p, px);
        }
        for (; x < w; ++x) {
            uint8_t *px = row + static_cast<size_t>(x) * 4;
            const uint8_t t = px[0]; px[0] = px[2]; px[2] = t;
        }
    }
}

// One RGB channel for 16 px: clamp((298*Y + a*U + b*V + 128) >> 8) → uint8x16.
// All arithmetic is widened to int32 (298*255 overflows int16), matching the
// scalar path bit-for-bit; vqmovun (→0 floor) then vqmovn (→255 ceil) reproduce
// the [0,255] clamp exactly.
inline uint8x16_t neon_chan(int16x8_t Yl, int16x8_t Yh,
                            int16x8_t Ul, int16x8_t Uh,
                            int16x8_t Vl, int16x8_t Vh,
                            int16_t a, int16_t b) noexcept
{
    const int32x4_t c128 = vdupq_n_s32(128);
    int32x4_t ll = vmlal_n_s16(vmlal_n_s16(vaddq_s32(vmull_n_s16(vget_low_s16(Yl), 298), c128),
                                           vget_low_s16(Ul), a), vget_low_s16(Vl), b);
    int32x4_t lh = vmlal_n_s16(vmlal_n_s16(vaddq_s32(vmull_n_s16(vget_high_s16(Yl), 298), c128),
                                           vget_high_s16(Ul), a), vget_high_s16(Vl), b);
    int32x4_t hl = vmlal_n_s16(vmlal_n_s16(vaddq_s32(vmull_n_s16(vget_low_s16(Yh), 298), c128),
                                           vget_low_s16(Uh), a), vget_low_s16(Vh), b);
    int32x4_t hh = vmlal_n_s16(vmlal_n_s16(vaddq_s32(vmull_n_s16(vget_high_s16(Yh), 298), c128),
                                           vget_high_s16(Uh), a), vget_high_s16(Vh), b);
    ll = vshrq_n_s32(ll, 8); lh = vshrq_n_s32(lh, 8);
    hl = vshrq_n_s32(hl, 8); hh = vshrq_n_s32(hh, 8);
    const uint16x8_t lo16 = vcombine_u16(vqmovun_s32(ll), vqmovun_s32(lh));
    const uint16x8_t hi16 = vcombine_u16(vqmovun_s32(hl), vqmovun_s32(hh));
    return vcombine_u8(vqmovn_u16(lo16), vqmovn_u16(hi16));
}

inline void neon_yuv_block16(uint8x16_t Y8, uint8x16_t U16, uint8x16_t V16,
                             uint8_t *d) noexcept
{
    const int16x8_t s16 = vdupq_n_s16(16);
    const int16x8_t s128 = vdupq_n_s16(128);
    const int16x8_t Yl = vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(Y8))), s16);
    const int16x8_t Yh = vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(vget_high_u8(Y8))), s16);
    const int16x8_t Ul = vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(U16))), s128);
    const int16x8_t Uh = vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(vget_high_u8(U16))), s128);
    const int16x8_t Vl = vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(V16))), s128);
    const int16x8_t Vh = vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(vget_high_u8(V16))), s128);
    uint8x16x3_t rgb;
    rgb.val[0] = neon_chan(Yl, Yh, Ul, Uh, Vl, Vh, 0, 409);     // R
    rgb.val[1] = neon_chan(Yl, Yh, Ul, Uh, Vl, Vh, -100, -208); // G
    rgb.val[2] = neon_chan(Yl, Yh, Ul, Uh, Vl, Vh, 516, 0);     // B
    vst3q_u8(d, rgb);
}

void neon_nv12(uint8_t *dst, int dstStride, const uint8_t *yP, int yStride,
               const uint8_t *uvP, int uvStride, int w, int h, bool nv21) noexcept
{
    const int w16 = w & ~15;
    for (int y = 0; y < h; ++y) {
        const uint8_t *yRow = yP + static_cast<size_t>(y) * yStride;
        const uint8_t *cRow = uvP + static_cast<size_t>(y >> 1) * uvStride;
        uint8_t *d = dst + static_cast<size_t>(y) * dstStride;
        int x = 0;
        for (; x < w16; x += 16) {
            const uint8x16_t Y8 = vld1q_u8(yRow + x);
            const uint8x8x2_t uv = vld2_u8(cRow + x);      // 8 U + 8 V
            const uint8x8_t Us = nv21 ? uv.val[1] : uv.val[0];
            const uint8x8_t Vs = nv21 ? uv.val[0] : uv.val[1];
            const uint8x8x2_t Uz = vzip_u8(Us, Us);        // duplicate per 2 px
            const uint8x8x2_t Vz = vzip_u8(Vs, Vs);
            neon_yuv_block16(Y8, vcombine_u8(Uz.val[0], Uz.val[1]),
                             vcombine_u8(Vz.val[0], Vz.val[1]),
                             d + static_cast<size_t>(x) * 3);
        }
        for (; x < w; ++x) {
            const int ci = x & ~1;
            int U, V;
            if (nv21) { V = cRow[ci]; U = cRow[ci + 1]; }
            else      { U = cRow[ci]; V = cRow[ci + 1]; }
            yuvToRgbScalar(yRow[x], U, V, d + static_cast<size_t>(x) * 3);
        }
    }
}

void neon_i420(uint8_t *dst, int dstStride, const uint8_t *yP, int yStride,
               const uint8_t *uP, int uStride, const uint8_t *vP, int vStride,
               int w, int h) noexcept
{
    const int w16 = w & ~15;
    for (int y = 0; y < h; ++y) {
        const uint8_t *yRow = yP + static_cast<size_t>(y) * yStride;
        const uint8_t *uRow = uP + static_cast<size_t>(y >> 1) * uStride;
        const uint8_t *vRow = vP + static_cast<size_t>(y >> 1) * vStride;
        uint8_t *d = dst + static_cast<size_t>(y) * dstStride;
        int x = 0;
        for (; x < w16; x += 16) {
            const uint8x16_t Y8 = vld1q_u8(yRow + x);
            const uint8x8_t Us = vld1_u8(uRow + (x >> 1));  // 8 samples → 16 px
            const uint8x8_t Vs = vld1_u8(vRow + (x >> 1));
            const uint8x8x2_t Uz = vzip_u8(Us, Us);
            const uint8x8x2_t Vz = vzip_u8(Vs, Vs);
            neon_yuv_block16(Y8, vcombine_u8(Uz.val[0], Uz.val[1]),
                             vcombine_u8(Vz.val[0], Vz.val[1]),
                             d + static_cast<size_t>(x) * 3);
        }
        for (; x < w; ++x) {
            yuvToRgbScalar(yRow[x], uRow[x >> 1], vRow[x >> 1],
                           d + static_cast<size_t>(x) * 3);
        }
    }
}
#endif // SC_PIXELOPS_NEON

// ── x86-64 / x86 (SSSE3, AVX2) — swap only; YUV stays scalar on desktop ───────
#ifdef SC_PIXELOPS_X86

__attribute__((target("ssse3")))
void ssse3_swapRB(uint8_t *data, int w, int h, int stride) noexcept
{
    // Per 4-byte pixel [b,g,r,a] → [r,g,b,a]: pick src bytes 2,1,0,3.
    const __m128i mask = _mm_set_epi8(15, 12, 13, 14, 11, 8, 9, 10,
                                      7, 4, 5, 6, 3, 0, 1, 2);
    const int w4 = w & ~3;
    for (int y = 0; y < h; ++y) {
        uint8_t *row = data + static_cast<size_t>(y) * stride;
        int x = 0;
        for (; x < w4; x += 4) {
            uint8_t *p = row + static_cast<size_t>(x) * 4;
            __m128i v = _mm_loadu_si128(reinterpret_cast<const __m128i *>(p));
            v = _mm_shuffle_epi8(v, mask);
            _mm_storeu_si128(reinterpret_cast<__m128i *>(p), v);
        }
        for (; x < w; ++x) {
            uint8_t *px = row + static_cast<size_t>(x) * 4;
            const uint8_t t = px[0]; px[0] = px[2]; px[2] = t;
        }
    }
}

__attribute__((target("avx2")))
void avx2_swapRB(uint8_t *data, int w, int h, int stride) noexcept
{
    // Same 16-byte pattern in both 128-bit lanes (shuffle is per-lane; pixels
    // are 4 bytes so they never straddle a lane boundary).
    const __m256i mask = _mm256_set_epi8(15, 12, 13, 14, 11, 8, 9, 10,
                                         7, 4, 5, 6, 3, 0, 1, 2,
                                         15, 12, 13, 14, 11, 8, 9, 10,
                                         7, 4, 5, 6, 3, 0, 1, 2);
    const int w8 = w & ~7;
    for (int y = 0; y < h; ++y) {
        uint8_t *row = data + static_cast<size_t>(y) * stride;
        int x = 0;
        for (; x < w8; x += 8) {
            uint8_t *p = row + static_cast<size_t>(x) * 4;
            __m256i v = _mm256_loadu_si256(reinterpret_cast<const __m256i *>(p));
            v = _mm256_shuffle_epi8(v, mask);
            _mm256_storeu_si256(reinterpret_cast<__m256i *>(p), v);
        }
        for (; x < w; ++x) {
            uint8_t *px = row + static_cast<size_t>(x) * 4;
            const uint8_t t = px[0]; px[0] = px[2]; px[2] = t;
        }
    }
}
#endif // SC_PIXELOPS_X86

// ── dispatch + self-test ──────────────────────────────────────────────────────

using SwapFn = void (*)(uint8_t *, int, int, int);
using Nv12Fn = void (*)(uint8_t *, int, const uint8_t *, int, const uint8_t *, int,
                        int, int, bool);
using I420Fn = void (*)(uint8_t *, int, const uint8_t *, int, const uint8_t *, int,
                        const uint8_t *, int, int, int);

SwapFn g_swap = scalar_swapRB;
Nv12Fn g_nv12 = scalar_nv12;
I420Fn g_i420 = scalar_i420;
const char *g_name = "scalar";
bool g_ok = true;
std::once_flag g_once;

// Validate the *selected* implementations against scalar on a synthetic image
// whose width (18) forces both the wide SIMD body and a 2-px scalar tail, and
// whose values push past 0/255 to exercise clamping. On any mismatch, pin that
// op back to scalar — so an un-host-verified NEON path can never ship wrong
// colors; it can only fail safe.
void runSelfTest() noexcept
{
    constexpr int W = 18, H = 4;
    bool downgraded = false;

    // swapRB: gradient over BGRA.
    {
        std::vector<uint8_t> a(static_cast<size_t>(W) * H * 4), b;
        for (int i = 0; i < W * H * 4; ++i) a[i] = static_cast<uint8_t>(i * 7 + 11);
        b = a;
        scalar_swapRB(a.data(), W, H, W * 4);
        g_swap(b.data(), W, H, W * 4);
        if (a != b) { g_swap = scalar_swapRB; downgraded = true; }
    }

    // Synthetic YUV planes that span the value range (forces clamping).
    std::vector<uint8_t> Y(static_cast<size_t>(W) * H);
    std::vector<uint8_t> UV(static_cast<size_t>(W) * (H / 2));        // NV: w bytes/row
    std::vector<uint8_t> U(static_cast<size_t>(W / 2) * (H / 2));
    std::vector<uint8_t> V(static_cast<size_t>(W / 2) * (H / 2));
    for (size_t i = 0; i < Y.size(); ++i)  Y[i]  = static_cast<uint8_t>(i * 13 + 3);
    for (size_t i = 0; i < UV.size(); ++i) UV[i] = static_cast<uint8_t>(i * 29 + 7);
    for (size_t i = 0; i < U.size(); ++i)  U[i]  = static_cast<uint8_t>(i * 17 + 5);
    for (size_t i = 0; i < V.size(); ++i)  V[i]  = static_cast<uint8_t>(i * 23 + 9);

    // nv12 + nv21.
    for (bool nv21 : {false, true}) {
        std::vector<uint8_t> a(static_cast<size_t>(W) * H * 3), b(a.size());
        scalar_nv12(a.data(), W * 3, Y.data(), W, UV.data(), W, W, H, nv21);
        g_nv12(b.data(), W * 3, Y.data(), W, UV.data(), W, W, H, nv21);
        if (a != b) { g_nv12 = scalar_nv12; downgraded = true; break; }
    }
    // i420.
    {
        std::vector<uint8_t> a(static_cast<size_t>(W) * H * 3), b(a.size());
        scalar_i420(a.data(), W * 3, Y.data(), W, U.data(), W / 2, V.data(), W / 2, W, H);
        g_i420(b.data(), W * 3, Y.data(), W, U.data(), W / 2, V.data(), W / 2, W, H);
        if (a != b) { g_i420 = scalar_i420; downgraded = true; }
    }

    if (downgraded) {
        g_ok = false;
        qCWarning(lcPixelOps) << "SIMD self-test FAILED for backend" << g_name
                              << "— one or more ops downgraded to scalar. Colors are"
                                 " still correct (scalar reference); report this.";
        g_name = "scalar (simd self-test failed)";
    }
}

void doResolve() noexcept
{
    g_swap = scalar_swapRB;
    g_nv12 = scalar_nv12;
    g_i420 = scalar_i420;
    g_name = "scalar";

#if defined(SC_PIXELOPS_NEON)
    g_swap = neon_swapRB;
    g_nv12 = neon_nv12;
    g_i420 = neon_i420;
    g_name = "NEON";
#elif defined(SC_PIXELOPS_X86)
    if (__builtin_cpu_supports("avx2")) {
        g_swap = avx2_swapRB;
        g_name = "AVX2";
    } else if (__builtin_cpu_supports("ssse3")) {
        g_swap = ssse3_swapRB;
        g_name = "SSSE3";
    }
    // YUV→RGB stays scalar on x86: desktop uses the GPU/Qt conversion paths, so
    // the hand-vectorized YUV win lives on the ARM devices that actually hit it.
#endif

    runSelfTest();
    qCInfo(lcPixelOps) << "frame pixel-ops backend:" << g_name;
}

inline void ensureResolved() noexcept { std::call_once(g_once, doResolve); }

} // namespace

// ── public API ────────────────────────────────────────────────────────────────

void swapRB8888(uint8_t *data, int width, int height, int stride) noexcept
{
    if (!data || width <= 0 || height <= 0 || stride < width * 4) return;
    ensureResolved();
    g_swap(data, width, height, stride);
}

void nv12ToRgb888(uint8_t *dst, int dstStride, const uint8_t *yPlane, int yStride,
                  const uint8_t *uvPlane, int uvStride, int width, int height,
                  bool nv21) noexcept
{
    if (!dst || !yPlane || !uvPlane || width <= 0 || height <= 0
        || dstStride < width * 3 || yStride < width || uvStride < width) return;
    ensureResolved();
    g_nv12(dst, dstStride, yPlane, yStride, uvPlane, uvStride, width, height, nv21);
}

void i420ToRgb888(uint8_t *dst, int dstStride, const uint8_t *yPlane, int yStride,
                  const uint8_t *uPlane, int uStride, const uint8_t *vPlane,
                  int vStride, int width, int height) noexcept
{
    if (!dst || !yPlane || !uPlane || !vPlane || width <= 0 || height <= 0
        || dstStride < width * 3 || yStride < width
        || uStride < width / 2 || vStride < width / 2) return;
    ensureResolved();
    g_i420(dst, dstStride, yPlane, yStride, uPlane, uStride, vPlane, vStride, width, height);
}

const char *backendName() noexcept { ensureResolved(); return g_name; }
bool selfTest() noexcept { ensureResolved(); return g_ok; }

} // namespace SnapClone::Camera::PixelOps
