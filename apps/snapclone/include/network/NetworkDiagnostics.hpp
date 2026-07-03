#pragma once

// ─── Network/NetworkDiagnostics.hpp ──────────────────────────────────────────
//
// Runs four async connectivity checks and streams results via signals.
//
// Checks:
//   1. UDP multicast loopback  — gossip peer discovery
//   2. TCP listen              — inbound WebSocket connections
//   3. WebSocket loopback      — local firewall on loopback interface
//   4. Public IP discovery     — NAT/internet reachability + local IP list
//
// Race-condition safety:
//   Each check uses a std::shared_ptr<bool> "done" flag so that competing
//   callbacks (e.g. readyRead arriving just as the timeout fires) cannot both
//   decrement m_pending, which would corrupt the counter and cause
//   diagnosisReady() to fire multiple times or not at all.
//
// Usage:
//   auto *diag = new NetworkDiagnostics(9876, 9877, this);
//   connect(diag, &NetworkDiagnostics::diagnosisReady, this, &MyClass::onResults);
//   diag->runAll();
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QList>
#include <memory>

namespace SnapClone::Network {

struct DiagnosticResult {
    enum class Status { Ok, Warning, Fail };

    QString name;
    Status  status  = Status::Fail;
    QString detail;      ///< human-readable description of the outcome
    QString suggestion;  ///< actionable fix commands for the user
};

class NetworkDiagnostics : public QObject
{
    Q_OBJECT

public:
    explicit NetworkDiagnostics(quint16 wsPort     = 9876,
                                quint16 gossipPort  = 9877,
                                QObject *parent     = nullptr);

    /// Start all checks concurrently.  checkCompleted() fires for each one;
    /// diagnosisReady() fires once all four are done.
    void runAll();

signals:
    /// Fired once per check as it completes (useful for streaming UI updates).
    void checkCompleted(SnapClone::Network::DiagnosticResult result);

    /// Fired once all checks are done with the complete result list.
    void diagnosisReady(QList<SnapClone::Network::DiagnosticResult> results);

private:
    /// Called exactly once per check to record the result and decrement m_pending.
    void onCheckDone(DiagnosticResult result);

    void checkUdpMulticast();
    void checkUdpStun();
    void checkTcpListen();
    void checkLoopbackWebSocket();
    void checkPublicAddress();

    quint16                 m_wsPort;
    quint16                 m_gossipPort;
    QList<DiagnosticResult> m_results;
    int                     m_pending = 0;
};

} // namespace SnapClone::Network

Q_DECLARE_METATYPE(SnapClone::Network::DiagnosticResult)
