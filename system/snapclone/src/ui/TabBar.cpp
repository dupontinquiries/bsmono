#include "ui/TabBar.hpp"
#include "ui/Theme.hpp"

#include <QPainter>
#include <QMouseEvent>
#include <QTouchEvent>
#include <QPropertyAnimation>
#include <QGraphicsOpacityEffect>
#include <QEasingCurve>
#include <QTimer>
#include <QAbstractAnimation>

namespace SnapClone::UI {

static constexpr int kBarHeight    = 56;   // total height
static constexpr int kIndicatorH   = 3;    // accent line height
static constexpr int kIconSize     = 20;   // emoji font size
static constexpr int kLabelSize    = 10;   // label font size

TabBar::TabBar(QWidget *parent)
    : QWidget(parent)
{
    setMinimumHeight(kBarHeight);
    setMaximumHeight(kBarHeight);
    setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Fixed);
    setFocusPolicy(Qt::NoFocus); // don't steal focus from page widgets
    setAttribute(Qt::WA_OpaquePaintEvent);
    // Accept touch events directly — the parent application disables
    // AA_SynthesizeMouseForUnhandledTouchEvents so we must handle
    // touch ourselves rather than relying on synthetic mouse events.
    setAttribute(Qt::WA_AcceptTouchEvents);
    m_anim = new QPropertyAnimation(this, "indicatorX", this);
    m_anim->setDuration(Theme::ANIM_FAST);
    m_anim->setEasingCurve(QEasingCurve::OutCubic);
}

void TabBar::addTab(const QString &icon, const QString &label)
{
    m_tabs.append(Tab{ icon, QIcon{}, label });
    update();
}

int TabBar::tabWidth() const
{
    if (m_tabs.isEmpty()) return width();
    return width() / static_cast<int>(m_tabs.size());
}

int TabBar::indicatorTargetX(int index) const
{
    const int tw = tabWidth();
    return index * tw + tw / 2 - tw / 6; // centre the indicator under the tab
}

void TabBar::animateIndicator(int toX)
{
    m_anim->stop();
    m_anim->setStartValue(m_indicatorX);
    m_anim->setEndValue(toX);
    m_anim->start();
}

void TabBar::setCurrentIndex(int index)
{
    if (index < 0 || index >= static_cast<int>(m_tabs.size())) return;
    if (index == m_current && m_indicatorX == indicatorTargetX(index)) return;
    m_current = index;
    animateIndicator(indicatorTargetX(index));
    update();

    // Tactile feedback: a short fade-highlight overlay on the newly-selected
    // tab. The bar height stays constant (no more bounce/grow/shrink that
    // pushed page content). The slide of m_indicatorX above gives the
    // primary motion; this fade is the accent that confirms the tap landed.
    //
    // Implementation: a child QWidget sized to the tab area, with a
    // QGraphicsOpacityEffect that fades from 0.35 → 0 over 350ms. WA_TFME
    // so it never blocks input. Self-deletes when fade completes.
    //
    // CRITICAL: before creating a new highlight, delete any in-flight
    // previous one. During rapid swiping (page-change → setCurrentIndex
    // every ~100ms), highlights would stack up on the tab bar, each with
    // its own QGraphicsOpacityEffect. Multiple effects overlapping the
    // same paint area caused "A paint device can only be painted by one
    // painter at a time" warnings (visible in the log) and likely
    // contributed to crashes.
    if (index >= 0 && index < static_cast<int>(m_tabs.size())) {
        // Delete the previous highlight if it's still alive (QPointer
        // auto-nulls when the highlight's deleteLater fires after fade).
        if (m_activeHighlight) {
            m_activeHighlight->hide();
            m_activeHighlight->deleteLater();
            m_activeHighlight = nullptr;
        }

        const int tw   = tabWidth();
        // Tab LEFT edge is index * tw — NOT indicatorTargetX which is
        // the offset for the narrow accent line at the top (tw/3 wide,
        // centered). Using indicatorTargetX here placed the highlight
        // starting one-third into the tab and extending past the right
        // edge — visible as a misaligned flashing box.
        const int tabX = index * tw;
        auto *highlight = new QWidget(this);
        m_activeHighlight = highlight;
        highlight->setGeometry(tabX, 0, tw, height());
        highlight->setAttribute(Qt::WA_TransparentForMouseEvents, true);
        highlight->setStyleSheet(QStringLiteral(
            "background: %1; border-radius: 0px;")
            .arg(QLatin1String(Theme::ACCENT_SOFT())));
        highlight->show();
        highlight->raise();

        auto *fx = new QGraphicsOpacityEffect(highlight);
        fx->setOpacity(0.35);
        highlight->setGraphicsEffect(fx);

        auto *fade = new QPropertyAnimation(fx, "opacity", highlight);
        fade->setDuration(350);
        fade->setStartValue(0.35);
        fade->setEndValue(0.0);
        fade->setEasingCurve(QEasingCurve::OutCubic);
        QObject::connect(fade, &QPropertyAnimation::finished,
                         highlight, &QWidget::deleteLater);
        fade->start(QAbstractAnimation::DeleteWhenStopped);
    }
}

void TabBar::setBadge(int tabIndex, int count)
{
    if (count > 0)
        m_badges[tabIndex] = count;
    else
        m_badges.remove(tabIndex);
    update();
}

void TabBar::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing);
    p.setRenderHint(QPainter::SmoothPixmapTransform);

    // Background: frosted dark panel
    p.fillRect(rect(), QColor(QLatin1String(Theme::BG_RAISED())));

    // Top separator
    p.setPen(QColor(QLatin1String(Theme::BORDER())));
    p.drawLine(0, 0, width(), 0);

    if (m_tabs.isEmpty()) return;

    const int tw     = tabWidth();
    const int h      = height();   // animated — changes on press
    const int iconSz = 22;
    const int lblSz  = 10;

    // Tabs
    for (int i = 0; i < static_cast<int>(m_tabs.size()); ++i) {
        const bool active = (i == m_current);
        const QRect tabRect(i * tw, 0, tw, h);

        // Active glow pill — tighter, more opaque than before
        if (active) {
            QColor glow(QLatin1String(Theme::ACCENT()));
            glow.setAlpha(35);
            p.setPen(Qt::NoPen);
            p.setBrush(glow);
            const int pillW = tw - 12;
            const int pillH = h - 14;
            const int pillX = tabRect.x() + (tw - pillW) / 2;
            const int pillY = 10;
            p.drawRoundedRect(pillX, pillY, pillW, pillH, pillH/2, pillH/2);
        }

        // Icon — SVG preferred, emoji fallback
        const QColor iconColor = active
            ? QColor(QLatin1String(Theme::ACCENT()))
            : QColor(QLatin1String(Theme::TEXT_SECONDARY()));
        const QRect iconRect(tabRect.x(), tabRect.y() + 10,
                             tabRect.width(), iconSz);

        bool drewSvg = false;
        if (!m_tabs[i].svgIcon.isNull()) {
            const QPixmap px = m_tabs[i].svgIcon.pixmap(
                iconSz * 2, iconSz * 2,
                QIcon::Normal,
                active ? QIcon::On : QIcon::Off);
            if (!px.isNull()) {
                const QRect dst(iconRect.left() + (iconRect.width() - iconSz)/2,
                                iconRect.top(),
                                iconSz, iconSz);
                p.drawPixmap(dst, px, px.rect());
                drewSvg = true;
            }
        }
        if (!drewSvg) {
            QFont ef = p.font();
            ef.setPixelSize(iconSz);
            p.setFont(ef);
            p.setPen(iconColor);
            p.drawText(iconRect, Qt::AlignCenter, m_tabs[i].icon);
        }

        // Label
        QFont lf = p.font();
        lf.setPixelSize(lblSz);
        lf.setWeight(active ? QFont::DemiBold : QFont::Normal);
        p.setFont(lf);
        p.setPen(iconColor);
        const QRect labelRect(tabRect.x(), tabRect.y() + 10 + iconSz + 2,
                              tabRect.width(), lblSz + 4);
        p.drawText(labelRect, Qt::AlignCenter, m_tabs[i].label);

        // Unread badge
        if (m_badges.contains(i) && m_badges[i] > 0) {
            const int dotR  = 5;
            const int dotCx = tabRect.x() + tabRect.width()/2 + iconSz/2 - 2;
            const int dotCy = tabRect.y() + 12;
            p.setPen(Qt::NoPen);
            p.setBrush(QColor(QLatin1String(Theme::BG_DEEP())));
            p.drawEllipse(QPoint(dotCx, dotCy), dotR+2, dotR+2);
            p.setBrush(QColor(QLatin1String(Theme::DANGER())));
            p.drawEllipse(QPoint(dotCx, dotCy), dotR, dotR);
            if (m_badges[i] > 9) {
                QFont bf; bf.setPixelSize(7); bf.setBold(true);
                p.setFont(bf);
                p.setPen(Qt::white);
                p.drawText(QRect(dotCx-dotR, dotCy-dotR, dotR*2, dotR*2),
                           Qt::AlignCenter, QStringLiteral("9+"));
            }
        }
    }

    // Accent indicator bar at top (skip if not yet positioned)
    if (m_indicatorX >= 0) {
        const int indW = tw / 3;
        p.setPen(Qt::NoPen);
        p.setBrush(QColor(QLatin1String(Theme::ACCENT())));
        p.drawRoundedRect(m_indicatorX, 0, indW, kIndicatorH, 1, 1);
    }
}

void TabBar::mousePressEvent(QMouseEvent *e)
{
    if (m_tabs.isEmpty()) return;
    const int tw = tabWidth();
    const int idx = e->position().toPoint().x() / tw;
    if (idx >= 0 && idx < static_cast<int>(m_tabs.size())) {
        setCurrentIndex(idx);
        emit tabSelected(idx);
    }
}

bool TabBar::event(QEvent *e)
{
    // Handle touch events directly for touchscreen devices.
    //
    // CRITICAL: include TouchUpdate. We don't ACT on it (no drag behavior
    // on tabs), but we must accept it so Qt doesn't consider the touch
    // abandoned and send a TouchCancel before TouchEnd arrives. Some
    // platforms (especially Wayland compositors) treat unhandled
    // TouchUpdate as "widget gave up on this sequence" and reroute or
    // cancel — which is the "taps don't work on mobile" failure mode.
    //
    // We also widen the press-tolerance: the user's finger may drift a
    // few pixels during a tap. We commit on TouchEnd based on the
    // RELEASE position (where the finger left the screen), not the
    // begin position — same as buttons.
    if (e->type() == QEvent::TouchBegin
        || e->type() == QEvent::TouchUpdate
        || e->type() == QEvent::TouchEnd
        || e->type() == QEvent::TouchCancel) {
        auto *te = static_cast<QTouchEvent *>(e);
        const auto &pts = te->points();
        if (!pts.isEmpty() && e->type() == QEvent::TouchEnd) {
            const QPoint localPos = pts.first().position().toPoint();
            const int tw = tabWidth();
            if (tw > 0) {
                const int idx = localPos.x() / tw;
                if (idx >= 0 && idx < static_cast<int>(m_tabs.size())
                    && localPos.y() >= 0 && localPos.y() < height()) {
                    setCurrentIndex(idx);
                    emit tabSelected(idx);
                }
            }
        }
        e->accept();
        return true;
    }
    return QWidget::event(e);
}

void TabBar::resizeEvent(QResizeEvent *)
{
    // Snap indicator to correct position after resize (no animation)
    m_indicatorX = indicatorTargetX(m_current);
    update();
}

void TabBar::showEvent(QShowEvent *e)
{
    QWidget::showEvent(e);
    // Position indicator on first show — tabWidth() requires valid width()
    if (m_indicatorX < 0 && !m_tabs.isEmpty())
        m_indicatorX = indicatorTargetX(m_current);
}

void TabBar::setTabSvgIcon(int tabIndex, const QIcon &icon)
{
    if (tabIndex < 0 || tabIndex >= static_cast<int>(m_tabs.size())) return;
    m_tabs[tabIndex].svgIcon = icon;
    update();
}

} // namespace SnapClone::UI
