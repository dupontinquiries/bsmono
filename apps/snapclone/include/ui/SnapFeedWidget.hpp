#pragma once

// V32.44+: the inbox's "received" surface, reimagined as a live tiled feed of
// incoming snaps instead of a conversation list. Each tile is SEALED — it shows
// who sent the snap and when, never the contents. Opening (tapping) is what
// reveals and then wipes the snap, so the view-once contract is preserved. New
// snaps push in as tiles; viewing one removes it (the owner re-supplies the set
// via setEntries after each change).

#include <QFrame>
#include <QWidget>
#include <QList>
#include <QString>
#include <QDateTime>
#include <QPixmap>

#include "core/Types.hpp"

class QLabel;
class QGridLayout;
class QScrollArea;
class QTimer;

namespace SnapClone::UI {

/// One sealed tile in the incoming-snap feed. Emits clicked(id) on tap.
class SnapTile : public QFrame
{
    Q_OBJECT
public:
    SnapTile(const SnapClone::Core::SnapId &id,
             const QString                 &senderName,
             const QDateTime               &sentAt,
             bool                           isVideo,
             bool                           isAudio,
             QWidget                       *parent = nullptr);

    /// Supply a thumbnail (loaded async by the feed). It's rendered SEALED —
    /// frosted behind the card chrome — so the feed reads visually without
    /// giving away the snap. A null/absent thumbnail leaves the avatar-only look.
    void setThumbnail(const QPixmap &pm);

signals:
    void clicked(SnapClone::Core::SnapId id);
    /// V32.48+: long-press (press-and-hold) — the feed uses this to open the
    /// sender's conversation thread instead of viewing the single snap.
    void held(SnapClone::Core::SnapId id);

protected:
    void mousePressEvent(QMouseEvent *e) override;
    void mouseReleaseEvent(QMouseEvent *e) override;
    void paintEvent(QPaintEvent *e) override;

private:
    SnapClone::Core::SnapId m_id;
    QPixmap                 m_frosted;   ///< pre-blurred thumbnail source (small)
    QTimer                 *m_holdTimer = nullptr;
    bool                    m_held       = false;
};

/// A live, tiled feed of incoming (unopened) snaps. Two across, newest first.
/// Tapping a tile asks the owner to open the snap in the ephemeral viewer.
class SnapFeedWidget : public QWidget
{
    Q_OBJECT
public:
    explicit SnapFeedWidget(QWidget *parent = nullptr);

    struct Entry {
        SnapClone::Core::SnapId id;
        QString                 senderName;
        QDateTime               sentAt;
        bool                    isVideo = false;
        QString                 localPath;   ///< thumbnail source; empty = sealed avatar only
        bool                    isAudio = false;  /// V32.46+: voice note
    };

    /// Replace the whole feed. Caller passes incoming, unopened snaps; we render
    /// in the order given (the inbox supplies them newest-first).
    void setEntries(const QList<Entry> &entries);

    [[nodiscard]] int count() const { return static_cast<int>(m_entries.size()); }

signals:
    void snapTapped(SnapClone::Core::SnapId id);
    void snapHeld(SnapClone::Core::SnapId id);   // V32.48+: open the sender's thread

private:
    void rebuild();

    QScrollArea          *m_scroll = nullptr;
    QWidget              *m_grid   = nullptr;
    QGridLayout          *m_layout = nullptr;
    QLabel               *m_empty  = nullptr;
    QList<Entry>          m_entries;

    static constexpr int  kColumns = 2;
};

} // namespace SnapClone::UI
