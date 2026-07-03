// ─── UI/AmbientPaletteDeriver.cpp ────────────────────────────────────────────
//
// V32.29 — see header for design philosophy.
//
// Implementation notes:
//   • Image load uses QImage(path) which auto-detects format. PNGs, JPEGs,
//     WebP-via-plugin all work. Failed loads (corrupt, missing file) just
//     get skipped — we don't propagate errors because the failure mode for
//     an ambient theme is "fall back to the base palette," which is fine.
//   • Downscale to 64x64 with SmoothTransformation. We don't need full
//     resolution for ambient color — 4096 samples after downscale is more
//     than enough for k-means/bucket averaging.
//   • Edge-band mask: rows in [0..3] ∪ [60..63], cols in [0..3] ∪ [60..63].
//     ~4 px each side of a 64 px square. With overlap at corners that's
//     ~952 pixels per image — generous for the bucketing pass.
//   • Saturation threshold 0.20 (HSL). Below this, the pixel is "neutral
//     enough to be backdrop / wall / sky," counted as neutral. Above, it
//     contributes to the accent average. The threshold is tuned for
//     real-world photos which often have a lot of "near-grey but slightly
//     warm" pixels (skin tones, sunset edges) — too low and skin dominates
//     accent.
//   • If a bucket is empty across the WHOLE inbox, we keep base's value
//     for that slot. So an inbox of pure greyscale photos doesn't render
//     a black or random accent; it keeps the base accent unchanged.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/AmbientPaletteDeriver.hpp"

#include <QImage>
#include <QFile>
#include <QFileInfo>
#include <QLoggingCategory>
#include <algorithm>

namespace {
Q_LOGGING_CATEGORY(lcAmb, "snapclone.ui.ambient")

// HSL saturation threshold — pixels below this count as neutral.
constexpr qreal kSatThreshold = 0.20;

// Edge-band width as fraction of downscaled image side.
constexpr int kImageSide = 64;
constexpr int kEdgeWidth = 4;   // ~6.25% of 64

// Format a QColor as "#RRGGBB" (ignore alpha — the Palette struct is hex-only).
QString hexOf(const QColor &c) {
    return QStringLiteral("#%1%2%3")
        .arg(c.red(),   2, 16, QLatin1Char('0'))
        .arg(c.green(), 2, 16, QLatin1Char('0'))
        .arg(c.blue(),  2, 16, QLatin1Char('0'))
        .toUpper();
}

// Sample one image: accumulate sums into colorful and neutral buckets.
// Returns number of samples added.
// Internal helper: given a *prepared* image (already 64x64 ARGB32), bucket
// edge-band pixels by HSL saturation. Both Mode A (paths) and Mode B
// (live QImage) feed in here.
//
// V32.34+: optimized loop. Previously this used QImage::pixelColor()
// per pixel (slow — internal QColor allocation) + full HSL conversion
// via QColor::hslSaturationF + lightnessF (each ~10 multiplies/divides
// of floats). On 4096 pixels that's measurable overhead — 20-40ms on
// FP5 per derive tick, visible as jank when a tick coincides with a
// swipe.
//
// New loop:
//   • Direct scanline access via constScanLine() — no QColor allocation
//   • Integer saturation proxy: max(r,g,b) - min(r,g,b) is the HSL
//     "chroma" which is monotonic in saturation for the [0.10, 0.90]
//     lightness band we care about. The conversion factor between
//     chroma-byte (0-255) and hslSaturationF (0.0-1.0) varies with
//     lightness, but at our 0.20 threshold the band 51 < chroma is a
//     safe proxy that errs slightly conservative (treats some near-
//     threshold colors as neutral). Same overall behavior, ~5x faster.
//   • Lightness via (r+g+b) / 3 normalized — simple integer mean
//     instead of the full Rec.709/HSL transform.
//
// The function is hot-loop heavy and now runs from a worker thread
// (AmbientCameraDriver::onDeriveTick spawns it via QtConcurrent::run),
// so the cost is off the GUI path regardless — but a faster loop also
// means lower battery cost.
int sampleImage(const QImage &img,
                 double &accentR, double &accentG, double &accentB,
                 int    &accentN,
                 double &neutralR, double &neutralG, double &neutralB,
                 int    &neutralN)
{
    if (img.isNull()) return 0;
    // Verify format invariant — caller (prepareForSample) ensures this,
    // but defensive: if a non-ARGB32 image slips through, the scanline
    // bytes won't be at offsets we expect and we'd read garbage.
    if (img.format() != QImage::Format_ARGB32
        && img.format() != QImage::Format_ARGB32_Premultiplied) {
        return 0;
    }
    int added = 0;
    // ARGB32 scanline layout (little-endian): each pixel is 4 bytes,
    // stored as B G R A. Reading as uint32 gives 0xAARRGGBB.
    constexpr int kChromaThreshold = 51;  // 0.20 * 255 ≈ 51
    constexpr int kLumLowThreshold  = 25;  // 0.10 * 255 ≈ 25 (rough mean)
    constexpr int kLumHighThreshold = 230; // 0.90 * 255 ≈ 230

    for (int y = 0; y < kImageSide; ++y) {
        const bool yEdge = (y < kEdgeWidth) || (y >= kImageSide - kEdgeWidth);
        // QRgb is uint32 storing 0xAARRGGBB in CPU native byte order,
        // independent of memory byte layout — so qRed/qGreen/qBlue
        // are the portable way to pull channels from ARGB32. Slightly
        // slower than raw byte offset (one shift+mask per channel) but
        // works on big-endian. The shift+mask is constant-folded and
        // still much faster than QImage::pixelColor's QColor build.
        const QRgb *line = reinterpret_cast<const QRgb *>(img.constScanLine(y));
        for (int x = 0; x < kImageSide; ++x) {
            const bool xEdge = (x < kEdgeWidth) || (x >= kImageSide - kEdgeWidth);
            if (!yEdge && !xEdge) continue;
            const QRgb p = line[x];
            const int r = qRed(p);
            const int g = qGreen(p);
            const int b = qBlue(p);
            const int maxC = std::max({r, g, b});
            const int minC = std::min({r, g, b});
            const int chroma = maxC - minC;
            // Approximate lightness as mean of channels — close enough
            // for the wide [0.10, 0.90] band we use as a filter.
            const int lum = (r + g + b) / 3;
            const bool inLumBand = (lum > kLumLowThreshold
                                    && lum < kLumHighThreshold);
            const bool isSaturated = (chroma >= kChromaThreshold);
            if (isSaturated && inLumBand) {
                accentR += r / 255.0;
                accentG += g / 255.0;
                accentB += b / 255.0;
                ++accentN;
            } else {
                neutralR += r / 255.0;
                neutralG += g / 255.0;
                neutralB += b / 255.0;
                ++neutralN;
            }
            ++added;
        }
    }
    return added;
}

// Prepare a QImage for sampling: scale to 64x64, convert to ARGB32.
// Both paths and live-frame entry points use this normalisation.
QImage prepareForSample(const QImage &src)
{
    if (src.isNull()) return {};
    QImage img = src.scaled(kImageSide, kImageSide,
                              Qt::IgnoreAspectRatio,
                              Qt::SmoothTransformation);
    if (img.format() != QImage::Format_ARGB32 &&
        img.format() != QImage::Format_ARGB32_Premultiplied) {
        img = img.convertToFormat(QImage::Format_ARGB32);
    }
    return img;
}

// Sample one image FROM A FILE PATH. Returns number of samples added.
int sampleImageEdges(const QString &path,
                      double &accentR, double &accentG, double &accentB,
                      int    &accentN,
                      double &neutralR, double &neutralG, double &neutralB,
                      int    &neutralN)
{
    QImage img(path);
    if (img.isNull()) {
        qCDebug(lcAmb) << "skip (load failed):" << path;
        return 0;
    }
    img = prepareForSample(img);
    return sampleImage(img, accentR, accentG, accentB, accentN,
                             neutralR, neutralG, neutralB, neutralN);
}

// Build a color from sums + count. Returns invalid QColor if count is 0.
QColor avgColor(double r, double g, double b, int n) {
    if (n <= 0) return QColor();
    // Qt6 fromRgbF takes float, not double — cast after the bound check.
    return QColor::fromRgbF(static_cast<float>(qBound(0.0, r / n, 1.0)),
                             static_cast<float>(qBound(0.0, g / n, 1.0)),
                             static_cast<float>(qBound(0.0, b / n, 1.0)));
}

// Adjust a QColor's lightness in HSL space, clamped.
// Qt6 getHslF/fromHslF use float*, not qreal (double*).
QColor adjustLightness(const QColor &c, float delta) {
    float h, s, l, a;
    c.getHslF(&h, &s, &l, &a);
    l = qBound(0.0f, l + delta, 1.0f);
    return QColor::fromHslF(h, s, l, a);
}

// Adjust saturation similarly.
QColor adjustSaturation(const QColor &c, float factor) {
    float h, s, l, a;
    c.getHslF(&h, &s, &l, &a);
    s = qBound(0.0f, s * factor, 1.0f);
    return QColor::fromHslF(h, s, l, a);
}

// Build a Palette that overrides only the "ambient" slots of `base`.
// Returns by value because the caller will copy it into the synthetic
// storage anyway. The returned Palette's const char* fields point into
// the thread_local sHexBytes list below — caller MUST copy them via
// Theme::setSyntheticPalette before another call to this helper, which
// clears the list. (In practice, fromImagePaths/fromAccentColor return
// directly to setSyntheticPalette, so the lifetime is bounded.)
SnapClone::UI::Theme::Palette buildAmbientPalette(
    const QColor &accent,
    const QColor &background,
    const SnapClone::UI::Theme::Palette &base)
{
    using namespace SnapClone::UI::Theme;

    // Validate inputs — if invalid, fall back to base for that slot.
    const bool haveAccent = accent.isValid();
    const bool haveBg     = background.isValid();

    Palette p = base; // start from base; override selected slots

    // Helper that pushes a QByteArray (UTF-8) and returns its data pointer.
    // The thread_local static is cleared on every entry so each call's
    // strings have a known lifetime (until the next call to this helper).
    static thread_local QList<QByteArray> sHexBytes;
    sHexBytes.clear();
    auto pushBytes = [](const QString &s) -> const char * {
        sHexBytes.append(s.toLatin1());
        return sHexBytes.last().constData();
    };

    // BACKGROUND family — derive from `background` but ANCHOR to the base
    // palette's lightness for readability. V32.30 fix: previously we just
    // adjusted the derived bg's lightness, which produced fully-saturated
    // backgrounds that murdered text contrast. Now we keep base's overall
    // dark/light feel (so text colors still work) and only borrow the
    // *hue* of the ambient sample at low saturation. Splotches of color
    // are added separately by AmbientBlobs overlay, not by tinting the
    // whole canvas heavily.
    if (haveBg) {
        // Pull base's BG_BASE into HSL to use as the lightness anchor.
        const QColor baseBg = QColor(QString::fromLatin1(base.BG_BASE));
        float baseH = 0.f, baseS = 0.f, baseL = 0.5f, baseA = 1.f;
        if (baseBg.isValid()) baseBg.getHslF(&baseH, &baseS, &baseL, &baseA);

        // Pull derived background HSL.
        float bgH = 0.f, bgS = 0.f, bgL = 0.5f, bgA = 1.f;
        background.getHslF(&bgH, &bgS, &bgL, &bgA);

        // Blend recipe: keep base lightness (legibility), borrow ambient
        // hue, cap saturation at 0.18 (a quiet tint, not a wash).
        constexpr float kTintMaxSat = 0.18f;
        const float ambientSat = qBound(0.0f, bgS, kTintMaxSat);

        auto mkTinted = [&](float lDelta) {
            const float l = qBound(0.02f, baseL + lDelta, 0.98f);
            return QColor::fromHslF(bgH, ambientSat, l, 1.0f);
        };

        const QColor bgBase   = mkTinted(0.00f);
        const QColor bgCard   = mkTinted(+0.02f);
        const QColor bgRaised = mkTinted(+0.05f);
        const QColor bgHover  = mkTinted(+0.08f);
        const QColor bgDeep   = mkTinted(-0.04f);
        p.BG_DEEP   = pushBytes(hexOf(bgDeep));
        p.BG_BASE   = pushBytes(hexOf(bgBase));
        p.BG_CARD   = pushBytes(hexOf(bgCard));
        p.BG_RAISED = pushBytes(hexOf(bgRaised));
        p.BG_HOVER  = pushBytes(hexOf(bgHover));
    }

    // ACCENT family — derive from `accent`.
    if (haveAccent) {
        const QColor acc      = accent;
        const QColor accSoft  = adjustLightness(accent, +0.15f);
        const QColor accDim   = adjustSaturation(
                                 adjustLightness(accent, -0.10f), 0.5f);
        p.ACCENT      = pushBytes(hexOf(acc));
        p.ACCENT_SOFT = pushBytes(hexOf(accSoft));
        p.ACCENT_DIM  = pushBytes(hexOf(accDim));
        // Gradients — hero is the splash; bar is the swipe bar.
        // Both blend from a slightly-darkened accent to background, giving
        // the page chrome a subtle ambient tint without overwhelming.
        const QColor heroFrom = haveBg
            ? adjustLightness(background, -0.05f)
            : accDim;
        const QColor heroTo = haveBg ? background : acc;
        p.GRAD_HERO_FROM = pushBytes(hexOf(heroFrom));
        p.GRAD_HERO_TO   = pushBytes(hexOf(heroTo));
        p.GRAD_BAR_FROM  = pushBytes(hexOf(heroFrom));
        p.GRAD_BAR_TO    = pushBytes(hexOf(heroTo));
        // Online-glow inherits accent so the "alive" indicator matches
        // the ambient vibe.
        p.GLOW_ONLINE = pushBytes(hexOf(adjustLightness(acc, +0.05f)));
    }

    return p;
}
} // namespace

namespace SnapClone::UI {

Theme::Palette AmbientPaletteDeriver::fromImagePaths(
    const QStringList &paths, const Theme::Palette &base)
{
    if (paths.isEmpty()) {
        qCDebug(lcAmb) << "no input paths — returning base palette";
        return base;
    }

    double accentR = 0, accentG = 0, accentB = 0;
    int    accentN = 0;
    double neutralR = 0, neutralG = 0, neutralB = 0;
    int    neutralN = 0;
    int    imagesUsed = 0;
    int    maxImages = 12; // cap so a giant inbox doesn't grind the UI

    for (const QString &p : paths) {
        if (imagesUsed >= maxImages) break;
        if (!QFile::exists(p)) continue;
        const int n = sampleImageEdges(p,
                                        accentR, accentG, accentB, accentN,
                                        neutralR, neutralG, neutralB, neutralN);
        if (n > 0) ++imagesUsed;
    }

    if (imagesUsed == 0) {
        qCInfo(lcAmb) << "no usable images out of" << paths.size()
                      << "candidates — keeping base palette";
        return base;
    }

    const QColor accent     = avgColor(accentR, accentG, accentB, accentN);
    const QColor background = avgColor(neutralR, neutralG, neutralB, neutralN);

    qCInfo(lcAmb) << "ambient palette derived from" << imagesUsed << "images:"
                  << "accent=" << (accent.isValid()  ? hexOf(accent)     : "(none)")
                  << "bg="     << (background.isValid() ? hexOf(background) : "(none)")
                  << "samples accent/neutral=" << accentN << "/" << neutralN;

    return buildAmbientPalette(accent, background, base);
}

Theme::Palette AmbientPaletteDeriver::fromImage(
    const QImage &frame, const Theme::Palette &base)
{
    const AmbientColors c = deriveColors(frame);
    if (!c.accent.isValid() && !c.background.isValid()) return base;
    return buildAmbientPalette(c.accent, c.background, base);
}

AmbientPaletteDeriver::AmbientColors AmbientPaletteDeriver::deriveColors(
    const QImage &frame)
{
    AmbientColors out;
    if (frame.isNull()) return out;

    const QImage prepared = prepareForSample(frame);
    if (prepared.isNull()) return out;

    double accentR = 0, accentG = 0, accentB = 0;
    int    accentN = 0;
    double neutralR = 0, neutralG = 0, neutralB = 0;
    int    neutralN = 0;

    if (sampleImage(prepared,
                     accentR, accentG, accentB, accentN,
                     neutralR, neutralG, neutralB, neutralN) == 0) {
        return out;
    }
    out.accent     = avgColor(accentR, accentG, accentB, accentN);
    out.background = avgColor(neutralR, neutralG, neutralB, neutralN);
    return out;
}

Theme::Palette AmbientPaletteDeriver::fromColors(
    const QColor &accent, const QColor &background,
    const Theme::Palette &base)
{
    if (!accent.isValid() && !background.isValid()) return base;
    return buildAmbientPalette(accent, background, base);
}

Theme::Palette AmbientPaletteDeriver::fromAccentColor(
    const QColor &accent, const Theme::Palette &base)
{
    // Background derived by darkening the accent heavily and desaturating —
    // gives a "tinted dark" feel without picking an arbitrary grey.
    QColor bg;
    if (accent.isValid()) {
        bg = adjustSaturation(adjustLightness(accent, -0.45f), 0.35f);
    }
    return buildAmbientPalette(accent, bg, base);
}

} // namespace SnapClone::UI
