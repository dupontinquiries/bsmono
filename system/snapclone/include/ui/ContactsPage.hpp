#pragma once
// ─── UI/ContactsPage.hpp ──────────────────────────────────────────────────────
// Full-page address book with card-per-contact layout, avatar initials,
// online badge, nickname editing via double-tap, and vCard import/export.
// Replaces the basic FriendListWidget list for the Friends tab.
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QHash>
#include <QPointer>
#include <QEvent>
#include <QDateTime>
#include "core/Types.hpp"
#include "ui/SubTabbed.hpp"  // V32.34+: two-level nav interface

QT_BEGIN_NAMESPACE
class QScrollArea;
class QVBoxLayout;
class QLabel;
class QLineEdit;
class QPushButton;
class QTimer;
class QComboBox;
class QTimer;
class QDialog;
QT_END_NAMESPACE

namespace SnapClone::Core { class AppController; }

namespace SnapClone::UI {

class SegmentedTabStrip;  // V32.34+: contacts filter tabs
class ContactCard;        // V32.34+: expandable contact card w/ swipe-out submenu

/// A peer visible on the LAN via UDP multicast beacon.
struct NearbyPeer {
    Core::UserId id;
    QString      displayName;
    qint64       lastSeenMs = 0; ///< QDateTime::currentMSecsSinceEpoch()
};

class ContactsPage : public QWidget, public SubTabbed
{
    Q_OBJECT
public:
    explicit ContactsPage(QWidget *parent = nullptr);

    // ── SubTabbed (V32.34+ two-level nav) ────────────────────────────────
    // Contacts sub-tabs: 0=All, 1=Online, 2=Pending. Map onto the existing
    // m_cardFilter via applyCardFilterIndex so the unified bar / swipe nav
    // drive the same filtering as the in-page strip.
    [[nodiscard]] int subTabCount() const override { return 3; }
    [[nodiscard]] int currentSubTab() const override { return static_cast<int>(m_cardFilter); }
    void setCurrentSubTab(int index) override { applyCardFilterIndex(index); }
    [[nodiscard]] QString subTabLabel(int index) const override;
    [[nodiscard]] int subTabBadge(int index) const override;

    /// V32.31: optional AppController pointer. Used by features that
    /// need self-id + lastKnownLocation (landmark creation) and the
    /// storage manager. Non-owning. Safe to omit on test instances —
    /// features that need it will no-op.
    void setController(SnapClone::Core::AppController *c) { m_controller = c; }

    void setContacts(const QList<Core::Contact> &contacts);
    void setNearbyPeers(const QStringList &scIds); // populate nearby peers dropdown
    // Beacon panel integration
    void onNearbyPeerSeen(const Core::UserId &id, const QString &displayName);
    void pruneStaleNearbyPeers(); // called from beacon timer
    void setBeaconActive(bool active);
    void addOrUpdateContact(const Core::Contact &contact);
    void markOnline(const Core::UserId &id, bool online);
    void markHasPendingSnap(const Core::UserId &id, bool pending);
    /// V32.34+: per-contact outbound queue count. Shows a compact "⬆ N"
    /// pill on the contact card (or a ✓ briefly when the last one delivers),
    /// so the Contacts page is the at-a-glance overview of what's going out
    /// to whom. count==0 clears the pill. This is the per-contact analog of
    /// the inbox header pill; sending no longer needs its own top-level
    /// surface. Wired from MainWindow on addOutboundSnap / snapDelivered.
    void setOutboundQueued(const Core::UserId &id, int count);

    /// V32.34+: re-pull per-contact conversation activity from storage and
    /// rebuild the cards. Call after a snap is received/sent/expired so the
    /// "last snap / N new" line on each contact card stays current. Cheap
    /// (one storage query) but rebuilds the card list, so MainWindow should
    /// call it on actual snap events, not per-frame.
    void refreshConversationActivityAndRebuild();
    // phase: "connecting"|"punching"|"mailbox"|"relookup"|"udp"|"connected"|"failed"|""
    void setConnectionStatus(const Core::UserId &id,
                             const QString &phase, const QString &detail);
    // Update the per-card network stat line (transport + address).
    // The label fades in over 400 ms. A shared 3 s timer refreshes all labels.
    void setNetworkStat(const Core::UserId &id, const QString &stat);
    void setLastLocation(const Core::UserId &id, const Core::PeerLocation &loc);
    void clearNetworkStat(const Core::UserId &id);
    void removeContact(const Core::UserId &id);

    void setSelectedContact(const Core::UserId &id); ///< programmatic selection
    [[nodiscard]] std::optional<Core::UserId> selectedContact() const;
    [[nodiscard]] QString selectedContactName() const;

signals:
    void contactSelected(SnapClone::Core::UserId id);
    void contactEdited(SnapClone::Core::Contact contact);
    void contactDeleted(SnapClone::Core::UserId id);
    void addFriendRequested(QString host, quint16 port);
    void importVCardRequested();
    void nearbyPeerConnectRequested(QString scId); // user tapped a nearby peer
    void beaconToggleRequested(bool start);
    void qrScanRequested(); // user tapped the beacon toggle
    /// User tapped "Share my location" on a specific contact card.
    /// Argument is the recipient. MainWindow handler calls
    /// AppController::sendLocationSnap. Privacy gate (Settings
    /// "Share location" toggle) is checked at the handler.
    void locationShareRequested(SnapClone::Core::UserId recipient);
    /// V32.40+: user tapped "call" on a contact card. MainWindow opens the
    /// conversation with this contact already in-call (InboxWidget::startCallWith),
    /// so a call can be started without first having a snap thread. Only emitted
    /// in voice builds (the button is compiled in only then).
    void callRequested(SnapClone::Core::UserId id, QString displayName);
    /// V32.41+: user chose "Add to call" on a contact card's breakout submenu.
    /// MainWindow adds them to the active call (escalating 1:1 → group), or
    /// starts a call with them if none is active. Only emitted in voice builds.
    void addToCallRequested(SnapClone::Core::UserId id, QString displayName);
    /// V32.34+: emitted when a sub-tab's count badge changes (online / pending
    /// peer counts), so the UnifiedNavBar can repaint its count pills live. The
    /// bar reads the actual counts from subTabBadge(); this is just the nudge.
    void badgesChanged();

protected:
    bool eventFilter(QObject *obj, QEvent *ev) override;
    /// V32.34+: collapse any expanded ContactCard when the page is navigated
    /// away from, so it doesn't linger open behind the scenes / on return.
    void hideEvent(QHideEvent *e) override;

private slots:
    void onAddClicked();
    void onImportVCard();
    void onFilterChanged(const QString &text);

private:
    void buildUi();
    void rebuildCards();
    void applyCardFilterIndex(int idx);  ///< V32.34+: shared by strip + SubTabbed
    QWidget *makeCard(const Core::Contact &c);
    void openEditor(const QString &id);
    void restoreStatLabel(const QString &key);
    void restoreLocHint(const QString &key);

    QScrollArea  *m_scroll       = nullptr;
    QWidget      *m_cardContainer = nullptr;
    QVBoxLayout  *m_cardLayout   = nullptr;
    // V32.34+: the currently-expanded ContactCard (single-open-at-a-time). When
    // a card expands it emits expanded(self); we collapse the previous one.
    // QPointer so it auto-nulls if rebuildCards() deletes the card while it was
    // the tracked-open one — no dangling deref.
    QPointer<SnapClone::UI::ContactCard> m_openCard;
    QLabel       *m_emptyLabel   = nullptr;
    QLineEdit    *m_filterEdit   = nullptr;
    QComboBox    *m_nearbyCombo  = nullptr; // nearby peers from DHT cache
    QLineEdit    *m_hostEdit     = nullptr;
    QLineEdit    *m_portEdit     = nullptr;
    QPushButton  *m_addBtn       = nullptr;

    // V32.34+: prevent stacked-editor dialogs. The previous design
    // allowed multiple ContactEditorDialog instances to spawn from
    // rapid Edit-button taps — the comment in createCard claimed
    // "redundant B is a non-issue because A is on top of the modal
    // stack" but on Phosh/postmarketOS modal stacking is flaky and
    // the user sees a confusing pile of identical-looking sheets.
    // Now openEditor checks this pointer; if it points to a live
    // dialog, we raise it instead of creating a new one.
    QPointer<QDialog>  m_openEditor;

    // Nearby (LAN beacon) peers — visible but not yet in contacts
    QHash<QString, NearbyPeer>     m_nearbyPeers;    // keyed by stable ID
    // Beacon panel UI
    QWidget                       *m_beaconPanel    = nullptr; // collapsible container
    QWidget                       *m_beaconContent  = nullptr; // expanded content
    QPushButton                   *m_beaconToggle   = nullptr; // expand/collapse btn
    QPushButton                   *m_beaconStartBtn = nullptr; // start/stop beacon
    QLabel                        *m_beaconStatus   = nullptr;
    QTimer                        *m_beaconPruneTimer = nullptr;
    bool                           m_beaconExpanded  = false;
    bool                           m_beaconActive    = false;

    QHash<QString, Core::Contact>  m_contacts;       // keyed by stable ID
    QHash<QString, QTimer *>       m_spinnerTimers;  // active spinner timers per peer
    QHash<QString, QString>        m_peerStats;
    QHash<QString, Core::PeerLocation> m_peerLocations;      // latest stat string per peer

    /// V32.34+ contacts depth: per-contact conversation activity, so a card
    /// reads as a conversation summary (last snap time + unread count), not
    /// just a presence row. Populated from StorageManager::loadConversations
    /// in setContacts(); read in makeCard(). Lightweight local struct keeps
    /// the heavy storage header out of this UI header.
    struct ConvActivity {
        QDateTime lastSnapAt;
        int       unreadCount = 0;
        bool      valid       = false;
    };
    QHash<QString, ConvActivity> m_convActivity;
    void refreshConversationActivity();
    /// V32.34+: debounce timer for refreshConversationActivityAndRebuild so a
    /// snap storm (e.g. catching up after offline) coalesces into one rebuild
    /// instead of one-per-snap. Created lazily on first use.
    QTimer *m_activityRefreshTimer = nullptr;
    QTimer                        *m_statTimer = nullptr; // 3 s refresh for stat labels
    QString                        m_selectedId;
    QString                        m_filterText;

    /// V32.34+ contacts tabs: filter the card list using the shared in-page
    /// tab pattern (SegmentedTabStrip), matching tabbed Settings + the
    /// mailbox All/Unread tabs. All shows every bucket; Online shows only
    /// connected peers; Pending shows contacts still connecting / not yet
    /// seen. The text search (m_filterEdit) composes WITH the active tab.
    enum class CardFilter { All = 0, Online = 1, Pending = 2 };
    CardFilter                     m_cardFilter = CardFilter::All;
    SegmentedTabStrip             *m_filterTabs = nullptr;

    /// V32.32: skeleton-card placeholders shown during first-render before
    /// storage has handed us the contact list. Distinct from m_emptyLabel
    /// which is the "you have no contacts" message — that one is correct
    /// only AFTER we know the list is genuinely empty (not just unloaded).
    /// Without this distinction the page renders "No contacts yet" for
    /// ~100-300ms at startup before the storage load completes, which
    /// reads as "your data is gone."
    bool                           m_initialLoadDone = false;
    QList<QWidget *>               m_skeletonCards;

    void toggleBeaconPanel();
    QWidget *makeNearbyCard(const NearbyPeer &np);

    /// V32.32: build a single skeleton-card widget — grey rounded rectangle
    /// sized like a real contact card. Used during first-render before
    /// `setContacts()` has been called. No animation in V1 (a shimmer
    /// effect would be nice but adds a per-skeleton QPropertyAnimation
    /// that runs even after the cards are gone if we forget to stop
    /// them — keep it simple). Caller owns the returned widget.
    QWidget *makeSkeletonCard();

    /// V32.31: opens the AddLandmarkDialog. Reads self id and last-known
    /// location via m_controller; falls back to NaN coords (dialog will
    /// show its own "set location first" hint). On save, persists via
    /// m_controller->storageManager()->saveLandmark().
    void onAddLandmarkClicked();

    /// V32.31: non-owning AppController pointer. Provides self id, last
    /// known location, and storage access for landmark creation. Set by
    /// MainWindow via setController(). May be null in test builds.
    SnapClone::Core::AppController *m_controller = nullptr;
};

} // namespace SnapClone::UI
