#pragma once

// ─── UI/ConversationDetailDialog.hpp ─────────────────────────────────────────
//
// Drill-down view of a single 1:1 conversation thread. Shows every snap
// exchanged with one other party, oldest at top, newest at bottom — the
// classic chat-transcript convention even though snaps are ephemeral
// media not persistent text.
//
// Visual model:
//   • Inbound (received from other party):   LEFT-aligned card, with
//     muted-accent background and the sender name as a small caption.
//   • Outbound (sent by self): RIGHT-aligned card, with accent
//     background. Delivery status (sent / delivered) shown as a small
//     "✓" or "✓✓" beneath.
//
// Lifecycle:
//   • Constructed with an immutable snapshot of the thread's snaps —
//     the list does not auto-update. If new snaps arrive while the
//     detail dialog is open, the user closes-and-reopens to refresh.
//     (Keeps the dialog simple; live-updating is a v2 feature.)
//
// Why "presentation-only":
//   • The dialog does NOT open viewers directly. Instead it emits
//     snapTapped(SnapId), and the parent InboxWidget handles viewer
//     launch using its existing dedup map. This means one consistent
//     "is this viewer open" answer across both the conversation list
//     and the detail view — no race where the user taps a snap from
//     the inbox list while the same snap is open from a detail
//     dialog, spawning a second viewer.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/MobileDialog.hpp"
#include "core/Types.hpp"
#include <QList>

QT_BEGIN_NAMESPACE
class QScrollArea;
class QVBoxLayout;
class QFrame;       // V32.26: header drag-target
class QEvent;       // V32.26: handleHeaderDragEvent param type
class QShowEvent;   // V32.39+: voice subsystem lifecycle
class QHideEvent;
class QPushButton;
class QLineEdit;  // V32.39+: call toggle
QT_END_NAMESPACE

namespace SnapClone::Core { class VoiceNoteRecorder; }   // V32.46+: voice notes

namespace SnapClone::UI {

// V32.39+: group-voice surface widgets + their wiring coordinator.
class ConversationSecurityBar;
class GroupVoiceRoster;
class GroupVoiceBar;
class GroupVoiceCoordinator;
class VoiceNetworkBridge;

class ConversationDetailDialog : public MobileDialog
{
    Q_OBJECT

public:
    ConversationDetailDialog(const Core::UserId &selfId,
                             const Core::UserId &otherParty,
                             const QString      &otherDisplayName,
                             const QList<Core::Snap> &snaps,
                             QWidget *parent = nullptr);

    /// V32.41+: a participant of a (group) voice call — peer id + display name.
    /// A plain type plus const queries, so they live in public: (moc rejects
    /// non-function declarations inside a slots/signals section).
    struct CallParticipant { Core::UserId id; QString name; };

    /// True if `id` is one of this call's other participants. The owner uses this
    /// to route inbound voice to the right open dialog — covers the 1:1 other
    /// party and every added group member.
    [[nodiscard]] bool isCallPeer(const Core::UserId &id) const;
    /// True while a voice call is live on this dialog.
    [[nodiscard]] bool isCallActive() const;

signals:
    /// User tapped a snap in the thread. Parent InboxWidget handles
    /// the actual viewer launch — see header comment for rationale.
    void snapTapped(SnapClone::Core::SnapId snapId);

    /// V32.39+: an outbound group-voice packet (already VoiceWire-enveloped) for
    /// `recipient`. The owner (InboxWidget/AppController) connects this to the
    /// network's per-peer send. Emitted only while a call is active.
    void sendVoicePayload(SnapClone::Core::UserId recipient, QByteArray payload);

    /// V32.43+: user tapped the bottom-left "+" to pull more people into the
    /// live call. The owner (InboxWidget) opens the people picker and calls
    /// addCallParticipant() back on this dialog for each one chosen.
    void addPeopleRequested();

    /// V32.45+: the user composed an ephemeral text message. Carries a fully
    /// built Text snap (self→peer, expiry already stamped); the owner relays it
    /// to AppController::sendSnap for transmission + storage.
    void sendSnapRequested(SnapClone::Core::Snap snap);

public slots:
    /// V32.42+: posture engine hook. Flip the live call's talk mode between
    /// push-to-talk and gated (voice-activated). No-op unless a call is active —
    /// this is where the engine's "covered → gated" request is actually enforced.
    void setCallGated(bool gated);

    /// V32.39+: feed an inbound network payload addressed to this conversation.
    /// Voice packets are routed into the live call; anything else is ignored, so
    /// it's safe to forward all of a peer's payloads here.
    void deliverNetworkPayload(const SnapClone::Core::UserId &sender,
                               const QByteArray &payload);
    /// V32.49: feed ring-window audio (captured before accept) as ordered
    /// pre-roll, so an accepted call plays the head of the message.
    void primeVoicePreroll(const SnapClone::Core::UserId &sender,
                           const QList<QByteArray> &payloads);

    /// V32.40+: begin the voice call programmatically — used when accepting an
    /// incoming call so the thread opens already in-call. No-op if the voice
    /// backend isn't built or a call is already active.
    void startVoiceCall();

    /// V32.41+: group voice. The participants are self (added automatically) +
    /// the "others". A 1:1 thread defaults to just the thread's other party;
    /// setting or adding more turns it into a group call. Safe before OR during a
    /// call — the coordinator (roster) + network bridge are re-applied at once, so
    /// a peer added mid-call starts receiving this side's voice immediately.
    void setCallParticipants(const QList<CallParticipant> &others);
    void addCallParticipant(const Core::UserId &id, const QString &name);

protected:
    /// V32.26: intercept mouse press/move/release on the header bar
    /// to implement drag-down-to-close. Delegates to base for anything
    /// else (e.g. MobileDialog's scrim-tap-to-dismiss).
    bool eventFilter(QObject *watched, QEvent *event) override;

    /// V32.39+: end any active voice call when the conversation leaves the
    /// screen. The call is started/stopped EXPLICITLY via the header Call
    /// button (see setVoiceCallActive), so the mic is never opened merely by
    /// opening a thread.
    void hideEvent(QHideEvent *e) override;

private:
    void buildUi();
    /// V32.39+: start/stop the voice call explicitly. Joins/leaves via the
    /// coordinator and shows/hides the roster + push-to-talk bar; the security
    /// banner stays visible regardless. Reflects state on the header button.
    void setVoiceCallActive(bool on);
    void appendSnapCard(const Core::Snap &snap);
    /// (Re)apply m_participants to the coordinator roster (self + others) and the
    /// network bridge peers (the others). Idempotent; called at build and on any
    /// participant change.
    void applyParticipantsToVoice();

    /// V32.26: drag-down-to-close gesture handler.
    /// Returns true if the event was consumed (drag in progress); false
    /// to let the dialog process the event normally (e.g. taps elsewhere).
    /// Called from eventFilter on the header bar.
    bool handleHeaderDragEvent(QEvent *e);

    Core::UserId       m_selfId;
    Core::UserId       m_otherParty;
    QString            m_otherDisplayName;
    QList<Core::Snap>  m_snaps;
    /// V32.41+: the OTHER call participants (self is implicit). Empty until
    /// buildUi seeds it with the 1:1 other party; grows for a group call.
    QList<CallParticipant> m_participants;
    QScrollArea       *m_scroll = nullptr;
    QLineEdit         *m_composer = nullptr;  ///< V32.45+: ephemeral text input
    Core::VoiceNoteRecorder *m_recorder = nullptr;  ///< V32.46+: voice-note capture
    QVBoxLayout       *m_threadLayout = nullptr;
    QFrame            *m_header = nullptr;     // V32.26: drag-target widget

    // V32.39+: group-voice surface. The security bar sits under the header; the
    // roster + push-to-talk bar sit beneath the thread. m_voiceCoord owns the
    // backend objects and wires them to these three views (see
    // GroupVoiceCoordinator). The call is started/stopped via the header Call
    // button (setVoiceCallActive) and force-stopped on hideEvent.
    ConversationSecurityBar *m_securityBar = nullptr;
    GroupVoiceRoster        *m_voiceRoster = nullptr;
    GroupVoiceBar           *m_voiceBar    = nullptr;
    QPushButton             *m_addPeopleButton = nullptr;  ///< bottom-left mid-call add
    GroupVoiceCoordinator   *m_voiceCoord  = nullptr;
    VoiceNetworkBridge      *m_voiceBridge = nullptr;
    QPushButton             *m_callBtn     = nullptr;  // header call toggle

    // V32.26: drag-to-close state. Tracked while the user is mid-drag
    // on the header. m_dragActive flips to true on mouse-press at a
    // non-button location inside the header, and back to false on
    // release. m_dragStartY captures the press Y in global coords; we
    // compare against move events to decide if the gesture exceeded
    // the threshold for closing.
    bool               m_dragActive = false;
    int                m_dragStartY = 0;
    // V32.26: V1 deliberately omits live rubber-band sheet-follows-finger
    // feedback. That would fight MobileDialog::repositionSheet() which
    // owns the sheet's geometry on the inline path. User sees the
    // dialog vanish on threshold-cross instead. Promotable to
    // MobileDialog with live rubber-band as a v2.
};

} // namespace SnapClone::UI
