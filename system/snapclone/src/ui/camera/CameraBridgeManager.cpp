#include "ui/camera/CameraBridgeManager.hpp"

#include <QProcess>
#include <QStandardPaths>
#include <QFile>
#include <QFileInfo>
#include <QStringList>
#include <QLoggingCategory>
#include <QProcessEnvironment>
#include <unistd.h>

namespace SnapClone::UI::Camera {

Q_LOGGING_CATEGORY(lcBridge, "snapclone.camera.bridge")

CameraBridgeManager &CameraBridgeManager::instance()
{
    static CameraBridgeManager s;
    return s;
}

CameraBridgeManager::CameraBridgeManager() = default;

CameraBridgeManager::~CameraBridgeManager()
{
    if (m_proc) {
        // Disconnect signals first so a finished-during-shutdown signal
        // doesn't fire into our (half-destroyed) slots, AND so the
        // "QProcess: Destroyed while running" warning doesn't deep-link
        // back into an emit. Then terminate (SIGTERM) and wait. kill
        // (SIGKILL) if that doesn't take quickly.
        m_proc->QObject::disconnect();
        if (m_proc->state() != QProcess::NotRunning) {
            m_proc->terminate();
            if (!m_proc->waitForFinished(500)) {
                m_proc->kill();
                // Give SIGKILL another short window. If even this doesn't
                // finish, fall through — Qt will emit the warning and
                // the OS will reap the orphan when our pid exits.
                m_proc->waitForFinished(200);
            }
        }
        // Explicit delete here (not deleteLater) — we're in a destructor,
        // there's no event loop to deferred-delete to. Parent-owned would
        // also cascade, but doing it explicitly makes the order explicit.
        delete m_proc;
        m_proc = nullptr;
    }
}

QString CameraBridgeManager::findBinary() const
{
    // Search order: $PATH, then known install locations.
    const QStringList candidates = {
        QStandardPaths::findExecutable(QStringLiteral("camera-bridge")),
        qEnvironmentVariable("HOME") + QStringLiteral("/.local/bin/camera-bridge"),
        QStringLiteral("/usr/bin/camera-bridge"),
        QStringLiteral("/usr/local/bin/camera-bridge"),
        QStringLiteral("/app/lib/snapclone/camera-bridge"),  // Flatpak sidecar
        QStringLiteral("/opt/camera-bridge/bin/camera-bridge"),
    };
    qCInfo(lcBridge) << "Searching" << candidates.size() << "candidates for camera-bridge:";
    for (const QString &p : candidates) {
        const bool exists = !p.isEmpty() && QFile::exists(p);
        const bool exec   = exists && QFileInfo(p).isExecutable();
        qCInfo(lcBridge) << "  " << (exec ? "OK  " : exists ? "noexec" : "miss")
                         << (p.isEmpty() ? QStringLiteral("(empty)") : p);
        if (exec) return p;
    }
    return {};
}

QString CameraBridgeManager::resolvedPath() const { return m_binaryPath; }

bool CameraBridgeManager::isRunning() const
{
    return m_proc && m_proc->state() == QProcess::Running;
}

bool CameraBridgeManager::ensureRunning()
{
    if (isRunning()) return true;

    if (m_binaryPath.isEmpty()) m_binaryPath = findBinary();
    if (m_binaryPath.isEmpty()) {
        qCWarning(lcBridge) << "camera-bridge binary not found";
        return false;
    }

    // Tear down old process if it died
    if (m_proc) {
        m_proc->deleteLater();
        m_proc = nullptr;
    }
    // V32.34+: reset the readiness latch so the new process can emit
    // bridgeReady / bridgeFailedToInitialize fresh. Without this,
    // a respawn after a previous "ready" emission would never re-emit.
    m_readinessEmitted = false;

    m_proc = new QProcess(this);
    m_proc->setProcessChannelMode(QProcess::MergedChannels);

    QProcessEnvironment env = QProcessEnvironment::systemEnvironment();
    if (!env.contains(QStringLiteral("PIPEWIRE_RUNTIME_DIR"))) {
        env.insert(QStringLiteral("PIPEWIRE_RUNTIME_DIR"),
                   QStringLiteral("/run/user/") + QString::number(::getuid()));
    }
    m_proc->setProcessEnvironment(env);

    connect(m_proc, &QProcess::readyRead, this, [this]() {
        const QByteArray out = m_proc->readAll();
        if (out.isEmpty()) return;
        qCDebug(lcBridge) << "bridge:" << out.trimmed();

        // V32.34+: parse for readiness / fatal-error markers so callers
        // can stop polling/retrying once we have a definitive answer.
        // The bridge prints these as well-defined log lines:
        //   "camera-bridge ready"            → fully initialised
        //   "No hw camera nodes found"       → libcamera couldn't enumerate
        //   "no libcamera nodes found"       → variant of the same
        //   "ERROR" lines                    → log::error! from the bridge
        if (!m_readinessEmitted) {
            const QString text = QString::fromUtf8(out);
            if (text.contains(QLatin1String("camera-bridge ready"))) {
                m_readinessEmitted = true;
                qCInfo(lcBridge) << "Bridge reports READY";
                emit bridgeReady();
            } else if (text.contains(QLatin1String("No hw camera nodes found"))
                       || text.contains(QLatin1String("no libcamera nodes found"))
                       || text.contains(QLatin1String("Install: apk add gst-plugin-libcamera"))) {
                m_readinessEmitted = true;
                qCWarning(lcBridge) << "Bridge reports NO CAMERAS";
                emit bridgeFailedToInitialize(
                    tr("camera-bridge couldn't find any cameras via libcamera. "
                       "Install gst-plugin-libcamera (apk add gst-plugin-libcamera) "
                       "and verify with `cam --list`."));
            }
        }
    });
    connect(m_proc,
            QOverload<int, QProcess::ExitStatus>::of(&QProcess::finished),
            this, [this](int code, QProcess::ExitStatus) {
        qCWarning(lcBridge) << "camera-bridge exited:" << code;
        emit bridgeExited(code);
    });

    qCInfo(lcBridge) << "Starting camera-bridge:" << m_binaryPath;
    m_proc->start(m_binaryPath, {});
    return m_proc->waitForStarted(2000);
}

} // namespace SnapClone::UI::Camera
