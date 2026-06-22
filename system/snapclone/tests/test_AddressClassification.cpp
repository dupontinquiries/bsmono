// test_AddressClassification.cpp — locks down the LAN self-address ranking that
// determines which interface a peer advertises for LAN-direct connections.
//
// Regression target (device logs 2026-05-28): on the FP5, QNetworkInterface::
// allAddresses() lists usb0 (172.16.42.1, the USB-tether) before wlan0
// (192.168.2.x). A naive "first non-loopback IPv4" pick advertised the
// unreachable tether IP, so two phones on the same WiFi each tried to reach the
// other at 172.16.42.1, no session ever opened, and every snap queued forever.
// AddressClass::pickBestLanAddress must rank the real WiFi subnet above the
// tether so the advertised address is reachable.

#include <QtTest>
#include "network/AddressClassification.hpp"

using namespace SnapClone::Network;

class TestAddressClassification : public QObject
{
    Q_OBJECT
private slots:
    void wlanBeatsUsbTether();
    void ranking_192_gt_10_gt_172_gt_169();
    void emptyAndJunk();
    void realFp5CandidateOrder();
    void tenNetIsLan();
};

void TestAddressClassification::wlanBeatsUsbTether()
{
    // The exact FP5 case: tether enumerated first, wlan second.
    const QStringList candidates{ "172.16.42.1", "192.168.2.37" };
    QCOMPARE(AddressClass::pickBestLanAddress(candidates),
             QStringLiteral("192.168.2.37"));
    // Order must not matter (tie-break is by rank, not position).
    const QStringList reversed{ "192.168.2.37", "172.16.42.1" };
    QCOMPARE(AddressClass::pickBestLanAddress(reversed),
             QStringLiteral("192.168.2.37"));
}

void TestAddressClassification::ranking_192_gt_10_gt_172_gt_169()
{
    QVERIFY(AddressClass::lanAdvertiseRank("192.168.1.5")
            > AddressClass::lanAdvertiseRank("10.0.0.5"));
    QVERIFY(AddressClass::lanAdvertiseRank("10.0.0.5")
            > AddressClass::lanAdvertiseRank("172.16.42.1"));
    QVERIFY(AddressClass::lanAdvertiseRank("172.16.42.1")
            > AddressClass::lanAdvertiseRank("169.254.1.1"));
    QVERIFY(AddressClass::lanAdvertiseRank("169.254.1.1") > 0);
}

void TestAddressClassification::emptyAndJunk()
{
    QVERIFY(AddressClass::pickBestLanAddress({}).isEmpty());
    // A public/CGNAT address scores 0 → not chosen as a LAN-direct address.
    QCOMPARE(AddressClass::lanAdvertiseRank("8.8.8.8"), 0);
    QCOMPARE(AddressClass::lanAdvertiseRank("100.64.0.1"), 0); // CGNAT
    QCOMPARE(AddressClass::lanAdvertiseRank("not-an-ip"), 0);
    QCOMPARE(AddressClass::lanAdvertiseRank("127.0.0.1"), 0);  // loopback
}

void TestAddressClassification::realFp5CandidateOrder()
{
    // Mirror what allAddresses() returns on the FP5 (tether + wlan + v6 + loop).
    // pickBestLanAddress only sees the IPv4 strings the caller filters in, but
    // verify it still picks wlan when extra ranked-0 entries are present.
    const QStringList candidates{
        "172.16.42.1",   // usb0 tether
        "192.168.2.37",  // wlan0  ← must win
        "169.254.55.1",  // link-local
    };
    QCOMPARE(AddressClass::pickBestLanAddress(candidates),
             QStringLiteral("192.168.2.37"));
}

void TestAddressClassification::tenNetIsLan()
{
    // Some routers hand out 10.x; it must still be preferred over the tether.
    const QStringList candidates{ "172.16.42.1", "10.12.0.4" };
    QCOMPARE(AddressClass::pickBestLanAddress(candidates),
             QStringLiteral("10.12.0.4"));
}

QTEST_MAIN(TestAddressClassification)
#include "test_AddressClassification.moc"
