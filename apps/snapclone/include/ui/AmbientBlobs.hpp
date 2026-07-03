#pragma once
// ─── UI/AmbientBlobs.hpp ─────────────────────────────────────────────────────
//
// V32.30: animated ambient color splotches for large UI panes.
//
// Renders 2-4 large soft-edged radial gradient "blobs" in the active palette's
// accent colors, slowly drifting around and pulsing in size + opacity. The
// goal is to keep the canvas alive when ambient theming is in use — without
// the splotches, a calmly-tinted background looks dead; with them, the UI
// has a sense of breath. Looks great behind inbox lists, settings sections,
// wayfinder canvas, etc.
//
// Cheap by design:
//   • Updates at 30fps via a single QTimer
//   • Each blob is a single QRadialGradient fill (GPU-accelerated by Qt)
//   • Sleeps the timer when not visible (showEvent / hideEvent)
//   • Bypasses all painting when no synthetic palette is active — base
//     palettes render to a clean BG with no overlay cost
//
// Lifecycle:
//   • Constructed as a child of the host page widget
//   • The host MUST call AmbientBlobs::stackBehind(otherWidget) or use it
//     as a custom background. The blobs widget doesn't intercept input —
//     it sets WA_TransparentForMouseEvents so all clicks pass through.
//
// Threading: pure UI; everything runs on the GUI thread.
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QList>
#include <QPointF>
#include <QPixmap>

QT_BEGIN_NAMESPACE
class QTimer;
class QPaintEvent;
class QShowEvent;
class QHideEvent;
class QResizeEvent;
QT_END_NAMESPACE

namespace SnapClone::UI {

class AmbientBlobs : public QWidget {
    Q_OBJECT
public:
    explicit AmbientBlobs(QWidget *parent = nullptr);
    ~AmbientBlobs() override;

    /// How many blobs to render. Default 3. Range 1..5. More than 5 looks
    /// chaotic; fewer than 2 doesn't read as "ambient."
    void setBlobCount(int n);

    /// Speed multiplier — 1.0 = normal, 0.5 = half speed, 2.0 = twice as fast.
    /// Default 1.0. Mostly used to slow blobs down on the settings page where
    /// the user is focused on text.
    void setSpeed(float multiplier);

    /// V32.32: external pause control. When false, the animation timer
    /// stops and paintEvent renders nothing. Used by MainWindow to pause
    /// blobs while the camera page is visible — the viewfinder dominates
    /// the screen anyway, and stopping the 10fps repaint of a window-sized
    /// translucent layer is a measurable CPU win during camera+ambient
    /// load. The widget's visibility itself is not changed (it stays a
    /// child of the wrapper), only its animation tick.
    void setActive(bool active);

protected:
    void paintEvent(QPaintEvent *e) override;
    void showEvent(QShowEvent *e) override;
    void hideEvent(QHideEvent *e) override;
    void resizeEvent(QResizeEvent *e) override;

private:
    struct Blob {
        /// Position parameters expressed as 0..1 ratios of widget size, so
        /// the blob naturally repositions on resize. Drift uses a slow
        /// Lissajous figure (different periods on x and y) so paths don't
        /// repeat exactly — gives a sense of wandering rather than orbiting.
        float xPhase;     // 0..2π — current phase along x sine
        float yPhase;     // 0..2π — current phase along y sine
        float xFreq;      // radians per second along x (slow)
        float yFreq;      // radians per second along y (slow)
        float pulsePhase; // 0..2π — current phase for radius/opacity pulse
        float pulseFreq;  // radians per second for pulse
        /// Color index — 0 = ACCENT, 1 = ACCENT_SOFT, 2 = ACCENT_DIM,
        /// 3 = GLOW_ONLINE. The blob refreshes its color choice on each
        /// pulse cycle (so a vermillion accent + cool soft accent slowly
        /// cycle which dominates).
        int   colorIdx;
        /// Base radius as fraction of min(width, height). Each blob has
        /// a different base so they don't all pulse to the same size.
        float baseRadiusF;

        /// V32.31: cached gradient pixmap. Radial gradients in Qt are
        /// software-rasterized pixel-by-pixel — at 30fps × 3 blobs ×
        /// full-screen size = the framerate killer. We pre-render each
        /// blob's gradient to a fixed-size QPixmap at peak alpha and
        /// blit it scaled+alpha-modulated each frame. Cache invalidates
        /// only on color change (synthetic palette repaints).
        ///
        /// The cache stores at a normalized 256×256 reference size; the
        /// paint scales it to (2*currentRadius)². setOpacity() in the
        /// painter modulates the visible alpha against the cached
        /// peak-alpha pixmap.
        QPixmap cachedPx;
        QRgb    cachedColor = 0; // QRgb 0 == invalid sentinel
    };
    static constexpr int kCacheSize = 256;

    void seedBlobs();
    void onTick();

    QTimer *m_timer = nullptr;
    QList<Blob> m_blobs;
    int   m_blobCount = 3;
    float m_speed     = 1.0f;
    qint64 m_lastTickMs = 0;
    /// V32.32: external pause flag (see setActive). When false,
    /// paintEvent renders nothing AND the timer is stopped.
    bool   m_active   = true;
    /// V32.34+: app-state flag (Qt::ApplicationActive vs anything else).
    /// When the app is backgrounded (phone in pocket, user on another
    /// app), keep the timer stopped — no point animating pixels nobody
    /// can see. AmbientBlobs subscribes to QGuiApplication's signal in
    /// its ctor; the flag's initial value reflects the state at
    /// construction time so a backgrounded-at-startup launch behaves
    /// correctly.
    bool   m_appActive = true;
};

} // namespace SnapClone::UI
