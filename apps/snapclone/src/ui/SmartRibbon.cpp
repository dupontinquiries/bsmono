#include "ui/SmartRibbon.hpp"
#include "ui/Theme.hpp"
#include "ui/SvgIcon.hpp"

#include <QHBoxLayout>
#include <QPushButton>
#include <QLabel>
#include <QResizeEvent>
#include <QShowEvent>
#include <QPaintEvent>
#include <QPainter>
#include <QSettings>

namespace SnapClone::UI {

SmartRibbon::SmartRibbon(const QStringList &tabLabels, QWidget *parent)
    : QWidget(parent)
{
    // One row: [tab segments] … flexible spacer … [contextual actions].
    m_row = new QHBoxLayout(this);
    m_row->setContentsMargins(Theme::pageSideMarginPx(), Theme::SPACE_XS,
                              Theme::pageSideMarginPx(), Theme::SPACE_XS);
    m_row->setSpacing(Theme::SPACE_SM);

    // ── Left: tab segments ───────────────────────────────────────────────
    // Natural-width buttons (NOT equal-expanding like SegmentedTabStrip) so
    // the right side keeps room for actions. The selected-tab indicator is
    // painted on the TOP edge in paintEvent (bottom-docked bar).
    m_segGroup = new QHBoxLayout();
    m_segGroup->setContentsMargins(0, 0, 0, 0);
    m_segGroup->setSpacing(Theme::SPACE_XS);

    const QString btnStyle = QStringLiteral(
        "QPushButton { background: transparent; color: %1; border: none;"
        "  border-radius: 0; padding: 8px 4px; font-size: %2px; font-weight: 500; }"
        "QPushButton:hover { color: %3; }"
        "QPushButton:checked { color: %3; font-weight: 700; }")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(Theme::FONT_SM)
        .arg(QLatin1String(Theme::TEXT_PRIMARY()));

    // V32.34+ height fix: clamp every bar item to a single fixed height
    // (TOUCH_MIN already includes comfortable touch padding) so segments and
    // action buttons line up and neither can stretch the bar. Using only a
    // minimum let the stylesheet padding inflate the sizeHint, which made the
    // action buttons render too tall on the Sent/Unread tabs.
    const int kBarItemH = Theme::TOUCH_MIN;
    for (int i = 0; i < tabLabels.size(); ++i) {
        auto *btn = new QPushButton(tabLabels[i], this);
        btn->setCheckable(true);
        btn->setCursor(Qt::PointingHandCursor);
        btn->setStyleSheet(btnStyle);
        btn->setFixedHeight(kBarItemH);
        btn->setSizePolicy(QSizePolicy::Maximum, QSizePolicy::Fixed);
        connect(btn, &QPushButton::clicked, this, [this, i]() {
            setCurrentIndex(i);
        });
        m_tabs.append(TabBtn{ btn, nullptr, 0 });
        m_actions.append(QList<Action>{});   // empty action set until registered
        m_segGroup->addWidget(btn, 0);
    }
    if (!m_tabs.isEmpty()) m_tabs[0].button->setChecked(true);

    // ── Right: contextual actions (populated per active tab) ─────────────
    m_actGroup = new QHBoxLayout();
    m_actGroup->setContentsMargins(0, 0, 0, 0);
    m_actGroup->setSpacing(Theme::SPACE_XS);

    m_row->addLayout(m_segGroup, 0);
    m_row->addStretch(1);            // pushes actions to the right edge
    m_row->addLayout(m_actGroup, 0);

    // Bar surface: a raised strip with a hairline top border so it reads as a
    // distinct bottom bar against the page content above it.
    setStyleSheet(QStringLiteral(
        "SnapClone--UI--SmartRibbon { background: %1;"
        "  border-top: 1px solid %2; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::BORDER())));
    setAttribute(Qt::WA_StyledBackground, true);

    // V32.34+ height fix: the whole bar is exactly one item tall plus the
    // row's top+bottom margins (SPACE_XS each) plus the 1px top border.
    // Fixing it stops the action buttons' box from inflating the bar and
    // keeps the bottom strip from eating the page's vertical budget at
    // 380px. This is the "max height = lines needed + touch padding" cap.
    const int barH = Theme::TOUCH_MIN + 2 * Theme::SPACE_XS + 1;
    setFixedHeight(barH);
    setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Fixed);

    rebuildActionButtons();
}

void SmartRibbon::setCurrentIndex(int index)
{
    if (index < 0 || index >= m_tabs.size()) return;
    m_current = index;
    for (int i = 0; i < m_tabs.size(); ++i)
        if (m_tabs[i].button) m_tabs[i].button->setChecked(i == index);

    rebuildActionButtons();   // swap the right-side actions to this tab's set

    if (!m_persistKey.isEmpty()) {
        QSettings s;
        s.setValue(m_persistKey, index);
    }
    update();                 // repaint the indicator at the new segment
    emit tabSelected(index);
}

void SmartRibbon::setTabActions(int tabIndex, const QList<Action> &actions)
{
    if (tabIndex < 0 || tabIndex >= m_actions.size()) return;
    m_actions[tabIndex] = actions;
    if (tabIndex == m_current) rebuildActionButtons();
}

void SmartRibbon::rebuildActionButtons()
{
    // Tear down the previous tab's action buttons.
    for (auto *b : m_actionButtons) { m_actGroup->removeWidget(b); b->deleteLater(); }
    m_actionButtons.clear();

    if (m_current < 0 || m_current >= m_actions.size()) return;

    const QString actStyle = QStringLiteral(
        "QPushButton { background: transparent; color: %1; border: none;"
        "  border-radius: %2px; padding: 0 8px; font-size: %3px; }"
        "QPushButton:hover:enabled { color: %4; background: %5; }"
        "QPushButton:disabled { color: %6; }")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(Theme::RADIUS_SM)
        .arg(Theme::FONT_SM)
        .arg(QLatin1String(Theme::ACCENT()))
        .arg(QLatin1String(Theme::BG_HOVER()))
        .arg(QLatin1String(Theme::TEXT_DISABLED()));

    const int kBarItemH = Theme::TOUCH_MIN;
    for (const Action &a : m_actions[m_current]) {
        auto *btn = new QPushButton(a.label, this);
        if (!a.icon.isEmpty())
            btn->setIcon(SvgIcon::secondary(a.icon));
        btn->setCursor(Qt::PointingHandCursor);
        btn->setStyleSheet(actStyle);
        // V32.34+ height fix: fixed height (vertical padding handled by the
        // fixed box, not stylesheet padding) so the action buttons match the
        // segment height exactly and can't render too tall.
        btn->setFixedHeight(kBarItemH);
        btn->setSizePolicy(QSizePolicy::Maximum, QSizePolicy::Fixed);
        // Capture the trigger by value; guard against a null trigger.
        const auto trig = a.trigger;
        connect(btn, &QPushButton::clicked, this, [trig]() {
            if (trig) trig();
        });
        // Initial enabled state from the predicate (null = always enabled).
        btn->setEnabled(a.enabled ? a.enabled() : true);
        m_actionButtons.append(btn);
        m_actGroup->addWidget(btn, 0);
    }
}

void SmartRibbon::refreshActions()
{
    if (m_current < 0 || m_current >= m_actions.size()) return;
    const auto &set = m_actions[m_current];
    // m_actionButtons is index-aligned with the current tab's action set.
    for (int i = 0; i < m_actionButtons.size() && i < set.size(); ++i) {
        const auto &a = set[i];
        m_actionButtons[i]->setEnabled(a.enabled ? a.enabled() : true);
    }
}

void SmartRibbon::setBadge(int index, int count)
{
    if (index < 0 || index >= m_tabs.size()) return;
    TabBtn &t = m_tabs[index];
    t.count = count;
    if (count <= 0) {
        if (t.badge) { t.badge->hide(); }
        return;
    }
    if (!t.badge) {
        t.badge = new QLabel(this);
        t.badge->setAttribute(Qt::WA_TransparentForMouseEvents);
        t.badge->setAlignment(Qt::AlignCenter);
        t.badge->setStyleSheet(QStringLiteral(
            "QLabel { background: %1; color: #fff; border-radius: %2px;"
            "  font-size: %3px; font-weight: 700; padding: 0 5px; }")
            .arg(QLatin1String(Theme::ACCENT()))
            .arg(Theme::RADIUS_FULL)
            .arg(Theme::FONT_XS));
    }
    t.badge->setText(count > 99 ? QStringLiteral("99+")
                                : QString::number(count));
    t.badge->show();
    t.badge->raise();
    repositionBadge(index);
}

void SmartRibbon::setLabel(int index, const QString &text)
{
    if (index < 0 || index >= m_tabs.size()) return;
    if (m_tabs[index].button) m_tabs[index].button->setText(text);
}

void SmartRibbon::setPersistenceKey(const QString &key)
{
    m_persistKey = key;
    if (key.isEmpty()) return;
    QSettings s;
    int saved = s.value(key, 0).toInt();
    if (saved < 0 || saved >= m_tabs.size()) saved = 0;
    setCurrentIndex(saved);   // emits tabSelected so the host syncs
}

void SmartRibbon::repositionBadge(int index)
{
    if (index < 0 || index >= m_tabs.size()) return;
    TabBtn &t = m_tabs[index];
    if (!t.badge || t.count <= 0 || !t.button) return;
    t.badge->adjustSize();
    const QRect bg = t.button->geometry();
    // Top-right of the segment button (bottom-docked bar → badge on top edge).
    const int bw = t.badge->width();
    int x = bg.right() - bw + 2;
    int y = bg.top() - 2;
    if (x < 0) x = 0;
    if (y < 0) y = 0;
    t.badge->move(x, y);
}

int SmartRibbon::indicatorTargetX() const
{
    if (m_current < 0 || m_current >= m_tabs.size()) return -1;
    const auto *btn = m_tabs[m_current].button;
    return btn ? btn->geometry().left() : -1;
}

int SmartRibbon::indicatorWidth() const
{
    if (m_current < 0 || m_current >= m_tabs.size()) return 0;
    const auto *btn = m_tabs[m_current].button;
    return btn ? btn->width() : 0;
}

void SmartRibbon::paintEvent(QPaintEvent *e)
{
    QWidget::paintEvent(e);   // styled background (WA_StyledBackground) + top border
    // Selected-tab indicator on the TOP edge of the bar (the edge facing the
    // page content, since this bar is bottom-docked).
    const int x = indicatorTargetX();
    const int w = indicatorWidth();
    if (x < 0 || w <= 0) return;
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing, false);
    p.fillRect(QRect(x, 0, w, 2), QColor(QLatin1String(Theme::ACCENT())));
}

void SmartRibbon::resizeEvent(QResizeEvent *e)
{
    QWidget::resizeEvent(e);
    for (int i = 0; i < m_tabs.size(); ++i)
        if (m_tabs[i].count > 0) repositionBadge(i);
}

void SmartRibbon::showEvent(QShowEvent *e)
{
    QWidget::showEvent(e);
    for (int i = 0; i < m_tabs.size(); ++i)
        if (m_tabs[i].count > 0) repositionBadge(i);
}

} // namespace SnapClone::UI
