#include "ui/PagePeelHint.hpp"
#include "ui/Theme.hpp"
#include "ui/UxGuide.hpp"

#include <QPainter>
#include <QPainterPath>
#include <QGraphicsOpacityEffect>
#include <QAbstractAnimation>

namespace SnapClone::UI {

PagePeelHint::PagePeelHint(QWidget *parent) : QWidget(parent)
{
    setAttribute(Qt::WA_TransparentForMouseEvents);
    setAttribute(Qt::WA_TranslucentBackground);
    setAttribute(Qt::WA_NoSystemBackground);
    setGeometry(parent->rect());
    raise();
    show();
    startPulse();

    m_idleTimer = new QTimer(this);
    m_idleTimer->setSingleShot(true);
    m_idleTimer->setInterval(kIdleReturnMs);
    connect(m_idleTimer, &QTimer::timeout, this, [this]() {
        if (m_hidden) reshowPeels();
    });
}

void PagePeelHint::setPageState(int current, int total)
{
    m_showLeft  = (current > 0);
    m_showRight = (current < total - 1);
    if (m_anim && m_anim->state() == QAbstractAnimation::Running)
        hidePeelsBriefly();
    update();
}

void PagePeelHint::dismissIfFamiliar()
{
    const int uses = UxGuide::instance()
        .familiarityFor(QLatin1String(Feature::kSwipe)).useCount;
    if (uses >= kMinSwipesBeforeHide) {
        m_anim->stop();
        auto *fx   = new QGraphicsOpacityEffect(this);
        setGraphicsEffect(fx);
        auto *fade = new QPropertyAnimation(fx, "opacity", this);
        fade->setDuration(600);
        fade->setStartValue(1.0);
        fade->setEndValue(0.0);
        connect(fade, &QPropertyAnimation::finished, this, &QWidget::deleteLater);
        fade->start(QAbstractAnimation::DeleteWhenStopped);
    }
}

void PagePeelHint::startPulse()
{
    if (!m_anim) {
        m_anim = new QPropertyAnimation(this, "peelOpacity", this);
        m_anim->setLoopCount(-1);
    }
    m_anim->stop();
    m_anim->setDuration(3200);
    m_anim->setStartValue(0.0);
    m_anim->setKeyValueAt(0.45, 0.42);
    m_anim->setKeyValueAt(0.55, 0.42);
    m_anim->setEndValue(0.0);
    m_anim->setEasingCurve(QEasingCurve::SineCurve);
    m_hidden = false;
    m_anim->start();
}

void PagePeelHint::hidePeelsBriefly()
{
    if (m_hidden) return;
    m_hidden = true;
    m_anim->stop();
    m_op = 0.0;
    update();
    m_idleTimer->start(kIdleReturnMs);
}

void PagePeelHint::reshowPeels()
{
    m_hidden = false;
    auto *fx = new QGraphicsOpacityEffect(this);
    setGraphicsEffect(fx);
    fx->setOpacity(0.0);
    auto *fadeIn = new QPropertyAnimation(fx, "opacity", this);
    fadeIn->setDuration(800);
    fadeIn->setStartValue(0.0);
    fadeIn->setEndValue(1.0);
    fadeIn->setEasingCurve(QEasingCurve::OutCubic);
    connect(fadeIn, &QPropertyAnimation::finished, this, [this]() {
        setGraphicsEffect(nullptr);
        startPulse();
    });
    fadeIn->start(QAbstractAnimation::DeleteWhenStopped);
}

void PagePeelHint::paintEvent(QPaintEvent *)
{
    if (m_hidden || m_op <= 0.005) return;

    const int uses  = UxGuide::instance()
        .familiarityFor(QLatin1String(Feature::kSwipe)).useCount;
    const qreal scale = qMax(0.0, 1.0 - static_cast<double>(uses) / kMinSwipesBeforeHide);
    if (scale <= 0.0) return;

    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing);

    QColor peelColor(QLatin1String(Theme::TEXT_SECONDARY()));
    peelColor.setAlphaF(static_cast<float>(m_op * scale * 0.55));

    const int h     = height();
    const int midY  = h / 2;
    const int peelH = qMin(72, h / 3);
    const int peelW = 10;
    const int r     = peelH / 2;

    auto drawPeel = [&](bool leftSide) {
        const int rx  = leftSide ? 0 : width();
        const int dir = leftSide ? 1 : -1;

        QPainterPath path;
        if (leftSide) {
            path.moveTo(0, midY - r);
            path.lineTo(0, midY + r);
            path.quadTo(peelW * 1.4, midY + r, peelW * 1.4, midY);
            path.quadTo(peelW * 1.4, midY - r, 0, midY - r);
        } else {
            path.moveTo(rx, midY - r);
            path.lineTo(rx, midY + r);
            path.quadTo(rx - peelW * 1.4, midY + r, rx - peelW * 1.4, midY);
            path.quadTo(rx - peelW * 1.4, midY - r, rx, midY - r);
        }
        path.closeSubpath();

        p.setPen(Qt::NoPen);
        p.setBrush(peelColor);
        p.drawPath(path);

        QColor arrowColor = peelColor;
        arrowColor.setAlphaF(static_cast<float>(m_op * scale * 0.75));
        p.setPen(QPen(arrowColor, 1.5, Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin));
        p.setBrush(Qt::NoBrush);

        const int tipX   = leftSide ? (peelW - 3) : (rx - peelW + 3);
        const int arrowH = peelH / 6;
        QPainterPath arrow;
        arrow.moveTo(tipX + dir * 2, midY - arrowH);
        arrow.lineTo(tipX - dir * 2, midY);
        arrow.lineTo(tipX + dir * 2, midY + arrowH);
        p.drawPath(arrow);
    };

    if (m_showLeft)  drawPeel(true);
    if (m_showRight) drawPeel(false);
}

} // namespace SnapClone::UI
