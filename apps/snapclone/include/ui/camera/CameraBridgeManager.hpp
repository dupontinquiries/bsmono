#pragma once

// CameraBridgeManager — starts the external camera-bridge binary if it isn't
// already running. Singleton owned by GstLaunchBackend.
//
// On postmarketOS, camera-bridge creates virtual-cam-{rear,front,tracking}
// PipeWire nodes. Without it, GstLaunchBackend has nothing to consume.

#include <QObject>
#include <QString>

QT_BEGIN_NAMESPACE
class QProcess;
QT_END_NAMESPACE

namespace SnapClone::UI::Camera {

class CameraBridgeManager : public QObject
{
    Q_OBJECT
public:
    static CameraBridgeManager &instance();

    /// Try to find and spawn camera-bridge. Returns true if a process is
    /// running (either we just spawned one or one was already running).
    /// Idempotent — calling repeatedly is safe.
    bool ensureRunning();

    /// True if our spawned process is alive.
    [[nodiscard]] bool isRunning() const;

    /// Path to the binary we're using (resolved via $PATH + standard locations).
    /// Empty if not found.
    [[nodiscard]] QString resolvedPath() const;

signals:
    /// Bridge process exited (crashed or killed). Caller may want to retry.
    void bridgeExited(int exitCode);

    /// V32.34+: Emitted when the bridge logs its "camera-bridge ready"
    /// line on stdout. Distinct from "process running" — the process
    /// can be running but still mid-initialization (gstreamer init,
    /// libcamera probe, output node spawn). Consumers that need to
    /// wait for actual node publication should connect to this.
    void bridgeReady();

    /// V32.34+: Emitted when the bridge logs a fatal startup error
    /// (no libcamera nodes, missing dependencies, etc). The bridge
    /// process is still running but won't publish anything useful.
    /// Caller should surface to the user and stop retrying start().
    void bridgeFailedToInitialize(const QString &reason);

private:
    CameraBridgeManager();
    ~CameraBridgeManager();
    QString findBinary() const;

    QProcess *m_proc = nullptr;
    QString   m_binaryPath;
    /// V32.34+: latch so we emit bridgeReady() / bridgeFailedToInitialize()
    /// exactly once per process spawn — the relevant log lines may be
    /// emitted as part of a larger chunk and our readyRead handler
    /// could see them multiple times if we don't dedupe.
    bool      m_readinessEmitted = false;
};

} // namespace SnapClone::UI::Camera
