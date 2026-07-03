#include "ui/camera/GlassScrim.hpp"

#include <QPainter>
#include <QPaintEvent>
#include <QResizeEvent>
#include <QLinearGradient>
#include <algorithm>

namespace SnapClone::UI::Camera {

GlassScrim::GlassScrim(Edge edge, QWidget *parent)
    : QWidget(parent)
    , m_edge(edge)
{
    // Taps must fall through to the viewfinder; the control bar above us is the
    // interactive layer. (CameraWidget also sets this, but be self-safe.)
    setAttribute(Qt::WA_TransparentForMouseEvents, true);
    // We paint every pixel we occupy (glass + gradient), so no system bg fill.
    setAttribute(Qt::WA_OpaquePaintEvent, false);
    setAttribute(Qt::WA_NoSystemBackground, true);
}

void GlassScrim::setChromaticShift(int shiftPx)
{
    m_shiftPx = std::max(0, shiftPx);
    rebuildGlass();
    update();
}

void GlassScrim::setBackdrop(const QImage &strip)
{
    m_backdrop = strip;
    rebuildGlass();
    update();
}

void GlassScrim::resizeEvent(QResizeEvent *)
{
    // Geometry changed → the cached glass is the wrong size; rebuild from the
    // last backdrop we were given (if any).
    rebuildGlass();
}

// Cheap separable box blur on a (small) ARGB32 image, radius r in pixels.
// Runs on the DOWNSCALED strip, so r is tiny and the cost is negligible.
static QImage boxBlur(const QImage &srcIn, int r)
{
    if (r < 1) return srcIn;
    QImage src = srcIn.convertToFormat(QImage::Format_ARGB32);
    const int w = src.width(), h = src.height();
    if (w == 0 || h == 0) return src;
    QImage tmp(w, h, QImage::Format_ARGB32);
    const int win = 2 * r + 1;

    // Horizontal pass
    for (int y = 0; y < h; ++y) {
        const QRgb *s = reinterpret_cast<const QRgb *>(src.constScanLine(y));
        QRgb *d = reinterpret_cast<QRgb *>(tmp.scanLine(y));
        for (int x = 0; x < w; ++x) {
            int ra = 0, ga = 0, ba = 0, aa = 0;
            for (int k = -r; k <= r; ++k) {
                const int xx = std::clamp(x + k, 0, w - 1);
                const QRgb p = s[xx];
                ra += qRed(p); ga += qGreen(p); ba += qBlue(p); aa += qAlpha(p);
            }
            d[x] = qRgba(ra / win, ga / win, ba / win, aa / win);
        }
    }
    // Vertical pass
    QImage out(w, h, QImage::Format_ARGB32);
    for (int x = 0; x < w; ++x) {
        for (int y = 0; y < h; ++y) {
            int ra = 0, ga = 0, ba = 0, aa = 0;
            for (int k = -r; k <= r; ++k) {
                const int yy = std::clamp(y + k, 0, h - 1);
                const QRgb p = reinterpret_cast<const QRgb *>(tmp.constScanLine(yy))[x];
                ra += qRed(p); ga += qGreen(p); ba += qBlue(p); aa += qAlpha(p);
            }
            reinterpret_cast<QRgb *>(out.scanLine(y))[x] =
                qRgba(ra / win, ga / win, ba / win, aa / win);
        }
    }
    return out;
}

void GlassScrim::rebuildGlass()
{
    const QSize sz = size();
    if (sz.isEmpty() || m_backdrop.isNull()) { m_glass = QImage(); return; }

    // 1) Downscale the strip. The downscale is most of the blur and makes every
    //    subsequent op run on ~1/(downscale^2) of the pixels — the key to FP5
    //    affordability. Guard against a zero-size tiny image.
    const int dw = std::max(1, sz.width()  / m_downscale);
    const int dh = std::max(1, sz.height() / m_downscale);
    QImage small = m_backdrop.scaled(dw, dh, Qt::IgnoreAspectRatio,
                                     Qt::SmoothTransformation)
                       .convertToFormat(QImage::Format_ARGB32);

    // 2) Frost: a 1px box blur on the tiny image ≈ a big blur at full res.
    small = boxBlur(small, 1);

    // 3) Subtle chromatic shift. Offset in DOWNSCALED space is the final shift
    //    divided by downscale, but we want it visible at small sizes too, so we
    //    shift by at least 1 downscaled px when any shift is requested. Take R
    //    from the left/up-shifted copy, B from the right/down-shifted copy, G
    //    from centre. Horizontal fringe for the bars (they span full width).
    if (m_shiftPx > 0) {
        const int d = std::max(1, m_shiftPx / m_downscale);
        QImage shifted(small.size(), QImage::Format_ARGB32);
        const int sw = small.width(), sh = small.height();
        for (int y = 0; y < sh; ++y) {
            QRgb *o = reinterpret_cast<QRgb *>(shifted.scanLine(y));
            const QRgb *c = reinterpret_cast<const QRgb *>(small.constScanLine(y));
            for (int x = 0; x < sw; ++x) {
                const QRgb rp = c[std::clamp(x - d, 0, sw - 1)]; // R from left
                const QRgb bp = c[std::clamp(x + d, 0, sw - 1)]; // B from right
                const QRgb gp = c[x];                            // G centre
                o[x] = qRgba(qRed(rp), qGreen(gp), qBlue(bp), qAlpha(gp));
            }
        }
        small = shifted;
    }

    // 4) Upscale back to widget size — bilinear smoothing adds free softening.
    m_glass = small.scaled(sz, Qt::IgnoreAspectRatio, Qt::SmoothTransformation);
}

void GlassScrim::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::SmoothPixmapTransform, true);

    const int w = width(), h = height();
    if (w == 0 || h == 0) return;

    // Alpha mask: opaque at the screen edge, fading to fully transparent toward
    // the frame centre, with eased intermediate stops so it feathers (no band).
    // Top scrim fades downward; bottom scrim fades upward (mirror).
    QLinearGradient mask(0, 0, 0, h);
    if (m_edge == Edge::Top) {
        mask.setColorAt(0.0,  QColor(0, 0, 0, 255));
        mask.setColorAt(0.35, QColor(0, 0, 0, 150));
        mask.setColorAt(0.70, QColor(0, 0, 0, 60));
        mask.setColorAt(1.0,  QColor(0, 0, 0, 0));
    } else {
        mask.setColorAt(0.0,  QColor(0, 0, 0, 0));
        mask.setColorAt(0.30, QColor(0, 0, 0, 60));
        mask.setColorAt(0.65, QColor(0, 0, 0, 150));
        mask.setColorAt(1.0,  QColor(0, 0, 0, 255));
    }

    // Draw the frosted+shifted glass through the gradient alpha mask: render the
    // glass into a layer, then multiply its alpha by the gradient so it only
    // shows (and only blurs the perceived backdrop) near the edge.
    if (!m_glass.isNull()) {
        p.save();
        p.drawImage(0, 0, m_glass);
        // DestinationIn with the gradient keeps glass alpha = gradient alpha.
        p.setCompositionMode(QPainter::CompositionMode_DestinationIn);
        p.fillRect(rect(), mask);
        p.restore();
    }

    // Faint dark tint on top (same gradient shape, low alpha) so white controls
    // stay legible over a bright scene without flattening the whole preview.
    QLinearGradient tint(0, 0, 0, h);
    if (m_edge == Edge::Top) {
        tint.setColorAt(0.0, QColor(0, 0, 0, 90));
        tint.setColorAt(1.0, QColor(0, 0, 0, 0));
    } else {
        tint.setColorAt(0.0, QColor(0, 0, 0, 0));
        tint.setColorAt(1.0, QColor(0, 0, 0, 110));
    }
    p.fillRect(rect(), tint);
}

} // namespace SnapClone::UI::Camera
