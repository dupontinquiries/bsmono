#pragma once

// ─── UI/InboxWidget.hpp ───────────────────────────────────────────────────────
//
// Inbox page — displays received snaps and launches the SnapViewerDialog.
//
// Snap structs are stored in m_snaps (keyed by snap ID string) so the viewer
// can access the full media bytes without a storage round-trip.
//
// Unread count:
//   m_unreadCount tracks snaps that have been added but not yet opened.
//   unreadCountChanged() is emitted whenever it changes so the parent window
//   can show a badge on the inbox tab indicator.
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QHash>
#include <QMap>
#include <QSet>
#include <QPointer>
#include <QIcon>     // V32.32: thumbnail cache
#include "core/Types.hpp"
#include "ui/SubTabbed.hpp"  // V32.34+: two-level nav interface

QT_BEGIN_NAMESPACE
class QLabel;
class QListWidget;
class QListWidgetItem;
class QPushButton;
class QVBoxLayout;
class QFrame;
class QTimer;
QT_END_NAMESPACE

// V32.35+: opaque request-mailbox handle from the Rust C FFI
// (snapclone-mailbox::requests_ffi). Forward-declared at global scope to match
// the C linkage in mailbox/snapclone_requests_ffi.h without including it here.
struct ScRequestMailbox;

namespace SnapClone::Storage { class StorageManager; }

namespace SnapClone::UI {

class ConversationDetailDialog;   // V32.39+: tracked open conversation (voice routing)
class IncomingCallDialog;         // V32.40+: incoming voice-call ring

class SegmentedTabStrip;  // V32.34+: mailbox filter tabs
class SmartRibbon;        // V32.34+: bottom tabs+actions ribbon (prototype)
class ExportReminderCard; // V32.34+: monthly export reminder card
class ContactCard;        // V32.35+: reused as a pending-request row (submenu)

class SnapFeedWidget;

class InboxWidget : public QWidget, public SubTabbed
{
    Q_OBJECT

public:
    explicit InboxWidget(QWidget *parent = nullptr);
    ~InboxWidget() override;

    // ── SubTabbed (V32.34+ two-level nav) ────────────────────────────────
    // Inbox sub-tabs are the filter tabs: 0=All, 1=Unread, 2=Sent. These
    // map onto the existing m_convFilter + applyFilterIndex machinery so the
    // unified bar / swipe navigator drive the SAME filter logic as the
    // in-page strips.
    [[nodiscard]] int subTabCount() const override { return 3; }
    [[nodiscard]] int currentSubTab() const override { return static_cast<int>(m_convFilter); }
    void setCurrentSubTab(int index) override { applyFilterIndex(index); }
    [[nodiscard]] QString subTabLabel(int index) const override;
    [[nodiscard]] QString subTabIcon(int index) const override;   // V32.49+: pull-pill icon
    [[nodiscard]] int subTabBadge(int index) const override;

    /// V32.20: identify "self" so addSnap can compute a conversation
    /// key. Must be called once at startup; without it the inbox falls
    /// back to flat list mode (one row per snap, the pre-V32.20 view).
    void setSelfId(const Core::UserId &selfId);

    /// V32.20: provide a storage handle so tapping a conversation row
    /// can drill into the full thread (including outbound snaps that
    /// only live in the DB, not in the in-memory m_snaps map). Optional;
    /// if not provided, conversation rows still render but drill-in
    /// shows only the inbound snaps we have in memory.
    void setStorage(Storage::StorageManager *storage);

    /// V32.20: rebuild the conversation row set from storage. Called
    /// after the controller finishes restoring the inbox at startup,
    /// so historical conversations (including the ones with only
    /// outbound snaps) appear without waiting for new traffic.
    void reloadConversationsFromStorage();

    /// V32.34+ monthly export reminder. The host (MainWindow) decides WHETHER
    /// to show it (MonthlyExportReminder::shouldShowThisMonth() + a recent-
    /// snap threshold) and supplies a few recent media paths to preview. The
    /// inbox just mounts/dismisses the card at the top of its layout.
    /// `exportReminderTriggered` fires when the user taps "Export as zip";
    /// the host runs SnapExporter + the save/share flow. The card self-
    /// dismisses on "Maybe later" (host snoozes via the signal too).
    void showExportReminder(const QStringList &recentMediaPaths);
    void hideExportReminder();
    /// Recent media paths (newest first, up to `max`) for the reminder
    /// preview — pulled from in-memory dedup stubs' storage. Convenience so
    /// the host doesn't re-query.
    [[nodiscard]] QStringList recentMediaPaths(int max) const;

    void addSnap(const Core::Snap &snap);
    void clearSnaps();
    void removeSnap(const Core::SnapId &id);
    /// V32.34+: close any open viewer dialog for this snap. Called when the
    /// snap's timed view window expires (snapViewExpired) so an ephemeral
    /// snap actually closes when its duration elapses, instead of lingering
    /// until the MobileDialog stuck-dialog safety timer auto-rejects it ~4s
    /// later (the field-log symptom). Enforces ephemerality AND removes the
    /// visible close lag on every timed video view. No-op if no viewer is
    /// open for the id.
    void closeViewerForSnap(const Core::SnapId &id);
    [[nodiscard]] int unreadCount() const { return m_unreadCount; }

    // ── V32.35+: inbound connection-request approval (unified mailbox) ───────
    /// A stranger's connection request arrived. Feeds the RequestMailbox model
    /// and refreshes the pending section. `senderId` is the stable id; `name`
    /// is a display label (may equal the id). Idempotent for a sender already
    /// pending (refreshes their row rather than stacking).
    void receiveConnectionRequest(const QString &senderId, const QString &name);
    /// Pending-request count (for the mailbox badge), read from the model.
    [[nodiscard]] int pendingRequestCount() const;

    /// V32.39+: deliver an inbound group-voice packet. Routed to the open
    /// conversation dialog iff it's the one for `sender`; ignored otherwise.
    void onVoicePayloadFromPeer(const SnapClone::Core::UserId &sender,
                                const QByteArray &payload);

    /// V32.42+: posture engine hook. Forward a gated/push-to-talk request to the
    /// open conversation's live call (no-op if none is open or in a call).
    void applyVoiceGated(bool gated);

    /// V32.40+: open the conversation with `peer` already in a voice call — the
    /// entry point used by the Friends page's Call button (so a call can start
    /// without an existing snap thread). No-op if self id isn't set yet; the call
    /// itself no-ops if the voice backend isn't built.
    void startCallWith(const SnapClone::Core::UserId &peer, const QString &displayName);

    /// V32.41+: add a participant to the call on the currently-open conversation,
    /// escalating a 1:1 to a group. If no call is active, starts one with this
    /// person instead. Used by the Friends page's "Add to call" breakout action.
    void addToActiveCall(const SnapClone::Core::UserId &peer, const QString &displayName);
    /// Start a voice call with several people at once — opens the call with the
    /// first picked person, then escalates to the rest via addToActiveCall.
    void startGroupVoiceCall(const QList<SnapClone::Core::UserId> &peers);

    // ── Outbound snap tracking ────────────────────────────────────────────
    // Shown as a compact banner section above the received-snaps list.
    void addOutboundSnap(const Core::SnapId  &id,
                         const Core::UserId  &recipient,
                         const QString       &recipientName,
                         const QDateTime     &sentAt);
    void updateOutboundStatus(const Core::SnapId &id,
                               const QString &phase,
                               const QString &detail);
    void markOutboundDelivered(const Core::SnapId &id);

signals:
    /// Fired whenever the number of unread snaps changes (arrived or opened).
    void unreadCountChanged(int count);

    /// Emitted when the user opens a snap — triggers storage update + expiry timer.
    void snapOpened(SnapClone::Core::SnapId id);

    /// Emitted when the user saves a snap image to disk.
    void snapSaved(QString path);

    void cameraRequested();
    void diagnosticsRequested();

    /// V32.39+: an outbound group-voice packet from the open conversation
    /// (already VoiceWire-enveloped). Host wires this to the network's per-peer
    /// send (NetworkManager::sendToPeer).
    void voicePayloadToSend(SnapClone::Core::UserId recipient, QByteArray payload);
    /// V32.45+: relayed from the open conversation — an ephemeral text snap to send.
    void sendSnapRequested(SnapClone::Core::Snap snap);

    // ── V32.35+: request-approval decisions (AppController acts on these) ────
    /// User accepted a pending request — host should establish the connection /
    /// add the contact.
    void connectionRequestAccepted(QString senderId);
    /// User rejected a pending request (dismissed; sender may re-request).
    void connectionRequestRejected(QString senderId);
    /// User blocked a sender — host should add to its persistent blocklist.
    void senderBlocked(QString senderId);

    /// V32.34+ monthly export reminder: user tapped "Export as zip".
    void exportReminderTriggered();
    /// User tapped "Maybe later" (host snoozes for the month).
    void exportReminderSnoozed();

protected:
    bool eventFilter(QObject *watched, QEvent *event) override;

private slots:
    void onItemDoubleClicked(QListWidgetItem *item);

private:
    void buildUi();
    /// V32.20: open a snap viewer (image or video) for the given snap id.
    /// Extracted from the legacy onItemDoubleClicked path so both the
    /// flat-list mode and the conversation-detail-dialog snapTapped relay
    /// can share the same dedup + viewer-launch logic.
    void openSnapById(const Core::SnapId &snapId);

    /// V32.40+: create + open a conversation thread with `other`, tracking it
    /// for voice routing and wiring its outbound voice packets. If startInCall
    /// is true the thread opens already in-call (used when accepting an incoming
    /// call). Returns the dialog (parented to the window, WA_DeleteOnClose).
    ConversationDetailDialog *openConversationWith(const Core::UserId &other,
                                                   const QString &displayName,
                                                   bool startInCall);

    /// V32.40+: incoming voice-call flow. A voice packet for a peer whose thread
    /// isn't open rings (startIncomingCallRing) instead of being dropped; accept
    /// opens the thread in-call and replays the buffered packets, decline/timeout
    /// drops them.
    void startIncomingCallRing(const Core::UserId &caller);
    void acceptIncomingCall();
    void declineIncomingCall();

    QListWidget               *m_list         = nullptr;
    SnapFeedWidget            *m_snapFeed     = nullptr;  ///< V32.44+: incoming-snap feed (replaces the list)
    QLabel                    *m_emptyLabel   = nullptr;
    QPushButton               *m_cameraButton = nullptr;
    int                        m_unreadCount  = 0;
    qint64                     m_lastOpenMs   = 0; // debounce snap open

    /// V32.34+ mailbox tabs: filter the conversation list. The "Recent
    /// snaps" static label is replaced by a SegmentedTabStrip
    /// (All / Unread / Sent) using the shared in-page tab pattern.
    /// All/Unread filter the received-conversation list; Sent swaps the
    /// list out for the outbound detail section (folding in what the old
    /// header pill + collapsible "Sent" block did). The Sent tab badge
    /// carries the in-flight outbound count.
    enum class ConvFilter { All = 0, Unread = 1, Sent = 2 };
    ConvFilter                 m_convFilter   = ConvFilter::All;
    SegmentedTabStrip         *m_filterTabs   = nullptr;

    // V32.34+ PROTOTYPE: bottom "smart ribbon" (tabs + contextual actions in
    // one row). Opt-in via SC_SMART_RIBBON env / settings flag so it can be
    // device-validated without disturbing the top strip. When active, the top
    // SegmentedTabStrip is hidden and this ribbon drives m_convFilter; both
    // share the same filter enum so the selection logic is identical. This is
    // the prototype for the eventual unified bottom bar that replaces the
    // page-nav TabBar — see docs/TODO.md "smart function ribbon".
    SmartRibbon               *m_ribbon       = nullptr;
    // V32.34+ contextual page action: a slim right-aligned button above the
    // list that surfaces the CURRENT filter's action — "Mark all read" on the
    // Unread filter, "Refresh" on Sent — appearing only when the action is
    // actually available. This replaces the SmartRibbon's per-tab actions for
    // the unified-nav UI (the bar stays nav-only; actions live in page content
    // per the chosen design). Shown regardless of which nav chrome is active.
    QWidget                   *m_actionRow    = nullptr;
    QPushButton               *m_filterAction = nullptr;
    void   refreshContextualAction();     ///< update label/visibility for m_convFilter
    // V32.34+ monthly export reminder card + the root layout we mount it into
    // (kept as a member so showExportReminder can insert it after buildUi).
    QVBoxLayout               *m_rootLayout   = nullptr;
    ExportReminderCard        *m_exportCard   = nullptr;

    // ── V32.35+: pending connection-request section (unified mailbox) ────────
    // Renders the RequestMailbox's pending requests as ContactCards whose
    // expanding submenu offers Accept / Reject / Block. The data + transition
    // rules live in the tested Rust model (snapclone-mailbox::requests) behind
    // the C FFI; this widget is a thin renderer + dispatcher. m_requestMailbox
    // is the opaque FFI handle (owned here; freed in the destructor).
    QWidget                   *m_pendingSection = nullptr;
    QVBoxLayout               *m_pendingLayout  = nullptr;
    QLabel                    *m_pendingHeader  = nullptr;
    ::ScRequestMailbox        *m_requestMailbox = nullptr;
    void buildPendingRequestsSection(QVBoxLayout *root);
    void rebuildPendingRequests();   ///< re-render rows from the FFI model
    /// Apply a menu action (accept/reject/block) to a sender, then refresh.
    void applyRequestAction(const QString &senderId, int action);

    void   applyFilterIndex(int idx);     ///< shared by both strips → m_convFilter
    void   markAllConversationsRead();    ///< Unread-tab action
    void   setFilterBadge(int index, int count); ///< updates both strips
    [[nodiscard]] bool smartRibbonEnabled() const;

    // V32.20: conversation grouping.
    // m_selfId identifies "self" so addSnap can compute conversationKey.
    // m_storage is used at drill-in time to load full thread snaps.
    // m_conversations maps "other party id string" → aggregate row data.
    // List items now represent conversations, not snaps. The Qt::UserRole
    // data on each item is the other-party UserId, NOT a snap id.
    Core::UserId                          m_selfId;
    // V32.39+: the currently-open conversation dialog + its peer, so inbound
    // voice packets can be routed to a live call. QPointer auto-nulls when the
    // WA_DeleteOnClose dialog is destroyed.
    QPointer<ConversationDetailDialog>    m_openConvDialog;
    Core::UserId                          m_openConvPeer;
    // V32.40+: incoming-call ring. When a voice packet arrives for a peer whose
    // thread isn't open we ring (m_ringDialog) rather than drop it; packets that
    // arrive while ringing are buffered (m_ringBuffer) and replayed once the user
    // accepts. One ring at a time; m_ringPeer identifies the caller.
    QPointer<IncomingCallDialog>          m_ringDialog;
    Core::UserId                          m_ringPeer;
    QList<QByteArray>                     m_ringBuffer;
    // V32.40+: peers with a pending missed call (declined or timed-out ring),
    // by id string. Lives outside m_conversations so it survives the snap-driven
    // row rebuilds; cleared when the thread is opened.
    QSet<QString>                         m_missedCalls;
    Storage::StorageManager              *m_storage = nullptr;
    struct ConversationRow {
        Core::UserId  otherParty;
        QString       otherDisplayName;
        QDateTime     lastSnapAt;
        Core::MediaType lastMediaType = Core::MediaType::Image;
        bool          lastIsOutbound = false;
        int           unreadCount    = 0;
        /// V32.32: path to the local thumbnail/preview for the most-
        /// recent PHOTO snap in this conversation. Empty if the latest
        /// snap is a video or no usable file path exists (e.g. snap
        /// already viewed-and-deleted under ephemeral policy).
        ///
        /// We persist the last-photo path even when the conversation's
        /// most-recent snap is a video — so a thumbnail-from-last-photo
        /// still gives a visual cue. This is "best image we have for
        /// this thread," not "thumbnail of the most recent snap."
        QString       lastPhotoPath;
    };
    QMap<QString, ConversationRow>        m_conversations;
    /// Updates a single conversation row in m_conversations from a snap
    /// (called for both inbound addSnap and outbound addOutboundSnap).
    /// Returns the conversation key (other party id) for caller convenience.
    QString updateConversationFromSnap(const Core::Snap &snap);
    /// Rebuilds m_list from m_conversations. Cheap: walks the QMap (sorted
    /// by other-party id) once, sorts to a temp list by lastSnapAt DESC,
    /// emits one QListWidgetItem per row. Avoids the QListWidget churn
    /// of "find item by id and update in place" for the V1 implementation.
    void rebuildConversationList();
    /// V32.44+: refresh the incoming-snap tile feed from m_snaps.
    void refreshSnapFeed();
    /// V32.34+: debounced wrapper. Burst of snap arrivals or storage
    /// callbacks used to trigger a wholesale list rebuild per event
    /// — N snaps in flight = N full rebuilds. requestRebuildSoon()
    /// instead arms a 60ms single-shot that calls rebuildConversationList()
    /// once at the end of the burst. Callers don't need to know about
    /// the indirection; they call requestRebuildSoon() everywhere a
    /// direct rebuild was wanted.
    void requestRebuildSoon();
    QTimer *m_rebuildTimer = nullptr;  ///< V32.34+: debounce
    /// Resolve the display name for an other-party id, falling back to
    /// the username portion of the id if no contact record exists.
    QString resolveDisplayName(const Core::UserId &id) const;
    /// Currently-open viewer dialogs keyed by snap ID. Used to dedupe
    /// open requests: if the user double-taps a snap and a viewer is
    /// already up, we raise/focus the existing one instead of spawning
    /// a second. Without this, two QMediaPlayers race over the same
    /// file and we've seen crashes on Qt's FFmpeg backend when a
    /// sws_context fails on the second player's init.
    /// QPointer so entries auto-null when the viewer is deleted.
    QHash<QString, QPointer<QWidget>> m_openViewers;

    // Full snap structs keyed by snap ID — needed for viewer media bytes.
    QHash<QString, Core::Snap> m_snaps;

    /// V32.32 → V32.34+: thumbnail caching MOVED to ThumbnailService.
    /// The async loader has its own LRU cache shared with other widgets
    /// (FriendListWidget, ContactPickerDialog), so per-widget caches
    /// were doing redundant work. Member retained as a stub for ABI
    /// continuity but is never written; remove on a future binary
    /// break. The new service is in ui/ThumbnailService.hpp.

    // ── Outbound banner ───────────────────────────────────────────────────
    struct OutboundRow {
        Core::SnapId  id;
        Core::UserId  recipient;
        QString       recipientName;
        QDateTime     sentAt;
        bool          delivered    = false;
        QWidget      *card         = nullptr;
        QLabel       *statusLbl    = nullptr;
        QTimer       *expireTimer  = nullptr; // 60s max visibility
    };
    QList<OutboundRow> m_outbound;
    QWidget           *m_outboundSection = nullptr;
    QLabel            *m_outboundSummary = nullptr;
    QVBoxLayout       *m_outboundLayout  = nullptr;
    QWidget           *m_groupRow        = nullptr; // collapsed "N sent · M delivered" row
    QLabel            *m_groupLabel      = nullptr;
    bool               m_grouped         = false;   // true when ≥ kGroupThreshold cards
    bool               m_expanded        = false;   // true when user tapped group row

    /// V32.34+: outbound status now lives in the "Sent" filter tab (the
    /// tab badge carries the in-flight count, and selecting it shows the
    /// outbound detail section). The earlier header-pill affordance
    /// (m_outboundPill + m_outboundDetailShown) is retired — kept here as
    /// nullptr/false so any lingering references compile while the
    /// rebuildOutboundView body is simplified.
    QPushButton       *m_outboundPill    = nullptr;
    bool               m_outboundDetailShown = false;

    static constexpr int kGroupThreshold = 5;       // collapse when ≥ this many rows
    static constexpr int kExpireSecs     = 60;      // max banner visibility per snap
    static constexpr int kDeliveredClearMs = 4000;  // show ✓ for this long then remove

    void buildOutboundSection(QVBoxLayout *root);
    void rebuildOutboundView();
    /// V32.34+: show/hide the conversation list vs the Sent detail section
    /// according to m_convFilter. Sent → outbound section visible, list +
    /// empty-label hidden; All/Unread → list visible, outbound section
    /// hidden. Centralizes the swap so the tab handler and rebuilds agree.
    void applyTabVisibility();
    void removeOutboundRow(const QString &snapId);
    void scheduleExpiry(OutboundRow &r);
};

} // namespace SnapClone::UI
