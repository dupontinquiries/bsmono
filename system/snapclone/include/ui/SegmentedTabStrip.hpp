#pragma once

// ─── ui/SegmentedTabStrip.hpp ─────────────────────────────────────────────────
//
// A compact horizontal tab strip: a row of equal-width checkable buttons with
// an underline indicator on the selected tab. Mirrors the in-page tab pattern
// first built inline in SettingsPage (V32.34+) and extracted here so the
// mailbox, contacts, and other multi-section pages can share one consistent
// look + behavior instead of re-implementing the strip each time.
//
// Usage:
//   auto *strip = new SegmentedTabStrip({tr("All"), tr("Sent")}, this);
//   connect(strip, &SegmentedTabStrip::tabSelected, stack,
//           &QStackedWidget::setCurrentIndex);
//   strip->setBadge(1, 3);   // little "3" pill on the second tab
//
// Pair it with a QStackedWidget for the bodies. The strip owns only the
// header row; the caller owns the stack and the wiring.
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QList>

QT_BEGIN_NAMESPACE
class QPushButton;
class QLabel;
class QHBoxLayout;
class QResizeEvent;
class QShowEvent;
QT_END_NAMESPACE

namespace SnapClone::UI {

class SegmentedTabStrip : public QWidget
{
    Q_OBJECT
public:
    explicit SegmentedTabStrip(const QStringList &labels,
                               QWidget *parent = nullptr);

    /// Programmatically select a tab (also emitted by user taps).
    void setCurrentIndex(int index);
    [[nodiscard]] int currentIndex() const { return m_current; }

    /// Show a small count badge on a tab (0 hides it). Useful for unread
    /// counts on a mailbox "All"/"Unread" tab, queued counts on "Sent", etc.
    void setBadge(int index, int count);

    /// Update a tab's label text after construction (e.g. "Sent (2)").
    void setLabel(int index, const QString &text);

    /// V32.34+ finalize: remember the last-open tab across launches. Pass a
    /// unique QSettings key (e.g. "ui/tabs/settings"). When set, the strip
    /// immediately selects the saved index (clamped to range, default 0) —
    /// which emits tabSelected so the host's stack/visibility syncs — and
    /// from then on writes the index to QSettings on every change. Call this
    /// AFTER connecting to tabSelected so the restore selection reaches the
    /// host. Safe to never call: without a key the strip is purely in-memory.
    void setPersistenceKey(const QString &key);

signals:
    void tabSelected(int index);

protected:
    // V32.34+: badges are positioned from the button width, which is 0
    // before the first layout pass — so a badge set during construction
    // would sit at x=0 until something forced a re-layout. Repositioning on
    // resize + show fixes first-paint placement across all surfaces using
    // this strip (settings, mailbox, contacts).
    void resizeEvent(QResizeEvent *e) override;
    void showEvent(QShowEvent *e) override;

private:
    void rebuildBadge(int index);
    void repositionBadge(int index);  ///< place the badge from current width

    struct TabBtn {
        QPushButton *button = nullptr;
        QLabel      *badge  = nullptr;  // overlaid count pill, created lazily
        int          count  = 0;
    };
    QList<TabBtn>  m_tabs;
    QHBoxLayout   *m_row     = nullptr;
    int            m_current = 0;
    QString        m_persistKey;  ///< V32.34+: QSettings key, empty = no persistence
};

} // namespace SnapClone::UI
