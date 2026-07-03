#include "ui/SegmentedTabStrip.hpp"
#include "ui/Theme.hpp"

#include <QHBoxLayout>
#include <QPushButton>
#include <QLabel>
#include <QResizeEvent>  // V32.34+: badge first-paint reposition
#include <QShowEvent>
#include <QSettings>     // V32.34+: remember last-open tab

namespace SnapClone::UI {

SegmentedTabStrip::SegmentedTabStrip(const QStringList &labels, QWidget *parent)
    : QWidget(parent)
{
    m_row = new QHBoxLayout(this);
    m_row->setContentsMargins(Theme::pageSideMarginPx(), Theme::SPACE_SM,
                              Theme::pageSideMarginPx(), 0);
    m_row->setSpacing(Theme::SPACE_XS);

    const QString btnStyle = QStringLiteral(
        "QPushButton { background: transparent; color: %1; border: none;"
        "  border-bottom: 2px solid transparent; border-radius: 0;"
        "  padding: 8px 6px; font-size: %2px; font-weight: 500; }"
        "QPushButton:hover { color: %3; }"
        "QPushButton:checked { color: %3; border-bottom-color: %4;"
        "  font-weight: 700; }")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(Theme::FONT_SM)
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::ACCENT()));

    for (int i = 0; i < labels.size(); ++i) {
        auto *btn = new QPushButton(labels[i], this);
        btn->setCheckable(true);
        btn->setCursor(Qt::PointingHandCursor);
        btn->setStyleSheet(btnStyle);
        btn->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Fixed);
        connect(btn, &QPushButton::clicked, this, [this, i]() {
            setCurrentIndex(i);
        });
        m_tabs.append(TabBtn{ btn, nullptr, 0 });
        m_row->addWidget(btn, 1);
    }
    if (!m_tabs.isEmpty()) m_tabs[0].button->setChecked(true);

    // V32.34+: cap the strip's height to exactly what the tab row needs —
    // the button content (text + the 8px vertical padding in btnStyle) plus
    // the row's SPACE_SM top margin. Without a cap the strip can claim extra
    // vertical space from a parent layout that has slack, pushing the page
    // content down; "max height = lines needed + touch padding" keeps it
    // tight. FONT_SM line + 16px padding ≈ the natural button height; we add
    // a small floor via TOUCH_MIN so the tap target stays comfortable.
    const int rowH = qMax(Theme::TOUCH_MIN, Theme::FONT_SM + 16) + Theme::SPACE_SM;
    setMaximumHeight(rowH);
    setSizePolicy(QSizePolicy::Preferred, QSizePolicy::Fixed);
}

void SegmentedTabStrip::setCurrentIndex(int index)
{
    if (index < 0 || index >= m_tabs.size()) return;
    m_current = index;
    for (int i = 0; i < m_tabs.size(); ++i)
        if (m_tabs[i].button) m_tabs[i].button->setChecked(i == index);
    // V32.34+: persist the selection if a key was set. Cheap — QSettings
    // batches writes and this only fires on actual tab changes (user taps
    // or programmatic selects), not per-frame.
    if (!m_persistKey.isEmpty()) {
        QSettings s;
        s.setValue(m_persistKey, index);
    }
    emit tabSelected(index);
}

void SegmentedTabStrip::setPersistenceKey(const QString &key)
{
    m_persistKey = key;
    if (key.isEmpty()) return;
    // Restore the saved tab. Clamp to range in case the tab set shrank
    // between versions. setCurrentIndex emits tabSelected so the host syncs
    // its stack/visibility to the restored tab — which is why callers must
    // wire tabSelected before calling this.
    QSettings s;
    bool ok = false;
    int saved = s.value(key, 0).toInt(&ok);
    if (!ok || saved < 0 || saved >= m_tabs.size()) saved = 0;
    setCurrentIndex(saved);
}

void SegmentedTabStrip::setLabel(int index, const QString &text)
{
    if (index < 0 || index >= m_tabs.size()) return;
    if (m_tabs[index].button) m_tabs[index].button->setText(text);
}

void SegmentedTabStrip::setBadge(int index, int count)
{
    if (index < 0 || index >= m_tabs.size()) return;
    if (m_tabs[index].count == count) return;
    m_tabs[index].count = count;
    rebuildBadge(index);
}

void SegmentedTabStrip::rebuildBadge(int index)
{
    TabBtn &t = m_tabs[index];
    if (t.count <= 0) {
        if (t.badge) { t.badge->deleteLater(); t.badge = nullptr; }
        return;
    }
    if (!t.badge) {
        // Overlay the badge on the button as a child, top-right corner.
        t.badge = new QLabel(t.button);
        t.badge->setAttribute(Qt::WA_TransparentForMouseEvents);
        t.badge->setStyleSheet(QStringLiteral(
            "QLabel { background: %1; color: white; border-radius: %2px;"
            "  font-size: %3px; font-weight: 700; padding: 0 5px; }")
            .arg(QLatin1String(Theme::ACCENT()))
            .arg(Theme::RADIUS_FULL)
            .arg(Theme::FONT_XS));
    }
    t.badge->setText(t.count > 99 ? QStringLiteral("99+")
                                  : QString::number(t.count));
    t.badge->adjustSize();
    repositionBadge(index);
    t.badge->show();
    t.badge->raise();
}

void SegmentedTabStrip::repositionBadge(int index)
{
    if (index < 0 || index >= m_tabs.size()) return;
    TabBtn &t = m_tabs[index];
    if (!t.badge || !t.button) return;
    // Pin to the top-right of the button. button->width() is valid once the
    // strip has been laid out (resize/show); before that it's 0 and the
    // badge would clamp to x=0 — which is why resizeEvent/showEvent re-call
    // this once real widths exist.
    const int x = t.button->width() - t.badge->width() - 2;
    t.badge->move(qMax(0, x), 2);
}

void SegmentedTabStrip::resizeEvent(QResizeEvent *e)
{
    QWidget::resizeEvent(e);
    for (int i = 0; i < m_tabs.size(); ++i) repositionBadge(i);
}

void SegmentedTabStrip::showEvent(QShowEvent *e)
{
    QWidget::showEvent(e);
    for (int i = 0; i < m_tabs.size(); ++i) repositionBadge(i);
}

} // namespace SnapClone::UI
