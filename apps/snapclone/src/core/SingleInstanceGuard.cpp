#include "core/SingleInstanceGuard.hpp"

#include <QLockFile>
#include <QLocalServer>
#include <QLocalSocket>
#include <QStandardPaths>
#include <QDir>
#include <QCryptographicHash>
#include <QLoggingCategory>

namespace SnapClone::Core {

Q_LOGGING_CATEGORY(lcInst, "snapclone.instance")

namespace {
// Derive a stable, filesystem/socket-safe token for a profile name.
// Local-socket names have platform length + character constraints
// (especially on Windows named pipes, but also tidy on Unix), so we hash
// the profile rather than embedding arbitrary user text.
QString profileToken(const QString &profile)
{
    const QByteArray h =
        QCryptographicHash::hash(profile.toUtf8(), QCryptographicHash::Sha256);
    return QString::fromLatin1(h.toHex().left(16)); // 64-bit token is plenty
}
} // namespace

SingleInstanceGuard::SingleInstanceGuard(QString profile, QObject *parent)
    : QObject(parent)
    , m_profile(std::move(profile))
{
    const QString token = profileToken(m_profile);
    // Lock file lives under the app data dir so it's per-user and survives
    // in a predictable place. We do NOT put it in /tmp: a per-user data
    // path keeps two different OS users from colliding, and matches where
    // the rest of our state lives.
    const QString base =
        QStandardPaths::writableLocation(QStandardPaths::AppDataLocation);
    QDir().mkpath(base);
    m_lockPath   = base + QStringLiteral("/.instance-") + token + QStringLiteral(".lock");
    // Local socket name: include the token. QLocalServer namespaces this
    // per-user on Unix (abstract/filesystem socket under the user runtime
    // dir), so two OS users won't clash.
    m_socketName = QStringLiteral("snapclone-") + token;
}

SingleInstanceGuard::~SingleInstanceGuard()
{
    if (m_server) {
        m_server->close();
    }
    // QLockFile unlocks on destruction (its dtor calls unlock()).
}

bool SingleInstanceGuard::tryBecomePrimary()
{
    m_lock = std::make_unique<QLockFile>(m_lockPath);
    // If a previous instance crashed without unlocking, QLockFile detects
    // the holder PID is dead after this interval and reclaims the lock.
    // 30s is comfortably longer than any legitimate startup hiccup while
    // still recovering quickly from a real crash.
    m_lock->setStaleLockTime(30'000);

    // Non-blocking attempt. tryLock(0) returns immediately.
    if (m_lock->tryLock(0)) {
        // We are primary. Stand up the local server so a future second
        // launch can ping us to raise the window.
        m_server = new QLocalServer(this);
        // removeServer clears a stale socket file left by a crashed primary
        // (the lock's stale detection handled the lock; the socket file can
        // linger separately on Unix).
        QLocalServer::removeServer(m_socketName);
        if (!m_server->listen(m_socketName)) {
            // Not fatal: we still hold the lock and are the only instance.
            // We just won't get raise pings. Log and continue.
            qCWarning(lcInst) << "primary: could not listen on local socket"
                              << m_socketName << "-" << m_server->errorString()
                              << "(raise-on-relaunch disabled)";
        } else {
            connect(m_server, &QLocalServer::newConnection, this, [this]() {
                // Drain any pending connections. The payload is irrelevant
                // for now (just a ping); future: forward a file/URL the
                // second launch was opened with.
                while (auto *conn = m_server->nextPendingConnection()) {
                    conn->disconnectFromServer();
                    conn->deleteLater();
                    qCInfo(lcInst) << "primary: second launch detected — "
                                      "requesting window raise";
                    emit raiseRequested();
                }
            });
        }
        qCInfo(lcInst) << "primary instance for profile" << m_profile
                       << "(lock" << m_lockPath << ")";
        return true;
    }

    // Lock is held by a live instance. Determine who, ping it to raise, and
    // tell the caller to exit.
    qint64 holderPid = 0;
    QString holderHost, holderApp;
    if (m_lock->getLockInfo(&holderPid, &holderHost, &holderApp)) {
        qCInfo(lcInst) << "another instance is running for profile" << m_profile
                       << "- pid" << holderPid << "host" << holderHost
                       << "- pinging it to raise, then exiting";
    } else {
        qCInfo(lcInst) << "another instance holds the lock for profile"
                       << m_profile << "- exiting";
    }

    // Best-effort raise ping. If it fails, we still exit (the primary is
    // alive per the lock; we simply couldn't reach its socket).
    QLocalSocket sock;
    sock.connectToServer(m_socketName);
    if (sock.waitForConnected(500)) {
        sock.write("raise");
        sock.flush();
        sock.waitForBytesWritten(200);
        sock.disconnectFromServer();
    } else {
        qCInfo(lcInst) << "could not reach primary's socket to request raise"
                       << "(it may be mid-startup); exiting anyway";
    }

    // Release our (failed) lock handle explicitly; we're about to exit.
    m_lock.reset();
    return false;
}

} // namespace SnapClone::Core
