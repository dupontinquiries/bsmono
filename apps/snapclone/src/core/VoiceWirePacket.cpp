#include "core/VoiceWirePacket.hpp"

#include <QDataStream>
#include <QIODevice>

namespace SnapClone::Core::VoiceWire {

namespace {
// magic(4) + version(1) + kind(1)
constexpr int kHeaderBytes = 6;

QByteArray makeHeader(Kind kind)
{
    QByteArray out;
    QDataStream ds(&out, QIODevice::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);
    ds << kMagic << kVersion << static_cast<quint8>(kind);
    return out;
}
} // namespace

bool isVoicePacket(const QByteArray &payload) noexcept
{
    if (payload.size() < 4) return false;
    QDataStream ds(payload);
    ds.setByteOrder(QDataStream::BigEndian);
    quint32 m = 0;
    ds >> m;
    return m == kMagic;
}

QByteArray encodeVoiceFrame(const QByteArray &sealedFrame)
{
    QByteArray out = makeHeader(Kind::VoiceFrame);
    out.append(sealedFrame);   // remainder of the payload IS the frame
    return out;
}

QByteArray encodeFloorAssert(quint64 claimTs)
{
    QByteArray out;
    QDataStream ds(&out, QIODevice::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);
    ds << kMagic << kVersion << static_cast<quint8>(Kind::FloorAssert) << claimTs;
    return out;
}

QByteArray encodeFloorRelease()
{
    return makeHeader(Kind::FloorRelease);
}

Decoded decode(const QByteArray &payload)
{
    Decoded d;
    if (payload.size() < kHeaderBytes) return d;
    QDataStream ds(payload);
    ds.setByteOrder(QDataStream::BigEndian);
    quint32 m = 0;
    quint8 ver = 0, k = 0;
    ds >> m >> ver >> k;
    if (m != kMagic || ver != kVersion) return d;
    if (k > static_cast<quint8>(Kind::FloorRelease)) return d;

    d.kind = static_cast<Kind>(k);
    switch (d.kind) {
    case Kind::VoiceFrame:
        d.frame = payload.mid(kHeaderBytes);   // may be empty (valid)
        break;
    case Kind::FloorAssert:
        if (payload.size() < kHeaderBytes + 8) return d;  // truncated
        ds >> d.claimTs;
        break;
    case Kind::FloorRelease:
        break;
    }
    d.ok = true;
    return d;
}

} // namespace SnapClone::Core::VoiceWire
