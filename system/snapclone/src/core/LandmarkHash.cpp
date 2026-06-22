// ─── Core/LandmarkHash.cpp ───────────────────────────────────────────────────
// V32.31 — see header.
// ─────────────────────────────────────────────────────────────────────────────

#include "core/LandmarkHash.hpp"

#include <QImage>
#include <QtMath>

namespace SnapClone::Core::LandmarkHash {

namespace {

// Step 1 helper: downscale to a working size and convert to ARGB32 so
// per-pixel reads are uniform. Used by both hash variants.
QImage prepare(const QImage &src, int side)
{
    if (src.isNull()) return {};
    QImage img = src.scaled(side, side, Qt::IgnoreAspectRatio,
                              Qt::SmoothTransformation);
    if (img.format() != QImage::Format_ARGB32 &&
        img.format() != QImage::Format_ARGB32_Premultiplied) {
        img = img.convertToFormat(QImage::Format_ARGB32);
    }
    return img;
}

// Heavy blur: separable gaussian σ≈sigma applied as a small box-blur
// approximation. We use a 3-pass box blur because the actual Gaussian
// kernel at σ=8 needs a 49-tap filter and we want to keep this cheap.
// 3 passes of a 9-wide box approximates σ≈√(3·(9²-1)/12) ≈ 4.5 which
// is plenty for our "destroy content, preserve silhouette" goal.
QImage heavyBlur(const QImage &src)
{
    if (src.isNull()) return src;
    const int W = src.width();
    const int H = src.height();
    QImage out = src;
    constexpr int kRadius = 4; // 9-tap box (2r+1)

    auto blurH = [&](QImage &img) {
        QImage tmp = img;
        for (int y = 0; y < H; ++y) {
            for (int x = 0; x < W; ++x) {
                int r = 0, g = 0, b = 0, n = 0;
                for (int dx = -kRadius; dx <= kRadius; ++dx) {
                    const int xs = qBound(0, x + dx, W - 1);
                    const QRgb px = img.pixel(xs, y);
                    r += qRed(px);
                    g += qGreen(px);
                    b += qBlue(px);
                    ++n;
                }
                tmp.setPixel(x, y, qRgb(r / n, g / n, b / n));
            }
        }
        img = tmp;
    };
    auto blurV = [&](QImage &img) {
        QImage tmp = img;
        for (int x = 0; x < W; ++x) {
            for (int y = 0; y < H; ++y) {
                int r = 0, g = 0, b = 0, n = 0;
                for (int dy = -kRadius; dy <= kRadius; ++dy) {
                    const int ys = qBound(0, y + dy, H - 1);
                    const QRgb px = img.pixel(x, ys);
                    r += qRed(px);
                    g += qGreen(px);
                    b += qBlue(px);
                    ++n;
                }
                tmp.setPixel(x, y, qRgb(r / n, g / n, b / n));
            }
        }
        img = tmp;
    };

    // 3 passes (separable horizontal + vertical each pass).
    for (int p = 0; p < 3; ++p) {
        blurH(out);
        blurV(out);
    }
    return out;
}

// aHash: average luminance of an 8x8 downscale; bit set per cell where
// the cell's value >= the global average. 64 bits → 8 bytes.
QByteArray averageHashFrom8x8(const QImage &smallImg)
{
    QByteArray out(8, 0);
    // First pass: global mean.
    int sum = 0;
    int n = 0;
    for (int y = 0; y < 8; ++y) {
        for (int x = 0; x < 8; ++x) {
            const QRgb px = smallImg.pixel(x, y);
            // Rec. 709 luma — closer to perceived brightness than naive avg.
            const int lum = (qRed(px) * 218 + qGreen(px) * 732 + qBlue(px) * 74) >> 10;
            sum += lum;
            ++n;
        }
    }
    if (n == 0) return out;
    const int mean = sum / n;
    // Second pass: emit bits.
    for (int y = 0; y < 8; ++y) {
        for (int x = 0; x < 8; ++x) {
            const QRgb px = smallImg.pixel(x, y);
            const int lum = (qRed(px) * 218 + qGreen(px) * 732 + qBlue(px) * 74) >> 10;
            if (lum >= mean) {
                const int bit = y * 8 + x;
                out[bit / 8] = static_cast<char>(
                    static_cast<unsigned char>(out[bit / 8])
                    | (1u << (7 - (bit % 8))));
            }
        }
    }
    return out;
}

// V32.31 (revised per user feedback): a *lighter* blur used as a pre-pass
// before Sobel edge detection. Single 5-tap box (σ ≈ √((5²-1)/12) ≈ 1.4).
// Heavy enough to destroy high-frequency content (text glyphs, faces,
// pixel-level noise) while preserving the macro-scale lines that make
// a landmark visually distinctive — building rooflines, horizon, tree
// silhouettes. Privacy-by-design: text and faces are blasted before
// any edge extraction sees them.
QImage lightBlur(const QImage &src)
{
    if (src.isNull()) return src;
    const int W = src.width();
    const int H = src.height();
    constexpr int kRadius = 2; // 5-tap

    auto blurH = [&](const QImage &in) {
        QImage out(in.size(), in.format());
        for (int y = 0; y < H; ++y) {
            for (int x = 0; x < W; ++x) {
                int r = 0, g = 0, b = 0, n = 0;
                for (int dx = -kRadius; dx <= kRadius; ++dx) {
                    const int xs = qBound(0, x + dx, W - 1);
                    const QRgb px = in.pixel(xs, y);
                    r += qRed(px); g += qGreen(px); b += qBlue(px); ++n;
                }
                out.setPixel(x, y, qRgb(r / n, g / n, b / n));
            }
        }
        return out;
    };
    auto blurV = [&](const QImage &in) {
        QImage out(in.size(), in.format());
        for (int x = 0; x < W; ++x) {
            for (int y = 0; y < H; ++y) {
                int r = 0, g = 0, b = 0, n = 0;
                for (int dy = -kRadius; dy <= kRadius; ++dy) {
                    const int ys = qBound(0, y + dy, H - 1);
                    const QRgb px = in.pixel(x, ys);
                    r += qRed(px); g += qGreen(px); b += qBlue(px); ++n;
                }
                out.setPixel(x, y, qRgb(r / n, g / n, b / n));
            }
        }
        return out;
    };
    // Single H + V pass — separable approximation of one Gaussian. Two
    // passes (as heavyBlur does) would push σ to ~2 and start eating the
    // macro structure we want to keep.
    return blurV(blurH(src));
}

// Sobel edge magnitude per pixel of the source image. Returns an
// 8-bit greyscale QImage same size as input with magnitude in [0..255].
QImage sobelEdges(const QImage &src)
{
    const int W = src.width();
    const int H = src.height();
    QImage out(W, H, QImage::Format_Grayscale8);
    out.fill(0);

    // Cache luminance plane to avoid recomputing per-kernel-tap.
    QByteArray lumPlane(W * H, 0);
    for (int y = 0; y < H; ++y) {
        for (int x = 0; x < W; ++x) {
            const QRgb px = src.pixel(x, y);
            const int l = (qRed(px) * 218 + qGreen(px) * 732 + qBlue(px) * 74) >> 10;
            lumPlane[y * W + x] = static_cast<char>(qBound(0, l, 255));
        }
    }
    auto L = [&](int x, int y) -> int {
        x = qBound(0, x, W - 1);
        y = qBound(0, y, H - 1);
        return static_cast<unsigned char>(lumPlane[y * W + x]);
    };
    for (int y = 0; y < H; ++y) {
        uchar *row = out.scanLine(y);
        for (int x = 0; x < W; ++x) {
            const int gx = -L(x-1,y-1) - 2*L(x-1,y) - L(x-1,y+1)
                          + L(x+1,y-1) + 2*L(x+1,y) + L(x+1,y+1);
            const int gy = -L(x-1,y-1) - 2*L(x,y-1) - L(x+1,y-1)
                          + L(x-1,y+1) + 2*L(x,y+1) + L(x+1,y+1);
            const int mag = qBound(0, (qAbs(gx) + qAbs(gy)) / 2, 255);
            row[x] = static_cast<uchar>(mag);
        }
    }
    return out;
}

} // namespace

QByteArray blurHash(const QImage &img)
{
    if (img.isNull()) return {};
    // Working size 32x32. Heavy blur on 32x32 is cheap (~3·2·1024 reads
    // = ~6k pixels per pass × 3 passes = ~18k pixel reads — milliseconds).
    QImage prep = prepare(img, 32);
    if (prep.isNull()) return {};
    prep = heavyBlur(prep);
    // Downscale to the 8x8 we hash.
    QImage tiny = prep.scaled(8, 8, Qt::IgnoreAspectRatio,
                                Qt::SmoothTransformation);
    return averageHashFrom8x8(tiny);
}

QByteArray edgeHash(const QImage &img)
{
    if (img.isNull()) return {};
    // Working size 64x64. Sobel needs the bigger canvas for meaningful
    // edges; otherwise the 8-bit threshold collapses to all-noise.
    QImage prep = prepare(img, 64);
    if (prep.isNull()) return {};
    // V32.31 (revised): blur BEFORE Sobel. Destroys text/faces/fine
    // texture before any edge information is extracted. The remaining
    // signal is macro-scale lines that survive blur (rooflines, horizon,
    // silhouettes) — exactly the structure that makes a landmark a
    // landmark. Privacy-by-design: an attacker reconstructing from the
    // edge map can recover roughly "where is the building outline" —
    // never the storefront sign that was attached to it.
    prep = lightBlur(prep);
    const QImage edges = sobelEdges(prep);

    // Reduce to an 8-wide × 4-tall grid (32 cells = 32 bits = 4 bytes).
    // Each cell is the mean edge magnitude of an 8x16 patch. Threshold
    // against the global mean → bit set if cell is "edgier than average."
    // This makes the hash invariant to absolute brightness and overall
    // edge density — captures WHERE edges concentrate in the frame.
    constexpr int kCellsX = 8;
    constexpr int kCellsY = 4;
    constexpr int kCellW  = 64 / kCellsX; // 8
    constexpr int kCellH  = 64 / kCellsY; // 16
    int cells[kCellsX * kCellsY] = {};
    int gSum = 0;
    for (int cy = 0; cy < kCellsY; ++cy) {
        for (int cx = 0; cx < kCellsX; ++cx) {
            int sum = 0;
            for (int y = cy * kCellH; y < (cy + 1) * kCellH; ++y) {
                const uchar *row = edges.constScanLine(y);
                for (int x = cx * kCellW; x < (cx + 1) * kCellW; ++x) {
                    sum += row[x];
                }
            }
            const int avg = sum / (kCellW * kCellH);
            cells[cy * kCellsX + cx] = avg;
            gSum += avg;
        }
    }
    const int gMean = gSum / (kCellsX * kCellsY);

    QByteArray out(4, 0);
    for (int i = 0; i < kCellsX * kCellsY; ++i) {
        if (cells[i] > gMean) {
            out[i / 8] = static_cast<char>(
                static_cast<unsigned char>(out[i / 8])
                | (1u << (7 - (i % 8))));
        }
    }
    return out;
}

int hammingDistance(const QByteArray &a, const QByteArray &b)
{
    if (a.size() != b.size()) return -1;
    int d = 0;
    for (int i = 0; i < a.size(); ++i) {
        unsigned char x = static_cast<unsigned char>(a[i]) ^
                          static_cast<unsigned char>(b[i]);
        // popcount (avoid std::popcount C++20 dep that may not be enabled)
        while (x) { d += (x & 1); x >>= 1; }
    }
    return d;
}

} // namespace SnapClone::Core::LandmarkHash
