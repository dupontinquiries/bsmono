#pragma once
// ─── core/VoiceWirePacket.hpp ─────────────────────────────────────────────────
//
// V32.39+: the on-wire envelope for group-voice traffic. Voice frames and
// floor-control share the same per-peer byte pipe as snaps/handshakes
// (NetworkManager::sendToPeer / payloadReceived), so they need a 4-byte magic to
// demux at the inbound dispatch — "SVOX", distinct from SHAK/SHK2/SNAP/SCUD/
// SCAP/PTBL. Layout (BigEndian, matching the existing magic peek):
//
//   magic(u32="SVOX") | version(u8) | kind(u8) | kind-specific…
//     VoiceFrame   : … | <opaque sealed frame bytes to end of payload>
//     FloorAssert  : … | claimTs(u64)
//     FloorRelease : …                       (no body)
//
// The floor holder is always the SENDER of the packet, so it isn't embedded —
// the dispatcher already knows senderId from payloadReceived. The voice frame
// itself is opaque here: it's whatever GroupVoiceSender produced (encoded +,
// when private mode is active, AEAD-sealed) — this envelope only routes it.
// ─────────────────────────────────────────────────────────────────────────────

#include <QByteArray>
#include <cstdint>

namespace SnapClone::Core::VoiceWire {

inline constexpr quint32 kMagic   = 0x53564F58; // "SVOX"
inline constexpr quint8  kVersion = 1;

enum class Kind : quint8 { VoiceFrame = 0, FloorAssert = 1, FloorRelease = 2 };

struct Decoded {
    bool       ok      = false;
    Kind       kind    = Kind::VoiceFrame;
    QByteArray frame;            // VoiceFrame: the opaque sealed voice frame
    quint64    claimTs = 0;      // FloorAssert: monotonic claim timestamp
};

/// Cheap 4-byte peek: is this payload a voice-wire packet? Lets the inbound
/// dispatcher skip non-voice traffic without a full decode.
[[nodiscard]] bool isVoicePacket(const QByteArray &payload) noexcept;

[[nodiscard]] QByteArray encodeVoiceFrame(const QByteArray &sealedFrame);
[[nodiscard]] QByteArray encodeFloorAssert(quint64 claimTs);
[[nodiscard]] QByteArray encodeFloorRelease();

/// Decode; `ok` is false on a wrong magic/version, an unknown kind, or a
/// truncated body. A VoiceFrame with a zero-length frame is valid (ok=true).
[[nodiscard]] Decoded decode(const QByteArray &payload);

} // namespace SnapClone::Core::VoiceWire
