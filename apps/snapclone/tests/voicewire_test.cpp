#include "core/VoiceWirePacket.hpp"
#include <cstdio>
namespace VW = SnapClone::Core::VoiceWire;
static int fails = 0;
#define CHECK(c,m) do{ if(!(c)){ std::printf("FAIL: %s\n", m); ++fails;} else std::printf("ok:   %s\n", m);}while(0)
int main(){
    // VoiceFrame round-trip (incl. a frame that itself starts with bytes resembling a header)
    QByteArray frame("\x01\x02\x53\x56hello-opus-frame", 18);
    QByteArray p = VW::encodeVoiceFrame(frame);
    CHECK(VW::isVoicePacket(p), "voice frame packet recognized by isVoicePacket");
    auto d = VW::decode(p);
    CHECK(d.ok && d.kind==VW::Kind::VoiceFrame && d.frame==frame, "voice frame decodes to identical bytes");

    // empty frame is valid
    auto de = VW::decode(VW::encodeVoiceFrame(QByteArray()));
    CHECK(de.ok && de.frame.isEmpty(), "empty voice frame is valid");

    // FloorAssert round-trip
    auto da = VW::decode(VW::encodeFloorAssert(0x0123456789ABCDEFULL));
    CHECK(da.ok && da.kind==VW::Kind::FloorAssert && da.claimTs==0x0123456789ABCDEFULL, "floor assert claimTs round-trips");

    // FloorRelease round-trip
    auto dr = VW::decode(VW::encodeFloorRelease());
    CHECK(dr.ok && dr.kind==VW::Kind::FloorRelease, "floor release decodes");

    // Non-voice payloads rejected (SNAP magic + short + random)
    QByteArray snap("\x53\x4E\x41\x50....", 8);  // "SNAP"
    CHECK(!VW::isVoicePacket(snap), "SNAP payload not mistaken for voice");
    CHECK(!VW::decode(snap).ok, "SNAP payload fails decode");
    CHECK(!VW::isVoicePacket(QByteArray("ab")), "2-byte payload not voice");

    // Truncated floor-assert (header but no u64)
    QByteArray badAssert = VW::encodeFloorAssert(5).left(8);
    CHECK(!VW::decode(badAssert).ok, "truncated floor-assert fails decode");

    std::printf(fails? "\n%d FAILED\n":"\nALL CHECKS PASSED\n", fails);
    return fails?1:0;
}
