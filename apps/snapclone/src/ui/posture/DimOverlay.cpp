#include "ui/posture/DimOverlay.hpp"

#include <QPropertyAnimation>
#include <QPainter>
#include <QEvent>
#include <QResizeEvent>

namespace SnapClone::UI {

namespace { constexpr qreal kSwallowAbove = 0.01; } // any dim ⇒ covered ⇒ swallow

DimOverlay::DimOverlay(QWidget *parent) : QWidget(parent)
{
    setAttribute(Qt::WA_NoSystemBackground);
    setAttribute(Qt::WA_TransparentForMouseEvents, true); // clear at rest
    setAttribute(Qt::WA_StyledBackground, false);
    setFocusPolicy(Qt::NoFocus);
    hide();
    if (parent) {
        parent->installEventFilter(this);   // track resize to stay full-bleed
        syncToParent();
    }
}

void DimOverlay::syncToParent()
{
    if (auto *p = parentWidget()) {
        setGeometry(p->rect());
        raise();
    }
}

void DimOverlay::setDimOpacity(qreal o)
{
    o = qBound<qreal>(0.0, o, 1.0);
    if (qFuzzyCompare(m_opacity, o)) return;
    m_opacity = o;
    if (m_opacity > 0.0 && !isVisible()) { syncToParent(); show(); }
    refreshInteractivity();
    update();
    if (m_opacity <= 0.0 && isVisible()) hide();
}

void DimOverlay::animateTo(qreal opacity, int durationMs)
{
    opacity = qBound<qreal>(0.0, opacity, 1.0);
    if (opacity > 0.0 && !isVisible()) { syncToParent(); show(); }
    if (durationMs <= 0) { setDimOpacity(opacity); return; }
    if (!m_anim) {
        m_anim = new QPropertyAnimation(this, "dimOpacity", this);
        m_anim->setEasingCurve(QEasingCurve::InOutQuad);
    }
    m_anim->stop();
    m_anim->setStartValue(m_opacity);
    m_anim->setEndValue(opacity);
    m_anim->setDuration(durationMs);
    m_anim->start();
}

void DimOverlay::refreshInteractivity()
{
    // Swallow touches whenever any dim is showing (the phone is covered); stay
    // click-through when clear.
    setAttribute(Qt::WA_TransparentForMouseEvents, m_opacity <= kSwallowAbove);
}

void DimOverlay::paintEvent(QPaintEvent *)
{
    if (m_opacity <= 0.0) return;
    QPainter p(this);
    p.fillRect(rect(), QColor(0, 0, 0, int(m_opacity * 255.0 + 0.5)));
}

bool DimOverlay::eventFilter(QObject *watched, QEvent *event)
{
    if (watched == parentWidget()
        && (event->type() == QEvent::Resize || event->type() == QEvent::Move
            || event->type() == QEvent::Show)) {
        syncToParent();
    }
    return QWidget::eventFilter(watched, event);
}

} // namespace SnapClone::UI
