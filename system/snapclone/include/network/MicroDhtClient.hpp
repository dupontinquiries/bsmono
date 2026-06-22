#pragma once
// ─── MicroDhtClient.hpp ───────────────────────────────────────────────────────
// Beta-testing shim: a tiny client for the standalone `sc-microdht` node
// (rust/snapclone-microdht). It speaks the same small UDP text protocol so a
// developer can point a dev build at a LOCAL micro-DHT instead of the real
// Jami/OpenDHT network, and exercise publish / lookup flows deterministically.
//
//   PING                          -> PONG
//   PUT <sc-id> <hexvalue> <ttl>  -> OK
//   GET <sc-id>                   -> VALUES <sc-id> <hex> <hex> ...
//   STATS                         -> STATS keys=.. values=.. ..
//
// Values are opaque blobs (the app's encrypted endpoint payloads); the client
// hex-encodes them on the wire exactly like the Rust client.
//
// This is intentionally NOT wire-compatible with OpenDHT — it is a value-layer
// test stand-in. When the "Beta testers → Local micro-DHT" toggle is on, the
// DHT publish/lookup abstraction routes here instead of to JamiAdapter. The
// real feature logic is unchanged; only the transport target differs.
//
// INTEGRATION BOUNDARY (V32.34+): this client + the Beta-testers settings UI
// are complete and tested standalone. The LIVE routing — making DhtDiscovery's
// announceAddress / announcePresence / lookupPeer / publishConnectionRequest
// (src/network/JamiAdapter_dht_impl.cpp) consult beta/micro_dht_enabled and use
// this client instead of OpenDHT — is intentionally deferred: it touches the
// load-bearing real-DHT path, which can't be compiled or exercised in the
// current environment, so wiring it blind was judged too risky. The settings
// keys record intent; connecting them is a small, reviewable follow-up.
//
// All calls are asynchronous (QUdpSocket). Results arrive via signals so the
// caller never blocks the UI thread. A per-request timeout emits requestTimed-
// Out so a dead/absent node doesn't hang a lookup forever.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QByteArray>
#include <QHostAddress>
#include <QList>
#include <QString>
#include <QHash>

class QUdpSocket;
class QTimer;

namespace SnapClone::Network {

class MicroDhtClient : public QObject
{
    Q_OBJECT
public:
    explicit MicroDhtClient(QObject *parent = nullptr);
    ~MicroDhtClient() override;

    /// Point the client at a node. host:port, e.g. "127.0.0.1", 4544.
    /// Safe to call repeatedly; rebinds the target. Returns false if the
    /// host string can't be resolved to an address synchronously (we only
    /// accept literal IPs here — beta testing is loopback/LAN, not DNS).
    bool setNode(const QString &host, quint16 port);

    QString nodeHost() const { return m_host; }
    quint16 nodePort() const { return m_port; }
    bool    hasNode()  const { return !m_addr.isNull() && m_port != 0; }

    /// Per-request timeout in milliseconds (default 1500). A request that gets
    /// no reply in this window emits requestTimedOut.
    void setTimeoutMs(int ms) { m_timeoutMs = ms; }

    // ── Requests (fire-and-forget; results come back as signals) ──
    void ping();
    void put(const QString &scId, const QByteArray &value, int ttlSecs);
    void get(const QString &scId);
    void stats();

signals:
    /// PONG received — node is alive. roundTripMs is best-effort latency.
    void pong(qint64 roundTripMs);
    /// PUT acknowledged ("OK") for the given sc-ID.
    void putAcknowledged(const QString &scId);
    /// GET result for scId: zero or more opaque value blobs (decoded from hex).
    void valuesReceived(const QString &scId, const QList<QByteArray> &values);
    /// Raw STATS line from the node.
    void statsReceived(const QString &statsLine);
    /// The node returned "ERR <msg>" for a request.
    void nodeError(const QString &message);
    /// A request got no reply within the timeout. `verb` is PING/PUT/GET/STATS.
    void requestTimedOut(const QString &verb);

private slots:
    void onReadyRead();

private:
    // Each outstanding request gets a monotonically increasing id so its
    // timeout timer can be matched/cancelled when the reply lands. The text
    // protocol has no request ids, so we match replies by VERB heuristically:
    // for the beta-test workload (one request at a time from the UI) this is
    // sufficient and keeps the wire protocol identical to the Rust node.
    void sendLine(const QByteArray &line, const QString &verb);
    void armTimeout(const QString &verb);
    void clearTimeout(const QString &verb);

    static QByteArray hexEncode(const QByteArray &bytes);
    static QByteArray hexDecode(const QByteArray &hex, bool *ok);

    QUdpSocket *m_sock = nullptr;
    QHostAddress m_addr;
    QString      m_host;
    quint16      m_port = 0;
    int          m_timeoutMs = 1500;
    qint64       m_pingSentMs = 0;

    // One timeout timer per in-flight verb (PING/PUT/GET/STATS). Keyed by verb
    // so a new request of the same verb resets its own timer.
    QHash<QString, QTimer *> m_timers;
};

} // namespace SnapClone::Network
