#pragma once

// ─── ui/ExportReminderCard.hpp ────────────────────────────────────────────────
//
// The "remember when?" monthly export prompt. A dismissable card shown at the
// top of the Inbox on the first launch of a new month (gated by
// MonthlyExportReminder::shouldShowThisMonth() AND a minimum recent-snap
// count, decided by the host). Surfaces a strip of small thumbnails from the
// user's recent snaps, a short prompt, and two actions:
//   • "Export as zip"  → emits exportRequested()  (host runs SnapExporter +
//                         save/share, then typically hides the card)
//   • "Maybe later"     → emits dismissed()        (host calls
//                         MonthlyExportReminder::snoozeForThisMonth() + hides)
//
// The card owns only its presentation; the host owns the show/hide decision,
// the snooze bookkeeping, and the actual export. Thumbnails load async via
// ThumbnailService with `this` as the lifetime guard, so a card destroyed
// mid-load drops its callbacks safely.
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QStringList>

QT_BEGIN_NAMESPACE
class QHBoxLayout;
QT_END_NAMESPACE

namespace SnapClone::UI {

class ExportReminderCard : public QWidget
{
    Q_OBJECT
public:
    /// `recentMediaPaths` — a handful of recent snap media files to preview as
    /// thumbnails (host picks them; the card shows up to kMaxThumbs). May be
    /// empty (the card still renders with just the prompt + buttons).
    explicit ExportReminderCard(const QStringList &recentMediaPaths,
                                QWidget *parent = nullptr);

signals:
    void exportRequested();
    void dismissed();

private:
    static constexpr int kMaxThumbs = 4;
    static constexpr int kThumbSize = 56;
    void loadThumbnails(const QStringList &paths);
    QHBoxLayout *m_thumbRow = nullptr;
};

} // namespace SnapClone::UI
