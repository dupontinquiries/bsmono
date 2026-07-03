// ─── MicroDhtClient.cpp ───────────────────────────────────────────────────────
// See MicroDhtClient.hpp. Async UDP client for the `sc-microdht` test node.
// ─────────────────────────────────────────────────────────────────────────────
#include "network/MicroDhtClient.hpp"

#include <QUdpSocket>
#include <QTimer>
#include <QDateTime>
#include <QLoggingCategory>
#include <QtGlobal>

Q_LOGGING_CATEGORY(lcMicroDht, "snapclone.net.microdht")

namespace SnapClone::Network {

MicroDhtClient::MicroDhtClient(QObject *parent)
    : QObject(parent)
    , m_sock(new QUdpSocket(this))
{
    connect(m_sock, &QUdpSocket::readyRead, this, &MicroDhtClient::onReadyRead);
}

MicroDhtClient::~MicroDhtClient() = default;

bool MicroDhtClient::setNode(const QString &host, quint16 port)
{
    QHostAddress addr;
    if (!addr.setAddress(host)) {
        // We only accept literal IPs (loopback / LAN) — beta testing doesn't
        // do DNS, and a synchronous resolve would block the UI thread.
        qCWarning(lcMicroDht) << "setNode: not a literal IP:" << host;
        return false;
    }
    m_addr = addr;
    m_host = host;
    m_port = port;
    // Bind an ephemeral local socket so we can receive replies. Rebind only if
    // not already bound (state stays across setNode calls).
    if (m_sock->state() == QAbstractSocket::UnconnectedState) {
        if (!m_sock->bind(QHostAddress(QHostAddress::AnyIPv4), 0)) {
            qCWarning(lcMicroDht) << "setNode: failed to bind local UDP socket:"
                                  << m_sock->errorString();
            return false;
        }
    }
    qCInfo(lcMicroDht) << "micro-DHT node set to" << host << ":" << port;
    return true;
}

// ── Requests ──────────────────────────────────────────────────────────────────

void MicroDhtClient::ping()
{
    m_pingSentMs = QDateTime::currentMSecsSinceEpoch();
    sendLine(QByteArrayLiteral("PING"), QStringLiteral("PING"));
}

void MicroDhtClient::put(const QString &scId, const QByteArray &value, int ttlSecs)
{
    if (ttlSecs < 1) ttlSecs = 1;
    QByteArray line = "PUT ";
    line += scId.toLatin1();
    line += ' ';
    line += hexEncode(value);
    line += ' ';
    line += QByteArray::number(ttlSecs);
    sendLine(line, QStringLiteral("PUT"));
}

void MicroDhtClient::get(const QString &scId)
{
    QByteArray line = "GET ";
    line += scId.toLatin1();
    sendLine(line, QStringLiteral("GET"));
}

void MicroDhtClient::stats()
{
    sendLine(QByteArrayLiteral("STATS"), QStringLiteral("STATS"));
}

// ── Send + timeout bookkeeping ──────────────────────────────────────────────

void MicroDhtClient::sendLine(const QByteArray &line, const QString &verb)
{
    if (!hasNode()) {
        qCWarning(lcMicroDht) << "sendLine: no node configured; dropping" << verb;
        emit nodeError(tr("No micro-DHT node configured"));
        return;
    }
    const qint64 written = m_sock->writeDatagram(line, m_addr, m_port);
    if (written < 0) {
        qCWarning(lcMicroDht) << "sendLine: writeDatagram failed:"
                              << m_sock->errorString();
        emit nodeError(m_sock->errorString());
        return;
    }
    armTimeout(verb);
    qCDebug(lcMicroDht) << "->" << line;
}

void MicroDhtClient::armTimeout(const QString &verb)
{
    QTimer *t = m_timers.value(verb, nullptr);
    if (!t) {
        t = new QTimer(this);
        t->setSingleShot(true);
        connect(t, &QTimer::timeout, this, [this, verb]() {
            qCWarning(lcMicroDht) << "request timed out:" << verb;
            emit requestTimedOut(verb);
        });
        m_timers.insert(verb, t);
    }
    t->start(m_timeoutMs);
}

void MicroDhtClient::clearTimeout(const QString &verb)
{
    if (QTimer *t = m_timers.value(verb, nullptr))
        t->stop();
}

// ── Receive + parse ─────────────────────────────────────────────────────────

void MicroDhtClient::onReadyRead()
{
    while (m_sock->hasPendingDatagrams()) {
        QByteArray buf;
        buf.resize(static_cast<int>(m_sock->pendingDatagramSize()));
        QHostAddress from;
        quint16 fromPort = 0;
        const qint64 n = m_sock->readDatagram(buf.data(), buf.size(), &from, &fromPort);
        if (n < 0) continue;
        buf.truncate(static_cast<int>(n));
        const QString line = QString::fromUtf8(buf).trimmed();
        qCDebug(lcMicroDht) << "<-" << line;

        if (line == QLatin1String("PONG")) {
            clearTimeout(QStringLiteral("PING"));
            const qint64 rtt = m_pingSentMs > 0
                ? QDateTime::currentMSecsSinceEpoch() - m_pingSentMs
                : -1;
            emit pong(rtt);
            continue;
        }
        if (line == QLatin1String("OK")) {
            clearTimeout(QStringLiteral("PUT"));
            // The protocol's OK doesn't echo the key; emit with empty scId.
            // Callers that need the key track it themselves (the UI does).
            emit putAcknowledged(QString());
            continue;
        }
        if (line.startsWith(QLatin1String("VALUES"))) {
            clearTimeout(QStringLiteral("GET"));
            const QStringList parts = line.split(QLatin1Char(' '), Qt::SkipEmptyParts);
            // parts[0]=VALUES, parts[1]=key, parts[2..]=hex values
            QString scId = parts.size() >= 2 ? parts.at(1) : QString();
            QList<QByteArray> values;
            for (int i = 2; i < parts.size(); ++i) {
                bool ok = false;
                const QByteArray v = hexDecode(parts.at(i).toLatin1(), &ok);
                if (ok) values.append(v);
            }
            emit valuesReceived(scId, values);
            continue;
        }
        if (line.startsWith(QLatin1String("STATS"))) {
            clearTimeout(QStringLiteral("STATS"));
            emit statsReceived(line);
            continue;
        }
        if (line.startsWith(QLatin1String("ERR"))) {
            // Clear whatever might be waiting — we can't know which verb, so
            // stop them all; the UI treats ERR as a generic failure.
            for (auto it = m_timers.constBegin(); it != m_timers.constEnd(); ++it)
                it.value()->stop();
            const QString msg = line.mid(4).trimmed();
            emit nodeError(msg.isEmpty() ? tr("node error") : msg);
            continue;
        }
        qCWarning(lcMicroDht) << "unrecognized reply:" << line;
    }
}

// ── Hex helpers (mirror rust/snapclone-microdht/src/protocol.rs) ─────────────

QByteArray MicroDhtClient::hexEncode(const QByteArray &bytes)
{
    // Lowercase hex, matching the Rust node's encoder.
    return bytes.toHex();
}

QByteArray MicroDhtClient::hexDecode(const QByteArray &hex, bool *ok)
{
    // QByteArray::fromHex is lenient (skips invalid), so validate first to
    // match the Rust decoder's strictness (even length, all hex digits).
    if (ok) *ok = false;
    if (hex.size() % 2 != 0) return {};
    for (char c : hex) {
        const bool isHex = (c >= '0' && c <= '9')
                        || (c >= 'a' && c <= 'f')
                        || (c >= 'A' && c <= 'F');
        if (!isHex) return {};
    }
    if (ok) *ok = true;
    return QByteArray::fromHex(hex);
}

} // namespace SnapClone::Network
