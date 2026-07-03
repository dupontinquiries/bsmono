#pragma once
#include "core/Types.hpp"
#include <QWidget>
#include <QHash>
#include <QDateTime>

class QVBoxLayout;
class QLabel;
class QScrollArea;

// ─── UI/OutboxWidget.hpp ──────────────────────────────────────────────────────
//
// Outbound snap log: shows each sent snap with its delivery status,
// per-peer network phase ("Connecting…", "Relay", "Delivered"), and a
// group summary pill ("2 / 5 delivered").
//
// Cards are sorted newest-first. Delivered snaps fade to a muted colour;
// queued/in-flight snaps pulse their status label.
// ─────────────────────────────────────────────────────────────────────────────

namespace SnapClone::UI {

class OutboxWidget : public QWidget
{
    Q_OBJECT
public:
    explicit OutboxWidget(QWidget *parent = nullptr);

    // Called by MainWindow when the snap is first sent (or queued offline).
    void addSnap(const Core::SnapId &id,
                 const Core::UserId &recipient,
                 const QString      &recipientName,
                 const QDateTime    &sentAt);

    // Network phase update — shown below the snap row.
    // phase matches AppController::peerConnectionStatus phase strings.
    void updateStatus(const Core::SnapId &id, const QString &phase, const QString &detail);

    // Snap confirmed delivered — mark row green, update summary pill.
    void markDelivered(const Core::SnapId &id);

    // Remove all delivered snaps older than 24 h (called periodically).
    void pruneOldDelivered();

signals:
    // Emitted when user taps "Retry" on a failed snap.
    void retryRequested(Core::UserId recipient);

private:
    void buildUi();
    void rebuildSummary();

    struct SnapRow {
        Core::SnapId  id;
        Core::UserId  recipient;
        QString       recipientName;
        QDateTime     sentAt;
        bool          delivered = false;
        QString       phase;
        QString       detail;
        QWidget      *card      = nullptr;
        QLabel       *statusLbl = nullptr;
        QLabel       *timeLbl   = nullptr;
    };

    QList<SnapRow>     m_rows;
    QLabel            *m_summaryPill  = nullptr;
    QLabel            *m_emptyLabel   = nullptr;
    QWidget           *m_listContainer = nullptr;
    QVBoxLayout       *m_listLayout   = nullptr;
};

} // namespace SnapClone::UI
