#include "ui/TouchFeedback.hpp"
#include "ui/Theme.hpp"
#include <QWidget>
#include <QLabel>
#include <QPainter>
#include <QPropertyAnimation>
#include <QGraphicsOpacityEffect>
#include <QGraphicsScale>
#include <QGraphicsEffect>
#include <QAbstractAnimation>
#include <QTimer>
#include <QEvent>
#include <QMouseEvent>
#include <QTouchEvent>
#include <QRect>
#include <QColor>

namespace SnapClone::UI {

// ─── RippleOverlay ────────────────────────────────────────────────────────────
// A transparent overlay that draws an expanding circle and fades out.
// Created on demand, owns itself via deleteLater.

class RippleOverlay : public QWidget
{
    Q_OBJECT
    Q_PROPERTY(qreal radius READ radius WRITE setRadius)
    Q_PROPERTY(qreal opacity READ rippleOpacity WRITE setRippleOpacity)
public:
    explicit RippleOverlay(QWidget *parent, const QPoint &centre)
        : QWidget(parent)
        , m_centre(centre)
        , m_color(QColor(QLatin1String(Theme::ACCENT())))
    {
        setAttribute(Qt::WA_TransparentForMouseEvents);
        setAttribute(Qt::WA_TranslucentBackground);
        setGeometry(parent->rect());
        m_color.setAlpha(60);
        raise();
        show();

        const int maxR = static_cast<int>(
            qSqrt(width() * width() + height() * height())) + 20;

        // Expand radius
        auto *expand = new QPropertyAnimation(this, "radius", this);
        expand->setDuration(380);
        expand->setStartValue(0.0);
        expand->setEndValue(static_cast<qreal>(maxR));
        expand->setEasingCurve(QEasingCurve::OutCubic);

        // Fade opacity from 1 → 0 starting at 60% of the animation
        auto *fade = new QPropertyAnimation(this, "opacity", this);
        fade->setDuration(380);
        fade->setStartValue(1.0);
        fade->setKeyValueAt(0.6, 0.9);
        fade->setEndValue(0.0);
        fade->setEasingCurve(QEasingCurve::InCubic);

        connect(expand, &QPropertyAnimation::finished, this, &QWidget::deleteLater);
        expand->start(QAbstractAnimation::DeleteWhenStopped);
        fade->start(QAbstractAnimation::DeleteWhenStopped);
    }

    qreal radius() const { return m_radius; }
    void setRadius(qreal r) { m_radius = r; update(); }
    qreal rippleOpacity() const { return m_opacity; }
    void setRippleOpacity(qreal o) { m_opacity = o; update(); }

protected:
    void paintEvent(QPaintEvent *) override {
        QPainter p(this);
        p.setRenderHint(QPainter::Antialiasing);
        QColor c = m_color;
        c.setAlphaF(static_cast<float>(c.alphaF() * m_opacity));
        p.setBrush(c);
        p.setPen(Qt::NoPen);
        p.drawEllipse(m_centre, static_cast<int>(m_radius),
                                static_cast<int>(m_radius));
    }

private:
    QPoint  m_centre;
    QColor  m_color;
    qreal   m_radius  = 0.0;
    qreal   m_opacity = 1.0;
};

// ─── PressEventFilter ─────────────────────────────────────────────────────────
// Installed on widgets that want persistent press feedback.

class PressEventFilter : public QObject
{
    Q_OBJECT
public:
    PressEventFilter(QWidget *target, TouchFeedback::Effect effect)
        : QObject(target), m_target(target), m_effect(effect) {}

protected:
    bool eventFilter(QObject *, QEvent *e) override {
        if (e->type() == QEvent::MouseButtonPress) {
            const auto *me = static_cast<QMouseEvent *>(e);
            dispatch(me->pos());
        } else if (e->type() == QEvent::TouchBegin) {
            const auto *te = static_cast<QTouchEvent *>(e);
            if (!te->points().isEmpty())
                dispatch(te->points().first().position().toPoint());
        }
        return false; // never consume
    }

private:
    void dispatch(const QPoint &pos) {
        switch (m_effect) {
        case TouchFeedback::Effect::Ripple:  TouchFeedback::ripple(m_target, pos); break;
        case TouchFeedback::Effect::Pulse:   TouchFeedback::pulse(m_target);        break;
        case TouchFeedback::Effect::Spring:  TouchFeedback::spring(m_target);       break;
        case TouchFeedback::Effect::Shimmer: TouchFeedback::shimmer(m_target);      break;
        }
    }
    QWidget                *m_target;
    TouchFeedback::Effect   m_effect;
};

// ─── TouchFeedback ────────────────────────────────────────────────────────────

void TouchFeedback::ripple(QWidget *w, const QPoint &pressPos)
{
    if (!w) return;
    const QPoint centre = pressPos.isNull()
        ? w->rect().center()
        : pressPos;
    new RippleOverlay(w, centre); // self-managing
}

void TouchFeedback::pulse(QWidget *w)
{
    if (!w) return;
    // Qt doesn't have a built-in scale effect, but we can fake a subtle pulse
    // by briefly adjusting the opacity: 1.0 → 0.7 → 1.0 over 200ms.
    // A true scale would require QGraphicsView; this is the widget-native approach.
    auto *fx = qobject_cast<QGraphicsOpacityEffect *>(w->graphicsEffect());
    bool ownsFx = false;
    if (!fx) {
        fx = new QGraphicsOpacityEffect(w);
        w->setGraphicsEffect(fx);
        ownsFx = true;
    }
    auto *anim = new QPropertyAnimation(fx, "opacity", w);
    anim->setDuration(220);
    anim->setStartValue(1.0);
    anim->setKeyValueAt(0.35, 0.72);
    anim->setEndValue(1.0);
    anim->setEasingCurve(QEasingCurve::OutBack);
    if (ownsFx) {
        QObject::connect(anim, &QPropertyAnimation::finished, w, [w]() {
            w->setGraphicsEffect(nullptr);
        });
    }
    anim->start(QAbstractAnimation::DeleteWhenStopped);
}

void TouchFeedback::spring(QWidget *w)
{
    if (!w) return;
    // Spring: opacity dips then snaps back with OutBack overshoot feel
    auto *fx = new QGraphicsOpacityEffect(w);
    w->setGraphicsEffect(fx);
    auto *anim = new QPropertyAnimation(fx, "opacity", w);
    anim->setDuration(280);
    anim->setStartValue(1.0);
    anim->setKeyValueAt(0.25, 0.65);
    anim->setKeyValueAt(0.60, 1.05); // slight "over" before settling (OutBack feel)
    anim->setEndValue(1.0);
    anim->setEasingCurve(QEasingCurve::OutBack);
    QObject::connect(anim, &QPropertyAnimation::finished, w, [w]() {
        w->setGraphicsEffect(nullptr);
    });
    anim->start(QAbstractAnimation::DeleteWhenStopped);
}

void TouchFeedback::shimmer(QWidget *w)
{
    if (!w) return;
    // Shimmer: a narrow bright strip sweeps left→right over the widget.
    // Implemented as a thin QWidget that moves across the parent.
    auto *strip = new QWidget(w);
    strip->setGeometry(-24, 0, 24, w->height());
    strip->setAttribute(Qt::WA_TransparentForMouseEvents);
    strip->setStyleSheet(QStringLiteral(
        "background: qlineargradient(x1:0,y1:0,x2:1,y2:0,"
        "stop:0 rgba(255,255,255,0),"
        "stop:0.4 rgba(255,255,255,40),"
        "stop:0.6 rgba(255,255,255,80),"
        "stop:1 rgba(255,255,255,0));"));
    strip->raise();
    strip->show();

    auto *anim = new QPropertyAnimation(strip, "pos", strip);
    anim->setDuration(600);
    anim->setStartValue(QPoint(-24, 0));
    anim->setEndValue(QPoint(w->width(), 0));
    anim->setEasingCurve(QEasingCurve::InOutCubic);
    QObject::connect(anim, &QPropertyAnimation::finished, strip, &QWidget::deleteLater);
    anim->start(QAbstractAnimation::DeleteWhenStopped);
}

void TouchFeedback::install(QWidget *w, Effect effect)
{
    if (!w) return;
    w->setAttribute(Qt::WA_AcceptTouchEvents);
    w->installEventFilter(new PressEventFilter(w, effect));
}

} // namespace SnapClone::UI

#include "TouchFeedback.moc"
