// ─── UI/AmbientBlobs.cpp ─────────────────────────────────────────────────────
//
// V32.30 — see header for design philosophy.
//
// Implementation notes:
//   • The blobs only PAINT when Theme::isSyntheticActive() returns true.
//     Static palettes get a no-op paintEvent — zero cost. Reason: blobs
//     are an "ambient mode" affordance; on the carefully-tuned static
//     palettes they'd just add noise.
//   • Each blob has independent Lissajous-style drift (separate x/y
//     frequencies with irrational-ratio periods) so paths never quite
//     repeat. The user sees motion without recognizable pattern.
//   • Opacity caps at 0.18 — same value as the background tint saturation
//     cap in AmbientPaletteDeriver. Both numbers were chosen to keep text
//     contrast in the safe zone (WCAG AA-equivalent for the dark
//     palettes we ship). Going higher kills readability — which is the
//     whole reason these blobs exist instead of brute-force tinting.
//   • Pulse cycle: 6 seconds (radius), 8 seconds (opacity) — slightly
//     offset so the visual "beat" is irregular. Drift cycle: 45 seconds
//     on x, 67 seconds on y — irrational ratio.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/AmbientBlobs.hpp"
#include "ui/Theme.hpp"

#include <QPainter>
#include <QRadialGradient>
#include <QTimer>
#include <QDateTime>
#include <QPaintEvent>
#include <QShowEvent>
#include <QHideEvent>
#include <QResizeEvent>
#include <QRandomGenerator>
#include "core/ActivityGate.hpp"  // V32.39+: shared foreground+screen-off gate
#include <cmath>

namespace SnapClone::UI {

AmbientBlobs::AmbientBlobs(QWidget *parent)
    : QWidget(parent)
{
    // Don't intercept input — the blobs are pure decoration. All clicks /
    // touches pass through to whatever's underneath.
    setAttribute(Qt::WA_TransparentForMouseEvents, true);
    // Transparent background — only the blobs themselves draw, the rest
    // of the widget surface is see-through so the host's BG_BASE shows.
    setAttribute(Qt::WA_StyledBackground, false);
    setAttribute(Qt::WA_NoSystemBackground, true);
    setAttribute(Qt::WA_OpaquePaintEvent, false);

    seedBlobs();

    m_timer = new QTimer(this);
    // V32.31: dropped from 30fps to 10fps. The blobs drift on 45-70s
    // periods and pulse on 6-10s periods — at 30fps each pixel moves
    // <0.1px between ticks, way below human perception. 10fps is
    // visually identical for slow-drift content and cuts CPU/GPU
    // load by 3×. Major framerate win when ambient mode is on,
    // since paintEvent does a full-screen radial-gradient eval per
    // blob (Qt's software rasterizer paints those pixel-by-pixel).
    m_timer->setInterval(100); // 10fps
    connect(m_timer, &QTimer::timeout, this, &AmbientBlobs::onTick);

    // V32.39+ battery: gate the animation on the process-wide ActivityGate.
    // Qt::WidgetVisible is "would be visible if its window were shown" — it
    // does NOT reflect whether the app is in the foreground. Without gating, a
    // backgrounded app keeps repainting the blobs at 10fps (CPU + GPU wakeups
    // every 100ms even though no pixels reach the screen).
    //
    // The gate folds in BOTH app foreground state AND screen-off — the latter
    // is new here; previously the blobs kept ticking with the screen dark
    // because this widget only watched applicationState. m_appActive now means
    // "interactive" = foreground ∧ screen on.
    {
        auto *gate = Core::ActivityGate::instance();
        m_appActive = gate->interactive();
        connect(gate, &Core::ActivityGate::interactiveChanged,
                this, [this](bool nowActive) {
            if (nowActive == m_appActive) return;
            m_appActive = nowActive;
            if (!m_timer) return;
            if (m_appActive && m_active && isVisible()) {
                m_lastTickMs = 0;  // reset dt to avoid jump on resume
                m_timer->start();
            } else {
                m_timer->stop();
            }
        });
    }
}

AmbientBlobs::~AmbientBlobs() = default;

void AmbientBlobs::setBlobCount(int n)
{
    m_blobCount = qBound(1, n, 5);
    seedBlobs();
    update();
}

void AmbientBlobs::setSpeed(float multiplier)
{
    m_speed = qBound(0.1f, multiplier, 4.0f);
}

void AmbientBlobs::setActive(bool active)
{
    if (m_active == active) return;
    m_active = active;
    if (!m_timer) return;
    if (active) {
        // V32.34+: only start if app is foreground AND we're visible.
        // Without this guard, setActive(true) called while backgrounded
        // (e.g. when the user toggles a setting via a quick-actions
        // panel) would start the timer, which the next tick would
        // stop on the isVisible check — but only AFTER one wasted
        // wakeup. Composing with both flags keeps things tight.
        if (m_appActive && isVisible() && !m_timer->isActive()) {
            m_lastTickMs = 0;  // reset dt so we don't jump on resume
            m_timer->start();
        }
        update();
    } else {
        m_timer->stop();
        update();  // schedule a final clear-paint (m_active=false → no draw)
    }
}

void AmbientBlobs::seedBlobs()
{
    m_blobs.clear();
    auto *rng = QRandomGenerator::global();
    constexpr float kTwoPi = 6.28318530718f;
    // Seed with intentionally varied frequencies / phases. Magic numbers
    // here are tuned for "calmly wandering" — each blob takes 30-70s to
    // traverse the canvas. Pulse uses 5-10s periods.
    for (int i = 0; i < m_blobCount; ++i) {
        Blob b;
        b.xPhase     = rng->generateDouble() * kTwoPi;
        b.yPhase     = rng->generateDouble() * kTwoPi;
        // 45s..70s drift period (radians per second = 2π / period)
        b.xFreq      = kTwoPi / (45.0f + rng->generateDouble() * 25.0f);
        b.yFreq      = kTwoPi / (45.0f + rng->generateDouble() * 25.0f);
        b.pulsePhase = rng->generateDouble() * kTwoPi;
        // 6s..10s pulse period
        b.pulseFreq  = kTwoPi / (6.0f + rng->generateDouble() * 4.0f);
        b.colorIdx   = i % 4;
        // Base radius 30%..55% of min(width, height) — large enough that
        // a single blob covers a meaningful area but never the whole canvas.
        b.baseRadiusF = 0.30f + rng->generateDouble() * 0.25f;
        m_blobs.append(b);
    }
}

void AmbientBlobs::onTick()
{
    if (!isVisible()) {
        m_timer->stop();
        return;
    }
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    const float dt = (m_lastTickMs == 0)
        ? 0.033f
        : qBound(0.0f, (now - m_lastTickMs) / 1000.0f, 0.2f); // cap dt at 200ms
    m_lastTickMs = now;

    for (auto &b : m_blobs) {
        b.xPhase    += b.xFreq    * dt * m_speed;
        b.yPhase    += b.yFreq    * dt * m_speed;
        b.pulsePhase += b.pulseFreq * dt * m_speed;
    }
    update();
}

void AmbientBlobs::paintEvent(QPaintEvent *)
{
    // V32.32: external pause hook. When setActive(false) was called
    // (e.g. user navigated to camera page), paint nothing — leaving
    // a transparent overlay with no draw cost.
    if (!m_active) return;
    // Only paint blobs when an ambient/synthetic palette is active. On
    // static palettes the widget is essentially a no-op transparent layer.
    if (!Theme::isSyntheticActive()) return;
    if (m_blobs.isEmpty()) return;

    QPainter p(this);
    // V32.31: dropped Antialiasing — we're blitting cached pixmaps now,
    // not drawing geometry. SmoothPixmapTransform is enough for clean
    // scaled blits and is much cheaper than full AA gradient eval.
    p.setRenderHint(QPainter::SmoothPixmapTransform);
    p.setCompositionMode(QPainter::CompositionMode_SourceOver);

    const int w = width();
    const int h = height();
    const int side = qMin(w, h);
    if (w <= 0 || h <= 0) return;

    // Color sources — read each frame so palette changes (e.g. user picks
    // ambient_inbox a second time after seeing more snaps) take effect
    // without re-seeding the blobs.
    const QColor cols[4] = {
        QColor(QString::fromLatin1(Theme::ACCENT())),
        QColor(QString::fromLatin1(Theme::ACCENT_SOFT())),
        QColor(QString::fromLatin1(Theme::ACCENT_DIM())),
        QColor(QString::fromLatin1(Theme::GLOW_ONLINE())),
    };

    // V32.31: peak alpha — the alpha the cached pixmap is rendered at.
    // Per-frame opacity is modulated DOWN from this via setOpacity().
    constexpr float kPeakAlpha = 0.20f;

    for (auto &b : m_blobs) {
        const float cx = (0.5f + 0.45f * std::sin(b.xPhase)) * w;
        const float cy = (0.5f + 0.45f * std::sin(b.yPhase)) * h;

        const float radiusMod = 1.0f + 0.15f * std::sin(b.pulsePhase);
        const float radius = b.baseRadiusF * side * radiusMod;
        const float opacity = 0.06f + 0.06f * (1.0f + std::sin(b.pulsePhase * 0.75f));

        QColor c = cols[b.colorIdx];
        if (!c.isValid()) continue;

        // V32.31: cache rebuild check. If the color the cache was built
        // against has changed (synthetic palette mutated since last
        // paint), re-render the cache. Otherwise the blit is a fast
        // bilinear scale, not a full gradient eval.
        const QRgb wantedColor = c.rgb();
        if (b.cachedColor != wantedColor || b.cachedPx.isNull()
            || b.cachedPx.width() != kCacheSize) {
            QPixmap px(kCacheSize, kCacheSize);
            px.fill(Qt::transparent);
            QPainter cp(&px);
            cp.setRenderHint(QPainter::Antialiasing);
            cp.setPen(Qt::NoPen);
            const float r = kCacheSize / 2.0f;
            QRadialGradient g(r, r, r);
            QColor center = c;
            center.setAlphaF(kPeakAlpha);
            QColor edge = c;
            edge.setAlphaF(0.0f);
            g.setColorAt(0.0, center);
            g.setColorAt(1.0, edge);
            cp.setBrush(g);
            cp.drawEllipse(QPointF(r, r), r, r);
            cp.end();
            b.cachedPx = px;
            b.cachedColor = wantedColor;
        }

        // Modulate the cached peak-alpha pixmap down to the current
        // animated opacity. setOpacity multiplies the source alpha;
        // since the source was rendered at kPeakAlpha (= 0.20), final
        // alpha at center = kPeakAlpha * (opacity / kPeakAlpha) = opacity.
        const float scale = qBound(0.0f, opacity / kPeakAlpha, 1.0f);
        p.setOpacity(scale);

        // Target rect: 2r × 2r centered on (cx, cy). drawPixmap will
        // bilinear-scale the 256×256 cache to this size — cheap.
        const QRectF dst(cx - radius, cy - radius, 2.0f * radius, 2.0f * radius);
        p.drawPixmap(dst, b.cachedPx, QRectF(0, 0, kCacheSize, kCacheSize));
    }
    p.setOpacity(1.0); // restore
}

void AmbientBlobs::showEvent(QShowEvent *e)
{
    QWidget::showEvent(e);
    // V32.34+ battery: respect m_active and m_appActive. The widget
    // can be re-shown after a hide (page swipe-back) while ambient
    // is disabled or app is backgrounded; in those cases we keep
    // the timer stopped.
    if (m_timer && !m_timer->isActive() && m_active && m_appActive) {
        m_lastTickMs = 0;
        m_timer->start();
    }
}

void AmbientBlobs::hideEvent(QHideEvent *e)
{
    QWidget::hideEvent(e);
    if (m_timer) m_timer->stop();
}

void AmbientBlobs::resizeEvent(QResizeEvent *e)
{
    QWidget::resizeEvent(e);
    // No reseed needed — blob positions are stored as 0..1 ratios.
    update();
}

} // namespace SnapClone::UI
