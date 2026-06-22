#pragma once

// ─── ui/SmartRibbon.hpp ───────────────────────────────────────────────────────
//
// A one-row "smart function ribbon" for mobile: tab segments on the LEFT and
// the active tab's contextual ACTION buttons on the RIGHT, in a single bar.
// Designed to live at the BOTTOM of a surface for thumb reach (Plasma Mobile /
// FP5). The action half adapts to the selected tab — e.g. on a Contacts
// "Pending" tab the ribbon offers accept-all / clear; on a Mailbox "Sent" tab
// it offers retry-failed — while the tab half stays constant for the surface.
//
// Relationship to SegmentedTabStrip: that widget is the top-docked, full-width
// equal-segment strip used inside Settings/Mailbox/Contacts today. SmartRibbon
// is the bottom-docked variant that ALSO carries contextual actions. It reuses
// the same visual language (checkable segments, accent indicator, count
// badges) but lays the segments out at their natural width on the left so the
// right side has room for actions. This is the prototype for the eventual
// unified bottom bar that will REPLACE the page-nav TabBar (user directive
// V32.34+); for now it is used standalone inside one surface to validate the
// layout + thumb-reach + 380px height budget before the global rewire.
//
// Usage:
//   auto *ribbon = new SmartRibbon({tr("All"), tr("Unread"), tr("Sent")}, this);
//   // Register contextual actions per tab index (0 = All, 1 = Unread, …):
//   ribbon->setTabActions(1, {
//       { QStringLiteral("check"), tr("Mark all read"),
//         [this]{ return m_unread > 0; },   // enabled-predicate
//         [this]{ markAllRead(); } }        // on-trigger
//   });
//   connect(ribbon, &SmartRibbon::tabSelected, stack, &QStackedWidget::setCurrentIndex);
//   ribbon->setBadge(1, unreadCount);
//
// The ribbon owns only the bar; the caller owns the page bodies + the actions'
// behavior. Call refreshActions() after state changes so enabled-predicates
// re-evaluate (e.g. after snaps arrive, after a contact is accepted).
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QList>
#include <QString>
#include <functional>

QT_BEGIN_NAMESPACE
class QPushButton;
class QLabel;
class QHBoxLayout;
class QResizeEvent;
class QShowEvent;
QT_END_NAMESPACE

namespace SnapClone::UI {

class SmartRibbon : public QWidget
{
    Q_OBJECT
public:
    /// A contextual action shown on the right of the ribbon for a given tab.
    struct Action {
        QString icon;                       ///< SvgIcon name (assets/icons/<name>.svg)
        QString label;                      ///< short caption (may be hidden on narrow bars)
        std::function<bool()>  enabled;     ///< re-evaluated on refreshActions(); null = always on
        std::function<void()>  trigger;     ///< invoked on tap
    };

    explicit SmartRibbon(const QStringList &tabLabels, QWidget *parent = nullptr);

    /// Programmatically select a tab (also emitted by user taps). Swaps the
    /// visible action set to that tab's actions.
    void setCurrentIndex(int index);
    [[nodiscard]] int currentIndex() const { return m_current; }

    /// Register the contextual actions for a tab. Replaces any existing set
    /// for that index. Safe to call before or after the tab is shown.
    void setTabActions(int tabIndex, const QList<Action> &actions);

    /// Re-evaluate every visible action's enabled-predicate and update the
    /// buttons' enabled state. Call after any state change that could flip an
    /// action's availability (snap arrival, contact accept, etc.).
    void refreshActions();

    /// Show a small count badge on a tab (0 hides it).
    void setBadge(int index, int count);

    /// Update a tab's label text after construction.
    void setLabel(int index, const QString &text);

    /// V32.34+: remember the last-open tab across launches (QSettings key).
    /// Mirrors SegmentedTabStrip::setPersistenceKey — restores the saved
    /// index on call (emitting tabSelected) and writes on every change.
    /// Call AFTER connecting tabSelected so the restore reaches the host.
    void setPersistenceKey(const QString &key);

signals:
    void tabSelected(int index);

protected:
    // Bottom-docked: the selected-tab indicator and the count badges sit on
    // the TOP edge of the bar (the edge facing the page content). Both are
    // positioned from button geometry, which is 0 before first layout, so we
    // reposition on resize + show — same first-paint fix as SegmentedTabStrip.
    void resizeEvent(QResizeEvent *e) override;
    void showEvent(QShowEvent *e) override;
    void paintEvent(QPaintEvent *e) override;

private:
    void rebuildActionButtons();   ///< (re)create the right-side buttons for m_current
    void repositionBadge(int index);
    int  indicatorTargetX() const; ///< left edge of the selected tab segment
    int  indicatorWidth() const;   ///< width of the selected tab segment

    struct TabBtn {
        QPushButton *button = nullptr;
        QLabel      *badge  = nullptr;  ///< overlaid count pill, created lazily
        int          count  = 0;
    };
    QList<TabBtn>          m_tabs;
    QList<QList<Action>>   m_actions;        ///< per-tab action sets (index-aligned with m_tabs)
    QList<QPushButton*>    m_actionButtons;  ///< currently-shown right-side buttons

    QHBoxLayout *m_row        = nullptr;  ///< the single row: [segments] … [actions]
    QHBoxLayout *m_segGroup   = nullptr;  ///< left: tab segments
    QHBoxLayout *m_actGroup   = nullptr;  ///< right: contextual actions
    int          m_current    = 0;
    QString      m_persistKey;
};

} // namespace SnapClone::UI
