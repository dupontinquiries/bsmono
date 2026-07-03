#pragma once

#include <QWidget>
#include <QImage>

namespace SnapClone::UI::Camera {

/// A frosted-glass scrim for the full-bleed camera bars.
///
/// It does NOT sample the screen behind it (QWidget can't read sibling pixels
/// cheaply, and QGraphicsEffect won't blur a live sibling). Instead the owner
/// (CameraWidget) feeds it the strip of the current viewfinder pixmap that lies
/// under this scrim's geometry — those pixels are already in hand each frame
/// (the scaled viewfinder QImage), so there is zero extra capture cost.
///
/// On each new strip we build a cached "glass" pixmap ONCE per frame:
///   1. downscale the strip (the downscale IS most of the blur, and makes every
///      later step run on ~1/16th the pixels — this is what keeps it cheap
///      enough for the Fairphone 5),
///   2. a small separable box blur on the tiny image (frost),
///   3. composite three copies offset by +dx / 0 / -dx pixels, taking R from
///      one, G from the centre, B from the other → subtle chromatic shift,
///   4. upscale back (bilinear = extra softening for free),
///   5. paint it, then lay the edge→transparent gradient + a faint tint on top
///      so controls stay legible and the panel reads as glass.
///
/// Mouse-transparent: set WA_TransparentForMouseEvents on the instance so taps
/// fall through to the viewfinder; the interactive control bar sits ABOVE it.
class GlassScrim : public QWidget {
    Q_OBJECT
public:
    enum class Edge { Top, Bottom };

    explicit GlassScrim(Edge edge, QWidget *parent = nullptr);

    /// Feed the viewfinder strip under this scrim (in viewfinder/scrim-local
    /// coordinates, i.e. already cropped to this widget's geometry). Triggers a
    /// rebuild of the cached glass pixmap and a repaint. Cheap to call ~30fps.
    void setBackdrop(const QImage &strip);

    /// Subtle by default. shiftPx is the per-channel offset in FINAL (upscaled)
    /// pixels; 1 reads as a faint fringe, 2-3 as noticeable. Clamped >= 0.
    void setChromaticShift(int shiftPx);

protected:
    void paintEvent(QPaintEvent *) override;
    void resizeEvent(QResizeEvent *) override;

private:
    void rebuildGlass();          // recompute m_glass from m_backdrop

    Edge    m_edge;
    int     m_shiftPx   = 1;      // start subtle
    int     m_downscale = 4;      // strip processed at 1/downscale resolution
    QImage  m_backdrop;           // latest strip (viewfinder-resolution)
    QImage  m_glass;              // cached processed pixmap at widget size
};

} // namespace SnapClone::UI::Camera
