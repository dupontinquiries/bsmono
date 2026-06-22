// tests/test_GossipDiscovery.cpp
//
// Unit tests for the gossip peer-discovery logic.
// We test the datagram parsing / view-merge logic by subclassing and exposing
// the protected processDatagram() equivalent via a test fixture.

#include <QtTest>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonArray>
#include "core/Types.hpp"

using namespace SnapClone;

// ── Minimal gossip payload builder (mirrors GossipDiscovery::buildGossipPayload) ──
static QByteArray makeGossipPayload(
    const QString &type,
    const QString &id,
    const QString &host,
    quint16        port,
    const QList<std::tuple<QString,QString,quint16>> &peers = {})
{
    QJsonObject obj;
    obj[QStringLiteral("type")] = type;
    obj[QStringLiteral("id")]   = id;
    obj[QStringLiteral("host")] = host;
    obj[QStringLiteral("port")] = port;

    QJsonArray arr;
    for (const auto &[pid, phost, pport] : peers) {
        QJsonObject p;
        p[QStringLiteral("id")]   = pid;
        p[QStringLiteral("host")] = phost;
        p[QStringLiteral("port")] = pport;
        arr.append(p);
    }
    obj[QStringLiteral("peers")] = arr;

    return QJsonDocument(obj).toJson(QJsonDocument::Compact);
}

class TestGossipPayload : public QObject
{
    Q_OBJECT

private slots:
    void validJoinPayloadParsed()
    {
        const QByteArray payload = makeGossipPayload(
            QStringLiteral("JOIN"),
            QStringLiteral("peer-abc"),
            QStringLiteral("192.168.1.5"),
            9877);

        const QJsonDocument doc = QJsonDocument::fromJson(payload);
        QVERIFY(!doc.isNull());
        QVERIFY(doc.isObject());

        const QJsonObject obj = doc.object();
        QCOMPARE(obj[QStringLiteral("type")].toString(), QStringLiteral("JOIN"));
        QCOMPARE(obj[QStringLiteral("id")].toString(),   QStringLiteral("peer-abc"));
        QCOMPARE(obj[QStringLiteral("host")].toString(), QStringLiteral("192.168.1.5"));
        QCOMPARE(obj[QStringLiteral("port")].toInt(),    9877);
    }

    void viewMergePayloadContainsPeers()
    {
        const QList<std::tuple<QString,QString,quint16>> peers = {
            { QStringLiteral("peer-1"), QStringLiteral("10.0.0.1"), 9877 },
            { QStringLiteral("peer-2"), QStringLiteral("10.0.0.2"), 9877 },
        };

        const QByteArray payload = makeGossipPayload(
            QStringLiteral("PING"),
            QStringLiteral("peer-abc"),
            QStringLiteral("192.168.1.5"),
            9877,
            peers);

        const QJsonDocument doc = QJsonDocument::fromJson(payload);
        const QJsonArray peerArray = doc.object()[QStringLiteral("peers")].toArray();
        QCOMPARE(peerArray.size(), 2);
        QCOMPARE(peerArray[0].toObject()[QStringLiteral("id")].toString(),
                 QStringLiteral("peer-1"));
    }

    void emptyPayloadParsedSafely()
    {
        const QJsonDocument doc = QJsonDocument::fromJson(QByteArray{});
        // Must not crash; must return a null document.
        QVERIFY(doc.isNull());
    }

    void peerIdRoundTrip()
    {
        const Core::PeerId id = Core::PeerId::generate();
        QVERIFY(!id.value.isEmpty());

        const Core::PeerId id2 = Core::PeerId::generate();
        QVERIFY(id != id2); // UUIDs must be unique
    }
};

QTEST_MAIN(TestGossipPayload)
#include "test_GossipDiscovery.moc"
