#include "ui/VoiceNetworkBridge.hpp"

#include "ui/GroupVoiceCoordinator.hpp"
#include "core/VoiceWirePacket.hpp"

#include <utility>

namespace SnapClone::UI {

namespace VW = SnapClone::Core::VoiceWire;

VoiceNetworkBridge::VoiceNetworkBridge(GroupVoiceCoordinator *coordinator,
                                       QList<Core::UserId> peers,
                                       QObject *parent)
    : QObject(parent)
    , m_coord(coordinator)
    , m_peers(std::move(peers))
{
    if (!m_coord) return;

    // ── outbound: coordinator seam → enveloped payload → each peer ──
    connect(m_coord, &GroupVoiceCoordinator::voicePacketToSend, this,
            [this](const QByteArray &frame) {
        const QByteArray pkt = VW::encodeVoiceFrame(frame);
        for (const auto &p : m_peers) emit sendPayload(p, pkt);
    });
    connect(m_coord, &GroupVoiceCoordinator::floorAssertToSend, this,
            [this](const QString & /*me*/, quint64 claimTs) {
        const QByteArray pkt = VW::encodeFloorAssert(claimTs);
        for (const auto &p : m_peers) emit sendPayload(p, pkt);
    });
    connect(m_coord, &GroupVoiceCoordinator::floorReleaseToSend, this,
            [this](const QString & /*me*/) {
        const QByteArray pkt = VW::encodeFloorRelease();
        for (const auto &p : m_peers) emit sendPayload(p, pkt);
    });
}

void VoiceNetworkBridge::setPeers(QList<Core::UserId> peers)
{
    m_peers = std::move(peers);
}

bool VoiceNetworkBridge::onPayloadFromPeer(const Core::UserId &sender,
                                           const QByteArray &payload)
{
    if (!VW::isVoicePacket(payload)) return false;   // not ours — leave it
    if (!m_coord) return true;
    const VW::Decoded d = VW::decode(payload);
    if (!d.ok) return true;                           // ours but malformed → drop
    switch (d.kind) {
    case VW::Kind::VoiceFrame:
        m_coord->onVoiceFrameReceived(sender.value, d.frame);
        break;
    case VW::Kind::FloorAssert:
        // The asserting peer IS the sender (holder isn't embedded in the wire).
        m_coord->onPeerFloorAssert(sender.value, d.claimTs);
        break;
    case VW::Kind::FloorRelease:
        m_coord->onPeerFloorRelease(sender.value);
        break;
    }
    return true;
}

void VoiceNetworkBridge::flushPreroll(const Core::UserId &sender,
                                      const QList<QByteArray> &payloads)
{
    if (!m_coord) return;
    for (const QByteArray &payload : payloads) {
        if (!VW::isVoicePacket(payload)) continue;
        const VW::Decoded d = VW::decode(payload);
        if (!d.ok || d.kind != VW::Kind::VoiceFrame) continue;
        m_coord->enqueuePreroll(sender.value, d.frame);
    }
}

} // namespace SnapClone::UI
