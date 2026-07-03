// ContactCard.cpp — V32.34+  (see ContactCard.hpp for the design overview)

#include "ui/ContactCard.hpp"
#include "ui/Theme.hpp"
#include "ui/SvgIcon.hpp"

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QPushButton>
#include <QPropertyAnimation>
#include <QMouseEvent>
#include <QEvent>
#include <QPainter>
#include <QPaintEvent>

namespace SnapClone::UI {

// How far a horizontal drag must travel on an expanded card before it counts
// as a "swipe out" (collapse) rather than a tap. Kept above the OS tap slop so
// a normal tap never collapses, but low enough that a deliberate flick does.
static constexpr int kSwipeCloseThresholdPx = 48;
// A press that moves less than this in any direction is treated as a TAP.
static constexpr int kTapSlopPx = 10;

ContactCard::ContactCard(const QString &contactId, QWidget *parent)
    : QWidget(parent), m_contactId(contactId)
{
    m_outer = new QVBoxLayout(this);
    m_outer->setContentsMargins(0, 0, 0, 0);
    m_outer->setSpacing(0);

    // Submenu container — height-animated, clipped, hidden when collapsed.
    m_submenu = new QWidget(this);
    m_submenu->setObjectName(QStringLiteral("contactCardSubmenu"));
    m_submenu->setStyleSheet(QStringLiteral(
        "#contactCardSubmenu { background:%1; border:none;"
        "  border-bottom-left-radius:%2px; border-bottom-right-radius:%2px; }")
        .arg(QLatin1String(Theme::BG_RAISED())).arg(Theme::RADIUS_MD));
    m_submenuLayout = new QVBoxLayout(m_submenu);
    m_submenuLayout->setContentsMargins(Theme::SPACE_SM, Theme::SPACE_XS,
                                        Theme::SPACE_SM, Theme::SPACE_SM);
    m_submenuLayout->setSpacing(Theme::SPACE_XS);
    m_submenu->setMaximumHeight(0);          // start collapsed
    m_submenu->setVisible(false);

    m_anim = new QPropertyAnimation(this, "submenuHeight", this);
    m_anim->setDuration(180);
    m_anim->setEasingCurve(QEasingCurve::OutCubic);
}

void ContactCard::setHeader(QWidget *header)
{
    m_header = header;
    if (!m_header) return;
    m_header->setParent(this);
    // Header is inserted FIRST, submenu below it.
    m_outer->insertWidget(0, m_header);
    if (m_outer->indexOf(m_submenu) < 0)
        m_outer->addWidget(m_submenu);
    // Watch the header for tap/swipe. The header's own child buttons (edit,
    // share-loc) keep working — they consume their own clicks before this
    // filter sees a release as a tap.
    m_header->installEventFilter(this);
}

void ContactCard::addAction(const QString &label, std::function<void()> onClick,
                            const QString &iconName, bool danger)
{
    auto *btn = new QPushButton(label, m_submenu);
    btn->setCursor(Qt::PointingHandCursor);
    btn->setFixedHeight(Theme::TOUCH_MIN);
    if (!iconName.isEmpty()) {
        btn->setIcon(SvgIcon::secondary(iconName, QSize(18, 18)));
        btn->setIconSize(QSize(18, 18));
    }
    // Left-aligned text, full-width row, subtle hover. Danger = red text.
    const char *fg = danger ? Theme::DANGER() : Theme::TEXT_PRIMARY();
    btn->setStyleSheet(QStringLiteral(
        "QPushButton { text-align:left; padding:0 %1px; background:transparent;"
        "  border:none; border-radius:%2px; color:%3; font-size:%4px; }"
        "QPushButton:hover { background:%5; }"
        "QPushButton:pressed { background:%6; }")
        .arg(Theme::SPACE_SM).arg(Theme::RADIUS_SM)
        .arg(QLatin1String(fg)).arg(Theme::FONT_BASE)
        .arg(QLatin1String(Theme::BG_HOVER()))
        .arg(QLatin1String(Theme::BG_CARD())));
    connect(btn, &QPushButton::clicked, this, [this, cb = std::move(onClick)]() {
        if (cb) cb();
        collapse();   // an action closes the submenu
    });
    m_submenuLayout->addWidget(btn);
}

int ContactCard::submenuFullHeight() const
{
    // The widget's own sizeHint can be skewed by the maximumHeight clamp we use
    // for the animation, so read the LAYOUT's natural sizeHint (the layout
    // isn't height-constrained) plus the container's content margins. Falls
    // back to the widget sizeHint if the layout isn't ready.
    if (m_submenuLayout) {
        const int h = m_submenuLayout->sizeHint().height();
        if (h > 0) return h;
    }
    return m_submenu ? m_submenu->sizeHint().height() : 0;
}

void ContactCard::setSubmenuHeight(int h)
{
    m_submenuHeight = h;
    if (!m_submenu) return;
    m_submenu->setMaximumHeight(h);
    m_submenu->setVisible(h > 0);
}

void ContactCard::startAnim(int from, int to)
{
    m_anim->stop();
    m_submenu->setVisible(true);            // visible during the whole animation
    m_anim->setStartValue(from);
    m_anim->setEndValue(to);
    m_anim->start();
}

void ContactCard::expand()
{
    if (m_expanded) return;
    m_expanded = true;
    startAnim(m_submenuHeight, submenuFullHeight());
    update();                // redraw the chevron (now ▴)
    emit expanded(this);     // owner collapses any other open card
}

void ContactCard::collapse()
{
    if (!m_expanded && m_submenuHeight == 0) return;
    m_expanded = false;
    startAnim(m_submenuHeight, 0);
    update();                // redraw the chevron (now ▾)
}

void ContactCard::toggle()
{
    if (m_expanded) collapse(); else expand();
}

void ContactCard::paintEvent(QPaintEvent *e)
{
    QWidget::paintEvent(e);
    if (!m_header) return;
    // A small chevron at the right edge of the HEADER row, vertically centered
    // on it, as a "this expands" affordance. Points down (▾) when collapsed,
    // up (▴) when expanded. Drawn here (not a header child widget) so we don't
    // disturb the caller-built header layout. Subtle: secondary text color.
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing, true);
    const int hH = m_header->height();
    const qreal cx = width() - 14.0;          // a touch in from the right edge
    const qreal cy = hH / 2.0;                 // centered on the header row
    const qreal s  = 4.0;                      // half-width of the chevron
    QColor c(QString::fromLatin1(Theme::TEXT_SECONDARY()));
    c.setAlphaF(0.6);
    QPen pen(c);
    pen.setWidthF(1.6);
    pen.setCapStyle(Qt::RoundCap);
    pen.setJoinStyle(Qt::RoundJoin);
    p.setPen(pen);
    if (m_expanded) {
        // ▴ : two strokes rising to a peak.
        p.drawLine(QPointF(cx - s, cy + s/2), QPointF(cx, cy - s/2));
        p.drawLine(QPointF(cx, cy - s/2), QPointF(cx + s, cy + s/2));
    } else {
        // ▾ : two strokes falling to a trough.
        p.drawLine(QPointF(cx - s, cy - s/2), QPointF(cx, cy + s/2));
        p.drawLine(QPointF(cx, cy + s/2), QPointF(cx + s, cy - s/2));
    }
}

bool ContactCard::eventFilter(QObject *obj, QEvent *ev)
{
    if (obj != m_header) return QWidget::eventFilter(obj, ev);

    switch (ev->type()) {
    case QEvent::MouseButtonPress: {
        auto *me = static_cast<QMouseEvent *>(ev);
        if (me->button() == Qt::LeftButton) {
            m_tracking   = true;
            m_swipeArmed = false;
            m_pressPos   = me->position();
        }
        // Don't consume — let child buttons still receive the press.
        return false;
    }
    case QEvent::MouseMove: {
        if (!m_tracking) return false;
        auto *me = static_cast<QMouseEvent *>(ev);
        const QPointF d = me->position() - m_pressPos;
        const qreal ax = qAbs(d.x());
        const qreal ay = qAbs(d.y());
        // Swipe-out only applies while expanded. Two-stage so it can't be
        // mistaken for the page's vertical scroll OR the unified nav's
        // horizontal page-swipe:
        //   - If VERTICAL motion dominates first (ay clearly > ax), this is a
        //     scroll: stop tracking, don't consume, let the scroller have it.
        //   - Otherwise, once horizontal travel passes the threshold AND is
        //     clearly x-dominant (ax > ay*1.5), arm + collapse + consume.
        if (m_expanded) {
            if (ay > ax && ay >= static_cast<qreal>(kTapSlopPx)) {
                m_tracking = false;   // vertical scroll — bow out cleanly
                return false;
            }
            if (ax >= static_cast<qreal>(kSwipeCloseThresholdPx)
                && ax > ay * 1.5) {
                m_swipeArmed = true;
                m_tracking   = false;
                collapse();
                return true;   // consume so this isn't ALSO read as a page swipe
            }
        } else {
            // Not expanded: any real motion means a scroll, not a tap — stop
            // tracking so the release won't toggle, and don't consume (the
            // scroller needs the events).
            if (ax >= static_cast<qreal>(kTapSlopPx)
                || ay >= static_cast<qreal>(kTapSlopPx)) {
                m_tracking = false;
            }
        }
        return false;
    }
    case QEvent::MouseButtonRelease: {
        if (!m_tracking) return false;
        auto *me = static_cast<QMouseEvent *>(ev);
        const QPointF d = me->position() - m_pressPos;
        m_tracking = false;
        // A tap (negligible travel) toggles the submenu. A larger move that
        // didn't arm the swipe is treated as a scroll/no-op (don't toggle).
        if (!m_swipeArmed && qAbs(d.x()) < kTapSlopPx
            && qAbs(d.y()) < kTapSlopPx) {
            toggle();
            return true;   // consumed the tap as a toggle
        }
        return false;
    }
    default:
        break;
    }
    return QWidget::eventFilter(obj, ev);
}

} // namespace SnapClone::UI
