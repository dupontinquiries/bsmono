#include "ui/camera/GlassIconButton.hpp"
#include "ui/Theme.hpp"

#include <QPainter>
#include <QPainterPath>
#include <QPaintEvent>
#include <QMouseEvent>
#include <QPropertyAnimation>
#include <QRadialGradient>
#include <QEasingCurve>

namespace SnapClone::UI::Camera {

GlassIconButton::GlassIconButton(const QString &glyph, QWidget *parent)
    : QPushButton(glyph, parent)
{
    setObjectName(QStringLiteral("iconBtn"));   // keep size/layout conventions
    setCursor(Qt::PointingHandCursor);
    setAttribute(Qt::WA_Hover, true);
    // We paint everything ourselves; no stylesheet background should fight it.
    // (Theme's QSS still sets size via #iconBtn; we override paint only.)

    m_scaleAnim = new QPropertyAnimation(this, "pressScale", this);
    m_scaleAnim->setDuration(Theme::ANIM_FAST);
    m_rippleAnim = new QPropertyAnimation(this, "ripple", this);
    m_rippleAnim->setDuration(Theme::ANIM_NORMAL + 120);
    m_rippleAnim->setEasingCurve(QEasingCurve::OutCubic);
}

void GlassIconButton::setPressScale(qreal s) { m_pressScale = s; update(); }
void GlassIconButton::setRipple(qreal r)     { m_ripple = r;     update(); }

void GlassIconButton::startPressAnim()
{
    m_scaleAnim->stop();
    m_scaleAnim->setStartValue(m_pressScale);
    m_scaleAnim->setEndValue(0.88);                 // dip
    m_scaleAnim->setEasingCurve(QEasingCurve::OutQuad);
    m_scaleAnim->start();
}

void GlassIconButton::startReleaseAnim()
{
    // Spring back with a slight overshoot for a tactile feel.
    m_scaleAnim->stop();
    m_scaleAnim->setStartValue(m_pressScale);
    m_scaleAnim->setEndValue(1.0);
    m_scaleAnim->setEasingCurve(QEasingCurve::OutBack);
    m_scaleAnim->start();

    // Ripple from the tap point.
    m_rippleAnim->stop();
    m_rippleAnim->setStartValue(0.0);
    m_rippleAnim->setEndValue(1.0);
    m_rippleAnim->start();
}

void GlassIconButton::mousePressEvent(QMouseEvent *e)
{
    m_rippleOrigin = e->position();
    startPressAnim();
    QPushButton::mousePressEvent(e);
}

void GlassIconButton::mouseReleaseEvent(QMouseEvent *e)
{
    startReleaseAnim();
    QPushButton::mouseReleaseEvent(e);
}

void GlassIconButton::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing, true);

    const QRectF full = rect();
    const QPointF c = full.center();
    // Apply the press scale about the center.
    p.translate(c);
    p.scale(m_pressScale, m_pressScale);
    p.translate(-c);

    // Circular body inset slightly for the 1px ring.
    const qreal d = qMin(full.width(), full.height());
    QRectF body(c.x() - d / 2.0 + 1, c.y() - d / 2.0 + 1, d - 2, d - 2);

    QPainterPath clip;
    clip.addEllipse(body);
    p.save();
    p.setClipPath(clip);

    // 1) Dark, more-opaque base (darker than the old rgba(36,34,32,0.88)).
    p.fillRect(body, QColor(12, 11, 10, 235));

    // 2) Soft radial mottling: two off-centre radial gradients with low-alpha
    //    light/dark lobes so the fill reads as uneven frosted glass rather than
    //    a flat disc. Kept subtle.
    {
        QRadialGradient g1(QPointF(body.left() + body.width() * 0.32,
                                   body.top()  + body.height() * 0.30),
                           body.width() * 0.75);
        g1.setColorAt(0.0, QColor(255, 255, 255, 26));   // faint highlight lobe
        g1.setColorAt(0.6, QColor(255, 255, 255, 8));
        g1.setColorAt(1.0, QColor(255, 255, 255, 0));
        p.fillRect(body, g1);

        QRadialGradient g2(QPointF(body.left() + body.width() * 0.72,
                                   body.top()  + body.height() * 0.74),
                           body.width() * 0.85);
        g2.setColorAt(0.0, QColor(0, 0, 0, 80));         // deepening lobe
        g2.setColorAt(0.7, QColor(0, 0, 0, 24));
        g2.setColorAt(1.0, QColor(0, 0, 0, 0));
        p.fillRect(body, g2);
    }

    // 3) Ripple from the tap point (expanding, fading ring+fill).
    if (m_ripple > 0.0 && m_ripple < 1.0) {
        const qreal maxR = d * 0.9;
        const qreal r = maxR * m_ripple;
        const int alpha = static_cast<int>(70 * (1.0 - m_ripple));
        QRadialGradient rg(m_rippleOrigin, r);
        rg.setColorAt(0.0, QColor(255, 255, 255, alpha));
        rg.setColorAt(0.7, QColor(255, 255, 255, alpha / 2));
        rg.setColorAt(1.0, QColor(255, 255, 255, 0));
        p.fillRect(body, rg);
    }
    p.restore();

    // 4) Thin ring for definition.
    QColor ring(255, 255, 255, 38);
    if (underMouse()) ring.setAlpha(70);
    p.setPen(QPen(ring, 1.0));
    p.setBrush(Qt::NoBrush);
    p.drawEllipse(body);

    // 5) The glyph (text()), centered, white.
    p.setPen(QColor(255, 255, 255, isEnabled() ? 235 : 110));
    QFont f = font();
    if (f.pixelSize() <= 0 && f.pointSize() <= 0) f.setPixelSize(22);
    p.setFont(f);
    p.drawText(body, Qt::AlignCenter, text());
}

} // namespace SnapClone::UI::Camera
