#pragma once
// ─── ui/GroupVoiceCoordinator.hpp ─────────────────────────────────────────────
//
// V32.39+: the integration seam for a conversation's group-voice feature. It
// owns the four backend objects (GroupRadioSession floor logic, GroupVoiceCapture
// mic, GroupVoiceSender encoder, GroupVoiceReceiver play-out) and wires their
// ~dozen internal signal/slot links ONCE, in one auditable place, instead of
// scattering connect() calls through InboxWidget / the conversation dialog.
//
// It drives the three voice-related view widgets (GroupVoiceBar control,
// GroupVoiceRoster speaker list, ConversationSecurityBar tier banner) and exposes
// only the TRANSPORT seams the network layer must bridge:
//
//   outbound  voicePacketToSend / floorAssertToSend / floorReleaseToSend
//             privateModeRequested   (→ seal+send / broadcast / crypto negotiation)
//   inbound   onVoiceFrameReceived / onPeerFloorAssert / onPeerFloorRelease
//             setSecurityTier        (← channel recv / negotiation result)
//
// The widgets and the network channel never connect to each other directly — the
// coordinator is the only thing that knows both sides, which keeps the views pure
// and the transport replaceable. Everything here is signal/slot glue (no blocking
// work); the actual mic/codec/socket I/O lives in the owned objects and the
// network layer the seams plug into.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QString>
#include <QList>
#include <QByteArray>

#include "core/GroupRadioSession.hpp"        // Core::TalkMode + GroupRadioSession
#include "ui/GroupVoiceRoster.hpp"           // GroupVoiceRoster::Participant
#include "ui/ConversationSecurityBar.hpp"    // ConversationSecurityBar::Tier

namespace SnapClone::Core {
class GroupVoiceCapture;
class GroupVoiceSender;
class GroupVoiceReceiver;
} // namespace SnapClone::Core

namespace SnapClone::UI {

class GroupVoiceBar;

class GroupVoiceCoordinator : public QObject {
    Q_OBJECT
public:
    using Participant = GroupVoiceRoster::Participant;
    using Tier        = ConversationSecurityBar::Tier;

    explicit GroupVoiceCoordinator(QString selfId, QObject *parent = nullptr);
    ~GroupVoiceCoordinator() override;

    /// True iff the voice backend was actually compiled in (needs
    /// SNAPCLONE_RUST_GROUP + SNAPCLONE_RUST_AUDIO; audio transmit/play-out also
    /// needs SNAPCLONE_OPUS_CODEC). When false the coordinator is inert — the
    /// views can still be shown but nothing is captured/encoded — so callers can
    /// hide the call control. Build-time constant.
    [[nodiscard]] bool isAvailable() const;

    /// Attach the views. Any may be null (the coordinator no-ops on a missing
    /// view). Safe to call again to re-point at different widgets; prior view
    /// links are dropped first.
    void setViews(GroupVoiceBar *bar,
                  GroupVoiceRoster *roster,
                  ConversationSecurityBar *securityBar);

    /// The people in this conversation — drives the roster and the id→name
    /// resolution used to label the active talker on the bar.
    void setParticipants(const QList<Participant> &people);

    void setTalkMode(Core::TalkMode mode);

    /// Call lifecycle: start/stop capture + floor session + play-out.
    void joinCall();
    void leaveCall();
    [[nodiscard]] bool inCall() const { return m_inCall; }

    /// Apply a security tier from the negotiation layer (drives BOTH bars). This
    /// is the authoritative state; the optimistic "Securing…" shown when the user
    /// requests private mode is overwritten by whatever the crypto layer reports.
    void setSecurityTier(Tier tier);

public Q_SLOTS:
    // ── inbound transport (the network channel calls these) ──
    void onVoiceFrameReceived(const QString &senderId, const QByteArray &wireFrame);
    void enqueuePreroll(const QString &senderId, const QByteArray &wireFrame);
    void onPeerFloorAssert(const QString &holder, quint64 claimTs);
    void onPeerFloorRelease(const QString &holder);

Q_SIGNALS:
    // ── outbound transport (the network channel connects to these) ──
    void voicePacketToSend(const QByteArray &wireFrame);
    void floorAssertToSend(const QString &me, quint64 claimTs);
    void floorReleaseToSend(const QString &me);
    void privateModeRequested(bool enabled);
    /// Passthrough of GroupVoiceRoster::participantActivated for the app (e.g.
    /// open a profile) — the coordinator doesn't act on it itself.
    void participantActivated(const QString &id);
    void captureError(const QString &what);

private:
    void wireInternal();
    void applyHolder(const QString &holder);
    void requestPrivate(bool on);
    [[nodiscard]] QString nameForId(const QString &id) const;

    QString m_selfId;
    GroupVoiceBar           *m_bar         = nullptr;
    GroupVoiceRoster        *m_roster      = nullptr;
    ConversationSecurityBar *m_securityBar = nullptr;
    Core::GroupRadioSession  *m_session    = nullptr;
    Core::GroupVoiceCapture  *m_capture    = nullptr;
    Core::GroupVoiceSender   *m_sender     = nullptr;
    Core::GroupVoiceReceiver *m_receiver   = nullptr;
    QList<Participant>        m_participants;
    QString                   m_holder;   // current floor holder id ("" = open)
    bool                      m_inCall = false;
};

} // namespace SnapClone::UI
