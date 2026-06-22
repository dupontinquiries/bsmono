#include "ui/camera/FocusReticle.hpp"

#include <QPainter>
#include <QPen>
#include <QTimer>
#include <QPropertyAnimation>
#include <QGraphicsOpacityEffect>
#include <QtMath>

namespace SnapClone::UI::Camera {

FocusReticle::FocusReticle(QWidget *parent)
    : QWidget(parent)
{
    setAttribute(Qt::WA_TransparentForMouseEvents, true);
    setAttribute(Qt::WA_NoSystemBackground, true);
    setAttribute(Qt::WA_TranslucentBackground, true);
    setFixedSize(kSize, kSize);
    hide();

    // Pulse timer — 60fps. Pure cosmetic, only runs while in Searching state.
    // Updates m_pulsePhase 0..1 over 800ms cycle. cos() in paintEvent renders
    // a "breath" between 0.85 and 1.0 scale to communicate "working".
    m_pulseTimer = new QTimer(this);
    m_pulseTimer->setInterval(16);
    connect(m_pulseTimer, &QTimer::timeout, this, [this]() {
        // 800ms cycle: increment 0..1 over 50 ticks (16ms × 50 = 800ms).
        m_pulsePhase += 0.02;
        if (m_pulsePhase > 1.0) m_pulsePhase -= 1.0;
        update();
    });

    // Linger timer — used by setState() to hold a result (Locked/Failed)
    // for ~1.2s before fading the reticle out. Single-shot.
    m_lingerTimer = new QTimer(this);
    m_lingerTimer->setSingleShot(true);
    m_lingerTimer->setInterval(1200);
    connect(m_lingerTimer, &QTimer::timeout, this, &FocusReticle::beginLingerThenFade);

    // Fade-out animation, applied via a QGraphicsOpacityEffect. Built lazily
    // on first use to avoid the perf hit of attaching an effect to every
    // tap (effects are cheap but not free; we only need it during fade).
}

FocusReticle::~FocusReticle() = default;

void FocusReticle::triggerAt(const QPointF &center)
{
    // Position so the supplied point is the reticle's middle.
    const int x = static_cast<int>(center.x()) - kSize / 2;
    const int y = static_cast<int>(center.y()) - kSize / 2;
    move(x, y);

    // Reset any in-flight fade so the new tap appears at full opacity.
    if (m_fadeAnim) {
        m_fadeAnim->stop();
        m_fadeAnim->deleteLater();
        m_fadeAnim = nullptr;
    }
    if (auto *fx = qobject_cast<QGraphicsOpacityEffect *>(graphicsEffect())) {
        fx->setOpacity(1.0);
    } else if (graphicsEffect()) {
        // Some other effect was installed — clear it so opacity 1.0 reads
        // straight off the widget surface.
        setGraphicsEffect(nullptr);
    }
    if (m_lingerTimer && m_lingerTimer->isActive()) m_lingerTimer->stop();

    m_state = State::Searching;
    m_pulsePhase = 0.0;
    show();
    raise();
    startPulse();
    update();
}

void FocusReticle::setState(State s)
{
    if (!isVisible()) return;
    if (m_state == s) return;
    m_state = s;
    if (s == State::Searching) {
        startPulse();
    } else {
        stopPulse();
        // Linger then fade. The linger timer fires beginLingerThenFade
        // which kicks off a 400ms opacity animation to 0.
        if (m_lingerTimer) m_lingerTimer->start();
    }
    update();
}

void FocusReticle::startPulse()
{
    if (m_pulseTimer && !m_pulseTimer->isActive()) m_pulseTimer->start();
}

void FocusReticle::stopPulse()
{
    if (m_pulseTimer) m_pulseTimer->stop();
    m_pulsePhase = 0.0;  // settle to "no breath" so the steady state is crisp
}

void FocusReticle::beginLingerThenFade()
{
    // Apply opacity effect and animate 1.0 → 0.0. On finished, hide() so
    // we're not consuming paint cycles for an invisible widget.
    auto *fx = qobject_cast<QGraphicsOpacityEffect *>(graphicsEffect());
    if (!fx) {
        fx = new QGraphicsOpacityEffect(this);
        fx->setOpacity(1.0);
        setGraphicsEffect(fx);
    }
    if (m_fadeAnim) {
        m_fadeAnim->stop();
        m_fadeAnim->deleteLater();
    }
    m_fadeAnim = new QPropertyAnimation(fx, "opacity", this);
    m_fadeAnim->setDuration(400);
    m_fadeAnim->setStartValue(1.0);
    m_fadeAnim->setEndValue(0.0);
    connect(m_fadeAnim, &QPropertyAnimation::finished, this, [this]() {
        hide();
    });
    m_fadeAnim->start();
}

void FocusReticle::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing);

    // Color & visual style depend on state.
    QColor color;
    switch (m_state) {
    case State::Searching: color = QColor(120, 220, 255); break;  // cyan
    case State::Locked:    color = QColor(120, 255, 140); break;  // green
    case State::Failed:    color = QColor(255, 120,  90); break;  // red-orange
    }

    // Pulse scale: 0.85..1.0 breathing on Searching, fixed 1.0 otherwise.
    // The cos()-based curve gives a smooth in/out without a hard reverse
    // at the peak — feels more "alive" than a linear lerp.
    qreal scale = 1.0;
    if (m_state == State::Searching) {
        const qreal breath = 0.5 - 0.5 * qCos(m_pulsePhase * 2.0 * M_PI);
        scale = 0.85 + 0.15 * breath;
    }

    const qreal inset = kSize * (1.0 - scale) * 0.5;
    const QRectF box(inset, inset, kSize - inset * 2.0, kSize - inset * 2.0);

    // Draw four L-shaped corner brackets rather than a full square; this
    // is the classic camera AF reticle visual. Each bracket is a quarter
    // of the box edge length.
    QPen pen(color, 2.5);
    pen.setCapStyle(Qt::RoundCap);
    p.setPen(pen);

    const qreal armLen = box.width() * 0.25;
    const qreal x0 = box.left(),  y0 = box.top();
    const qreal x1 = box.right(), y1 = box.bottom();

    // Top-left
    p.drawLine(QPointF(x0, y0 + armLen), QPointF(x0, y0));
    p.drawLine(QPointF(x0, y0), QPointF(x0 + armLen, y0));
    // Top-right
    p.drawLine(QPointF(x1 - armLen, y0), QPointF(x1, y0));
    p.drawLine(QPointF(x1, y0), QPointF(x1, y0 + armLen));
    // Bottom-left
    p.drawLine(QPointF(x0, y1 - armLen), QPointF(x0, y1));
    p.drawLine(QPointF(x0, y1), QPointF(x0 + armLen, y1));
    // Bottom-right
    p.drawLine(QPointF(x1 - armLen, y1), QPointF(x1, y1));
    p.drawLine(QPointF(x1, y1), QPointF(x1, y1 - armLen));

    // Center dot for the Locked state — small visual "yes, here" marker.
    if (m_state == State::Locked) {
        p.setBrush(color);
        p.setPen(Qt::NoPen);
        const QPointF c(kSize * 0.5, kSize * 0.5);
        p.drawEllipse(c, 3.5, 3.5);
    }
}

} // namespace SnapClone::UI::Camera
