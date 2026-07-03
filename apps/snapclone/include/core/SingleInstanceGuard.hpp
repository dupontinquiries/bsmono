#pragma once
// ─── SingleInstanceGuard.hpp ──────────────────────────────────────────────────
//
// V32.34+: enforce one running SnapClone process per profile.
//
// WHY this matters (it's correctness, not just battery):
//   • SQLite: two processes on one db → "database is locked" stalls and,
//     with WAL + a crash, possible corruption.
//   • DHT identity: two instances announcing the SAME sc-id with DIFFERENT
//     ports / session keys give peers conflicting "reach me here" records.
//     A sender may hit instance A's port while instance B holds the session
//     key — actively breaking reachability, not merely wasting resources.
//   • Battery / DHT amplification: N nodes = N× announce/re-announce traffic
//     and N× DhtResourceManager wakeups. ("Open it 100 times → blow up
//     networking + battery.")
//
// MECHANISM (two layers, both portable):
//   1. QLockFile — the authoritative "am I the only one?" check. It records
//      the PID + hostname + app name, and (crucially) auto-detects a STALE
//      lock left by a crashed previous instance via PID liveness, so a crash
//      doesn't permanently wedge the user out.
//   2. QLocalServer / QLocalSocket — a tiny IPC channel named per profile.
//      A second launch connects to it, says "raise yourself", and exits.
//      The primary instance, on receiving that, emits raiseRequested() so
//      MainWindow can bring itself forward (best-effort; a no-op on shells
//      that don't support it, but the second process STILL exits cleanly).
//
// SCOPE: keyed by a profile string (default "default"). Two DIFFERENT
// profiles are allowed to coexist (future multi-account-as-separate-process
// option); two instances of the SAME profile are not. See the multi-account
// brief in docs/TODO.md — the long-term plan is switch-in-place rather than
// second-process, but per-profile keying keeps that a policy choice.

#include <QObject>
#include <QString>
#include <memory>

QT_BEGIN_NAMESPACE
class QLockFile;
class QLocalServer;
QT_END_NAMESPACE

namespace SnapClone::Core {

class SingleInstanceGuard : public QObject
{
    Q_OBJECT
public:
    /// `profile` selects the lock/socket namespace (default "default").
    /// `parent` owns lifetime.
    explicit SingleInstanceGuard(QString profile = QStringLiteral("default"),
                                 QObject *parent = nullptr);
    ~SingleInstanceGuard() override;

    /// Try to become the primary instance.
    ///   • Returns true  → we ARE the primary; caller proceeds to start the
    ///                      app. A QLocalServer is now listening for future
    ///                      second-launch "raise" pings.
    ///   • Returns false → another instance already holds the lock. We have
    ///                      already pinged it to raise itself; the caller
    ///                      should exit 0 immediately WITHOUT starting any
    ///                      networking / DB.
    [[nodiscard]] bool tryBecomePrimary();

signals:
    /// Emitted on the primary instance when a second launch pings us to
    /// surface. MainWindow connects this to raise()/activateWindow().
    void raiseRequested();

private:
    QString                       m_profile;
    QString                       m_lockPath;
    QString                       m_socketName;
    std::unique_ptr<QLockFile>    m_lock;
    QLocalServer                 *m_server = nullptr; // parented to this
};

} // namespace SnapClone::Core
