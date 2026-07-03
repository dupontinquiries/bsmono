#include "network/FirewallPortManager.hpp"

#include <QDBusConnection>
#include <QDBusConnectionInterface>   // required for isServiceRegistered() — not pulled in by QDBusConnection alone
#include <QDBusInterface>
#include <QDBusReply>
#include <QDBusMessage>
#include <QProcess>
#include <QThread>
#include <QStandardPaths>             // for findExecutable() in pkexec detection
#include <QLoggingCategory>
#include <QCoreApplication>
#ifdef Q_OS_UNIX
#include <unistd.h>
#endif

Q_LOGGING_CATEGORY(lcFirewall, "snapclone.firewall")

namespace SnapClone::Network {

// ─── D-Bus service names ──────────────────────────────────────────────────────

static constexpr const char *kFirewalldService  = "org.fedoraproject.FirewallD1";
static constexpr const char *kFirewalldPath     = "/org/fedoraproject/FirewallD1";
static constexpr const char *kFirewalldIface    = "org.fedoraproject.FirewallD1";
static constexpr const char *kFirewalldZoneIface= "org.fedoraproject.FirewallD1.zone";

static constexpr const char *kUfwService        = "com.ubuntu.USF1";
static constexpr const char *kUfwPath           = "/com/ubuntu/USF1";
static constexpr const char *kUfwIface          = "com.ubuntu.USF1";

static constexpr const char *kPortalService     = "org.freedesktop.portal.Desktop";
static constexpr const char *kPortalPath        = "/org/freedesktop/portal/desktop";
static constexpr const char *kPortalFirewallIface = "org.freedesktop.portal.Firewall";

// ─── Construction ─────────────────────────────────────────────────────────────

FirewallPortManager::FirewallPortManager(QObject *parent)
    : QObject(parent)
{
    m_backend = detectBackend();
    emit backendDetected(m_backend, backendName(m_backend));
    qCInfo(lcFirewall) << "Firewall backend:" << backendName(m_backend);
}

// ─── Backend detection ────────────────────────────────────────────────────────

FirewallBackend FirewallPortManager::detectBackend() const
{
    const QDBusConnection sysBus = QDBusConnection::systemBus();

    // isServiceRegistered() requires QDBusConnectionInterface (not just QDBusConnection).
    // The interface() pointer may be null if the D-Bus daemon is not running (e.g. in
    // a minimal container) — guard before dereferencing.
    if (QDBusConnectionInterface *iface = sysBus.interface()) {
        // 1. firewalld (Fedora / RHEL / openSUSE / Arch)
        if (iface->isServiceRegistered(QLatin1String(kFirewalldService))) {
            return FirewallBackend::Firewalld;
        }
        // 2. UFW via D-Bus (Ubuntu 24.04+)
        if (iface->isServiceRegistered(QLatin1String(kUfwService))) {
            return FirewallBackend::UfwDbus;
        }
    } else {
        qCWarning(lcFirewall) << "D-Bus system bus not available; skipping firewalld/UFW detection.";
    }

    // 3. XDG Desktop Portal Firewall interface (Flatpak sandbox)
    {
        QDBusInterface portal(QLatin1String(kPortalService),
                              QLatin1String(kPortalPath),
                              QLatin1String(kPortalFirewallIface),
                              QDBusConnection::sessionBus());
        if (portal.isValid()) {
            return FirewallBackend::XdgPortal;
        }
    }

    // 4. firewall-cmd CLI — firewalld without D-Bus (containers, minimal installs)
    if (!QStandardPaths::findExecutable(QStringLiteral("firewall-cmd")).isEmpty()) {
        return FirewallBackend::FirewalldCli;
    }

    // 5. UFW CLI (Ubuntu/Debian/Alpine: ufw installed but no D-Bus service)
    if (!QStandardPaths::findExecutable(QStringLiteral("ufw")).isEmpty()) {
        return FirewallBackend::UfwCli;
    }

    // 6. iptables/ip6tables direct — works when running as root (containers, Alpine)
    if (runningAsRoot() &&
        (!QStandardPaths::findExecutable(QStringLiteral("iptables")).isEmpty() ||
         !QStandardPaths::findExecutable(QStringLiteral("iptables-legacy")).isEmpty())) {
        return FirewallBackend::IptablesDirect;
    }

    // 7. nftables CLI (Debian 10+, Fedora 32+, Arch default)
    if (!QStandardPaths::findExecutable(QStringLiteral("nft")).isEmpty()) {
        return FirewallBackend::Nftables;
    }

    // 8. pkexec + iptables — prompts Polkit auth (last resort on desktop)
    if (!QStandardPaths::findExecutable(QStringLiteral("pkexec")).isEmpty()) {
        return FirewallBackend::Pkexec;
    }

    return FirewallBackend::None;
}

QString FirewallPortManager::backendName(FirewallBackend b)
{
    switch (b) {
    case FirewallBackend::Firewalld:      return QStringLiteral("firewalld (D-Bus)");
    case FirewallBackend::FirewalldCli:   return QStringLiteral("firewalld (CLI)");
    case FirewallBackend::UfwDbus:        return QStringLiteral("UFW (D-Bus)");
    case FirewallBackend::UfwCli:         return QStringLiteral("UFW (CLI)");
    case FirewallBackend::Nftables:       return QStringLiteral("nftables (CLI)");
    case FirewallBackend::IptablesDirect: return QStringLiteral("iptables (direct/root)");
    case FirewallBackend::XdgPortal:      return QStringLiteral("XDG Desktop Portal");
    case FirewallBackend::Pkexec:         return QStringLiteral("iptables (pkexec/Polkit)");
    case FirewallBackend::None:           return QStringLiteral("none — open ports manually");
    }
    return QStringLiteral("unknown");
}

// ─── Public API ───────────────────────────────────────────────────────────────

void FirewallPortManager::requestPorts(const QList<PortRequest> &ports)
{
    if (ports.isEmpty()) {
        emit allPortsProcessed(true);
        return;
    }

    m_pending = static_cast<int>(ports.size());
    m_allOk   = true;

    for (const auto &req : ports) {
        switch (m_backend) {
        case FirewallBackend::Firewalld:      requestViaFirewalld(req);      break;
        case FirewallBackend::FirewalldCli:   requestViaFirewalldCli(req);   break;
        case FirewallBackend::UfwDbus:        requestViaUfw(req);            break;
        case FirewallBackend::UfwCli:         requestViaUfwCli(req);         break;
        case FirewallBackend::Nftables:       requestViaNftables(req);       break;
        case FirewallBackend::IptablesDirect: requestViaIptablesDirect(req); break;
        case FirewallBackend::XdgPortal:      requestViaXdgPortal(req);      break;
        case FirewallBackend::Pkexec:         requestViaPkexec(req);         break;
        case FirewallBackend::None:
            // No backend found — report failure immediately, no external process spawned.
            onPortResult(PortResult{
                req,
                false,
                QStringLiteral("No firewall backend available. "
                               "Open port %1/%2 manually (see 🔍 Network Diagnostics).")
                    .arg(req.port).arg(req.protocol),
                FirewallBackend::None
            });
            break;
        }
    }
}

// ─── firewalld ────────────────────────────────────────────────────────────────
//
// firewalld D-Bus API:
//   org.fedoraproject.FirewallD1.zone.addPort(zone, port, protocol, timeout)
//   timeout=0 means permanent (until next reload).

void FirewallPortManager::requestViaFirewalld(const PortRequest &req)
{
    QDBusInterface iface(QLatin1String(kFirewalldService),
                         QLatin1String(kFirewalldPath),
                         QLatin1String(kFirewalldZoneIface),
                         QDBusConnection::systemBus());

    if (!iface.isValid()) {
        qCWarning(lcFirewall) << "firewalld D-Bus interface not available";
        onPortResult({ req, false, QStringLiteral("firewalld not reachable"), FirewallBackend::Firewalld });
        return;
    }

    // First: get the default zone.
    QDBusInterface mgr(QLatin1String(kFirewalldService),
                       QLatin1String(kFirewalldPath),
                       QLatin1String(kFirewalldIface),
                       QDBusConnection::systemBus());
    const QDBusReply<QString> zoneReply = mgr.call(QStringLiteral("getDefaultZone"));
    const QString zone = zoneReply.isValid() ? zoneReply.value() : QStringLiteral("public");

    // AddPort(zone, portStr, protocol, timeout=0)
    const QString portStr = QString::number(req.port);
    const QDBusReply<QString> reply = iface.call(
        QStringLiteral("addPort"), zone, portStr, req.protocol, 0);

    PortResult result;
    result.request = req;
    result.backend = FirewallBackend::Firewalld;

    if (reply.isValid()) {
        result.success = true;
        result.message = QStringLiteral("Opened %1/%2 in firewalld zone '%3'.")
                             .arg(portStr, req.protocol, zone);
        qCInfo(lcFirewall) << result.message;

        // Persist to permanent config so the rule survives reboots.
        // runtimeToPermanent() copies all runtime rules to the permanent store —
        // simpler and more correct than trying to call addPort on the permanent
        // config zone object (which requires a different D-Bus object path).
        QDBusInterface mgr2(QLatin1String(kFirewalldService),
                            QLatin1String(kFirewalldPath),
                            QLatin1String(kFirewalldIface),
                            QDBusConnection::systemBus());
        const QDBusReply<void> permReply = mgr2.call(QStringLiteral("runtimeToPermanent"));
        if (!permReply.isValid()) {
            // Non-fatal — the runtime rule is still active until next reboot.
            qCDebug(lcFirewall) << "runtimeToPermanent unavailable (non-fatal):"
                                << permReply.error().message();
        }
    } else {
        // Port may already be open — treat that as success.
        const QString err = reply.error().message();
        if (err.contains(QLatin1String("ALREADY_ENABLED"), Qt::CaseInsensitive)) {
            result.success = true;
            result.message = QStringLiteral("Port %1/%2 already open in firewalld.")
                                 .arg(portStr, req.protocol);
        } else {
            result.success = false;
            result.message = QStringLiteral("firewalld AddPort failed: %1").arg(err);
            qCWarning(lcFirewall) << result.message;
        }
    }

    onPortResult(result);
}

// ─── UFW D-Bus ────────────────────────────────────────────────────────────────

void FirewallPortManager::requestViaUfw(const PortRequest &req)
{
    QDBusInterface iface(QLatin1String(kUfwService),
                         QLatin1String(kUfwPath),
                         QLatin1String(kUfwIface),
                         QDBusConnection::systemBus());

    PortResult result;
    result.request = req;
    result.backend = FirewallBackend::UfwDbus;

    if (!iface.isValid()) {
        result.success = false;
        result.message = QStringLiteral("UFW D-Bus interface not available");
        onPortResult(result);
        return;
    }

    const QString portProto = QStringLiteral("%1/%2").arg(req.port).arg(req.protocol);
    const QDBusReply<bool> reply = iface.call(QStringLiteral("AllowPort"), portProto);

    if (reply.isValid() && reply.value()) {
        result.success = true;
        result.message = QStringLiteral("UFW: allowed %1").arg(portProto);
        qCInfo(lcFirewall) << result.message;
    } else {
        result.success = false;
        result.message = QStringLiteral("UFW D-Bus AllowPort failed: %1")
                             .arg(reply.isValid() ? QStringLiteral("returned false")
                                                  : reply.error().message());
        qCWarning(lcFirewall) << result.message;
    }

    onPortResult(result);
}

// ─── XDG Desktop Portal ───────────────────────────────────────────────────────
//
// The Firewall portal interface is not yet finalised in the XDG spec; this
// uses the proposed method signature.  In Flatpak we also rely on the host
// firewall being open, so this is best-effort.

void FirewallPortManager::requestViaXdgPortal(const PortRequest &req)
{
    QDBusInterface iface(QLatin1String(kPortalService),
                         QLatin1String(kPortalPath),
                         QLatin1String(kPortalFirewallIface),
                         QDBusConnection::sessionBus());

    PortResult result;
    result.request = req;
    result.backend = FirewallBackend::XdgPortal;

    if (!iface.isValid()) {
        // Portal not available — in a Flatpak, best we can do is log and let
        // the user open the port on the host system.
        result.success = false;
        result.message = QStringLiteral(
            "XDG Firewall portal not available. "
            "In Flatpak, open port %1/%2 on the host:\n"
            "  sudo firewall-cmd --add-port=%1/%2 --permanent && sudo firewall-cmd --reload")
            .arg(req.port).arg(req.protocol);
        qCWarning(lcFirewall) << result.message;
        onPortResult(result);
        return;
    }

    QVariantMap options;
    options[QStringLiteral("protocol")] = req.protocol;
    const QDBusReply<QVariantMap> reply =
        iface.call(QStringLiteral("RequestPort"), static_cast<uint>(req.port), options);

    result.success = reply.isValid();
    result.message = reply.isValid()
        ? QStringLiteral("Portal granted port %1/%2").arg(req.port).arg(req.protocol)
        : QStringLiteral("Portal denied: %1").arg(reply.error().message());

    qCInfo(lcFirewall) << result.message;
    onPortResult(result);
}

// ─── pkexec / iptables fallback ───────────────────────────────────────────────

void FirewallPortManager::requestViaPkexec(const PortRequest &req)
{
    // Validate protocol before passing to shell — defence-in-depth even though
    // PortRequest is internal. Only "tcp" and "udp" are valid iptables protocols.
    const QString proto = req.protocol.toLower();
    if (proto != QLatin1String("tcp") && proto != QLatin1String("udp")) {
        qCWarning(lcFirewall) << "requestViaPkexec: invalid protocol" << req.protocol;
        onPortResult(PortResult{ req, false,
            QStringLiteral("Invalid protocol: %1").arg(req.protocol),
            FirewallBackend::Pkexec });
        return;
    }

    // Build the iptables command as a QStringList — QProcess does NOT invoke a shell,
    // so each element is a separate argv entry and no shell injection is possible.
    // We also constrain the port to a quint16, which rules out injection via port number.
    const QStringList args = {
        QStringLiteral("iptables"),
        QStringLiteral("-I"), QStringLiteral("INPUT"),
        QStringLiteral("-p"), proto,
        QStringLiteral("--dport"), QString::number(req.port),
        QStringLiteral("-j"), QStringLiteral("ACCEPT")
    };

    auto *proc = new QProcess(this);
    const QString portDesc = QStringLiteral("%1/%2").arg(req.port).arg(proto);

    connect(proc, &QProcess::finished, this,
            [this, proc, req, portDesc](int exitCode, QProcess::ExitStatus) {
        PortResult result;
        result.request = req;
        result.backend = FirewallBackend::Pkexec;

        if (exitCode == 0) {
            result.success = true;
            result.message = QStringLiteral("iptables rule added for %1 via pkexec.")
                                 .arg(portDesc);
            qCInfo(lcFirewall) << result.message;
        } else {
            result.success = false;
            result.message = QStringLiteral("pkexec iptables failed (exit %1): %2")
                                 .arg(exitCode)
                                 .arg(QString::fromLocal8Bit(proc->readAllStandardError()));
            qCWarning(lcFirewall) << result.message;
        }

        proc->deleteLater();
        onPortResult(result);
    });

    qCInfo(lcFirewall) << "Requesting Polkit authentication for port" << portDesc;
    proc->start(QStringLiteral("pkexec"), args);
}

// ─── Result aggregator ────────────────────────────────────────────────────────

void FirewallPortManager::onPortResult(PortResult result)
{
    if (!result.success) m_allOk = false;
    emit portOpened(result);

    if (--m_pending == 0) {
        emit allPortsProcessed(m_allOk);
    }
}

// ─── UFW CLI ─────────────────────────────────────────────────────────────────

void FirewallPortManager::requestViaUfwCli(const PortRequest &req)
{
    // Run blocking CLI on a worker thread to avoid freezing the UI.
    const QString portProto = QStringLiteral("%1/%2").arg(req.port).arg(req.protocol);
    const QString ufwPath = QStandardPaths::findExecutable(QStringLiteral("ufw"));
    auto *thread = QThread::create([this, req, portProto, ufwPath]() {
        PortResult result;
        result.request = req;
        result.backend = FirewallBackend::UfwCli;

        QProcess proc;
        proc.start(ufwPath, { QStringLiteral("allow"), portProto });
        if (!proc.waitForStarted(2000)) {
            result.message = QStringLiteral(
                "ufw could not start. Run manually: sudo ufw allow %1").arg(portProto);
            QMetaObject::invokeMethod(this, [this, result]() { onPortResult(result); },
                                      Qt::QueuedConnection);
            return;
        }
        proc.waitForFinished(5000);

        result.success = (proc.exitCode() == 0);
        if (result.success) {
            result.message = QStringLiteral("ufw: allowed %1.").arg(portProto);
        } else {
            const QString err = QString::fromLocal8Bit(proc.readAllStandardError()).trimmed();
            if (err.contains(QLatin1String("already"), Qt::CaseInsensitive) ||
                err.contains(QLatin1String("Skipping"), Qt::CaseInsensitive)) {
                result.success = true;
                result.message = QStringLiteral("ufw: %1 already allowed.").arg(portProto);
            } else {
                result.message = QStringLiteral(
                    "ufw failed (exit %1): %2  —  run: sudo ufw allow %3")
                    .arg(proc.exitCode()).arg(err, portProto);
            }
        }
        QMetaObject::invokeMethod(this, [this, result]() { onPortResult(result); },
                                  Qt::QueuedConnection);
    });
    thread->setParent(this);
    connect(thread, &QThread::finished, thread, &QThread::deleteLater);
    thread->start();
}

// ─── nftables CLI ────────────────────────────────────────────────────────────

void FirewallPortManager::requestViaNftables(const PortRequest &req)
{
    const QString nftPath = QStandardPaths::findExecutable(QStringLiteral("nft"));
    const QString proto = (req.protocol == QLatin1String("tcp"))
        ? QStringLiteral("tcp") : QStringLiteral("udp");
    auto *thread = QThread::create([this, req, nftPath, proto]() {
        PortResult result;
        result.request = req;
        result.backend = FirewallBackend::Nftables;
        runCliBlocking(nftPath, { QStringLiteral("add"), QStringLiteral("table"),
            QStringLiteral("inet"), QStringLiteral("filter") });
        runCliBlocking(nftPath, { QStringLiteral("add"), QStringLiteral("chain"),
            QStringLiteral("inet"), QStringLiteral("filter"), QStringLiteral("input"),
            QStringLiteral("{ type filter hook input priority 0; }") });
        const QString portStr = QString::number(req.port);
        const QString addErr = runCliBlocking(nftPath, {
            QStringLiteral("add"), QStringLiteral("rule"),
            QStringLiteral("inet"), QStringLiteral("filter"), QStringLiteral("input"),
            proto, QStringLiteral("dport"), portStr, QStringLiteral("accept") });
        result.success = addErr.isEmpty();
        if (result.success) {
            result.message = QStringLiteral("nftables: allowed %1/%2.").arg(portStr, req.protocol);
        } else if (addErr.contains(QLatin1String("already"), Qt::CaseInsensitive)) {
            result.success = true;
            result.message = QStringLiteral("nftables: %1/%2 already allowed.").arg(portStr, req.protocol);
        } else {
            result.message = QStringLiteral(
                "nft failed: %1  —  run: sudo nft add rule inet filter input %2 dport %3 accept")
                .arg(addErr, proto, portStr);
        }
        QMetaObject::invokeMethod(this, [this, result]() { onPortResult(result); },
                                  Qt::QueuedConnection);
    });
    thread->setParent(this);
    connect(thread, &QThread::finished, thread, &QThread::deleteLater);
    thread->start();
}


// ─── Helpers ─────────────────────────────────────────────────────────────────

bool FirewallPortManager::runningAsRoot()
{
#ifdef Q_OS_UNIX
    return (::geteuid() == 0);
#else
    return false;
#endif
}

QString FirewallPortManager::runCliBlocking(const QString &exe,
                                             const QStringList &args,
                                             int timeoutMs)
{
    // NOTE: Must only be called from a background thread (QThread::create).
    // Never call from the main/UI thread — it blocks for up to timeoutMs ms.
    // All callers in this file launch this via QThread::create(); verified below.
    Q_ASSERT_X(!QCoreApplication::instance() ||
               QThread::currentThread() != QCoreApplication::instance()->thread(),
               "runCliBlocking", "called on main thread — will freeze UI");
    QProcess p;
    p.start(exe, args);
    if (!p.waitForStarted(2000)) return QStringLiteral("process failed to start");
    p.waitForFinished(timeoutMs);
    if (p.exitCode() != 0)
        return QString::fromLocal8Bit(p.readAllStandardError()).trimmed();
    return {};
}

// ─── firewall-cmd CLI ─────────────────────────────────────────────────────────
// Used when firewalld is installed but its D-Bus service is unavailable
// (minimal container installs, firewalld running without D-Bus).

void FirewallPortManager::requestViaFirewalldCli(const PortRequest &req)
{
    const QString exe = QStandardPaths::findExecutable(QStringLiteral("firewall-cmd"));
    const QString portProto = QStringLiteral("%1/%2").arg(req.port).arg(req.protocol);
    auto *thread = QThread::create([this, req, exe, portProto]() {
        PortResult result;
        result.request = req;
        result.backend = FirewallBackend::FirewalldCli;
        const QString err1 = runCliBlocking(exe, { QStringLiteral("--add-port=") + portProto });
        const QString err2 = runCliBlocking(exe,
            { QStringLiteral("--permanent"), QStringLiteral("--add-port=") + portProto });
        const bool ok1 = err1.isEmpty() || err1.contains(QLatin1String("already"), Qt::CaseInsensitive);
        const bool ok2 = err2.isEmpty() || err2.contains(QLatin1String("already"), Qt::CaseInsensitive);
        result.success = ok1 || ok2;
        result.message = result.success
            ? QStringLiteral("firewall-cmd: opened %1.").arg(portProto)
            : QStringLiteral("firewall-cmd failed: %1 / %2 — run: "
                "sudo firewall-cmd --add-port=%3 --permanent && sudo firewall-cmd --reload")
                .arg(err1, err2, portProto);
        QMetaObject::invokeMethod(this, [this, result]() { onPortResult(result); },
                                  Qt::QueuedConnection);
    });
    thread->setParent(this);
    connect(thread, &QThread::finished, thread, &QThread::deleteLater);
    thread->start();
}

// ─── iptables direct (root) ───────────────────────────────────────────────────
// Used on Alpine, in Docker containers, or any system running as root where
// no higher-level firewall manager is present.

void FirewallPortManager::requestViaIptablesDirect(const PortRequest &req)
{
    const QString proto = req.protocol.toLower();
    if (proto != QLatin1String("tcp") && proto != QLatin1String("udp")) {
        onPortResult(PortResult{ req, false,
            QStringLiteral("Invalid protocol: %1").arg(req.protocol),
            FirewallBackend::IptablesDirect });
        return;
    }
    auto *thread = QThread::create([this, req, proto]() {
        PortResult result;
        result.request = req;
        result.backend = FirewallBackend::IptablesDirect;
        const QString portStr = QString::number(req.port);
        const QStringList checkArgs = { QStringLiteral("-C"), QStringLiteral("INPUT"),
            QStringLiteral("-p"), proto, QStringLiteral("--dport"), portStr,
            QStringLiteral("-j"), QStringLiteral("ACCEPT") };
        const QStringList addArgs = { QStringLiteral("-I"), QStringLiteral("INPUT"),
            QStringLiteral("-p"), proto, QStringLiteral("--dport"), portStr,
            QStringLiteral("-j"), QStringLiteral("ACCEPT") };
        for (const char *cmd : { "iptables", "iptables-legacy" }) {
            const QString exe = QStandardPaths::findExecutable(QString::fromLatin1(cmd));
            if (exe.isEmpty()) continue;
            if (runCliBlocking(exe, checkArgs).isEmpty()) {
                result.success = true;
                result.message = QStringLiteral("%1: %2/%3 already allowed.")
                    .arg(QString::fromLatin1(cmd), portStr, proto);
                QMetaObject::invokeMethod(this, [this, result]() { onPortResult(result); },
                                          Qt::QueuedConnection);
                return;
            }
            if (runCliBlocking(exe, addArgs).isEmpty()) {
                result.success = true;
                result.message = QStringLiteral("%1: opened %2/%3.")
                    .arg(QString::fromLatin1(cmd), portStr, proto);
                QMetaObject::invokeMethod(this, [this, result]() { onPortResult(result); },
                                          Qt::QueuedConnection);
                return;
            }
        }
        result.success = false;
        result.message = QStringLiteral(
            "iptables direct failed — run: sudo iptables -I INPUT -p %1 --dport %2 -j ACCEPT")
            .arg(proto, portStr);
        QMetaObject::invokeMethod(this, [this, result]() { onPortResult(result); },
                                  Qt::QueuedConnection);
    });
    thread->setParent(this);
    connect(thread, &QThread::finished, thread, &QThread::deleteLater);
    thread->start();
}

} // namespace SnapClone::Network
