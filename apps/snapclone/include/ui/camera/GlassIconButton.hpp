#pragma once

#include <QPushButton>
#include <QPointF>

class QPropertyAnimation;

namespace SnapClone::UI::Camera {

/// Round icon button for the camera chrome: paints its own dark, mottled
/// frosted-glass fill (instead of a flat stylesheet color) and animates on
/// press with a quick scale-down + spring-back AND a ripple that expands from
/// the actual tap point.
///
/// Painted (not stylesheet) because the look needs things QSS can't express:
///   * soft radial mottling — two off-centre radial gradients over a dark base,
///     so the fill reads as uneven "scattered" glass rather than a flat disc;
///   * a ripple keyed to the press coordinate, which a stylesheet :pressed
///     state cannot know.
///
/// Drop-in for the old objectName("iconBtn") QPushButtons; still emits clicked()
/// and honors text() for the glyph.
class GlassIconButton : public QPushButton {
    Q_OBJECT
    // Animated scale of the painted body (1.0 = full, dips on press).
    Q_PROPERTY(qreal pressScale READ pressScale WRITE setPressScale)
    // Ripple progress 0..1 (0 = none, 1 = fully expanded + faded).
    Q_PROPERTY(qreal ripple READ ripple WRITE setRipple)
public:
    explicit GlassIconButton(const QString &glyph, QWidget *parent = nullptr);

    qreal pressScale() const { return m_pressScale; }
    void  setPressScale(qreal s);
    qreal ripple() const { return m_ripple; }
    void  setRipple(qreal r);

protected:
    void paintEvent(QPaintEvent *) override;
    void mousePressEvent(QMouseEvent *) override;
    void mouseReleaseEvent(QMouseEvent *) override;

private:
    void startPressAnim();   // scale down
    void startReleaseAnim(); // spring back + ripple from m_rippleOrigin

    qreal   m_pressScale = 1.0;
    qreal   m_ripple     = 0.0;
    QPointF m_rippleOrigin;          // tap point in widget coords
    QPropertyAnimation *m_scaleAnim  = nullptr;
    QPropertyAnimation *m_rippleAnim = nullptr;
};

} // namespace SnapClone::UI::Camera
