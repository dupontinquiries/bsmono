#pragma once
// ─── ui/VoiceNetworkBridge.hpp ────────────────────────────────────────────────
//
// V32.39+: adapts a GroupVoiceCoordinator's transport seams to the per-peer byte
// pipe (NetworkManager::sendToPeer / payloadReceived). Outbound, it wraps the
// coordinator's voice frames + floor-control in the VoiceWire envelope and emits
// one sendPayload() per conversation peer. Inbound, onPayloadFromPeer() decodes a
// voice packet and routes it back into the coordinator, returning whether it
// consumed the payload so the caller's other dispatch can be skipped.
//
// It deliberately depends only on the coordinator + the envelope codec, NOT on
// NetworkManager — the owner connects sendPayload → sendToPeer and feeds
// payloadReceived → onPayloadFromPeer. That keeps this unit pure glue and
// testable, and lets the same bridge sit under either a transient dialog or a
// persistent per-conversation voice session.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QList>
#include <QByteArray>

#include "core/Types.hpp"   // Core::UserId

namespace SnapClone::UI {

class GroupVoiceCoordinator;

class VoiceNetworkBridge : public QObject {
    Q_OBJECT
public:
    /// `peers` are the recipients of this conversation's outbound voice (the
    /// other party for 1:1; all members for a group).
    explicit VoiceNetworkBridge(GroupVoiceCoordinator *coordinator,
                                QList<Core::UserId> peers,
                                QObject *parent = nullptr);

    void setPeers(QList<Core::UserId> peers);

public Q_SLOTS:
    /// Feed an inbound network payload. Returns true iff it was a voice-wire
    /// packet (then routed into the coordinator); false leaves it for the
    /// caller's normal dispatch. Malformed-but-ours packets are consumed
    /// (return true) and dropped.
    bool onPayloadFromPeer(const Core::UserId &sender, const QByteArray &payload);

Q_SIGNALS:
    /// One per peer for each outbound voice/floor packet. Wire to
    /// NetworkManager::sendToPeer.
    void sendPayload(SnapClone::Core::UserId recipient, QByteArray payload);

private:
    GroupVoiceCoordinator *m_coord = nullptr;
    QList<Core::UserId>    m_peers;
};

} // namespace SnapClone::UI
