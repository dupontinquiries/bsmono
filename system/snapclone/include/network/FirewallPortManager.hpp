#pragma once

// ─── Network/FirewallPortManager.hpp ─────────────────────────────────────────
//
// Requests firewall port rules through platform D-Bus interfaces without
// requiring the user to run anything manually.
//
// Strategy (tried in order, first success wins):
//
//   1. firewalld  — org.fedoraproject.FirewallD1  (Fedora/RHEL/openSUSE)
//      Calls AddPort(port, protocol, 0) on the default zone.
//
//   2. UFW via D-Bus — com.ubuntu.USF1  (Ubuntu ≥ 24.04 with ufw-dbus)
//      Calls AllowPort(port, protocol).
//
//   3. XDG Desktop Portal — org.freedesktop.portal.NetworkMonitor
//      (Flatpak sandbox) — requests firewall exemption through the portal.
//
//   4. Direct iptables via pkexec — last resort for systems with none of
//      the above.  Spawns `pkexec iptables -I INPUT -p tcp --dport N -j ACCEPT`
//      which shows a Polkit authentication prompt.
//
// All operations are non-blocking; results arrive via signals.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QList>

namespace SnapClone::Network {

enum class FirewallBackend {
    None,
    Firewalld,        ///< firewalld D-Bus  (Fedora/RHEL/openSUSE/Arch)
    FirewalldCli,     ///< firewall-cmd CLI (firewalld without D-Bus in containers)
    UfwDbus,          ///< UFW D-Bus        (Ubuntu 24.04+)
    UfwCli,           ///< ufw CLI          (Ubuntu/Debian/Alpine)
    Nftables,         ///< nft CLI          (Debian 10+/Fedora 32+/Arch)
    IptablesDirect,   ///< iptables/ip6tables direct (running as root)
    XdgPortal,        ///< XDG Desktop Portal (Flatpak)
    Pkexec,           ///< pkexec + iptables (Polkit prompt)
};

struct PortRequest {
    quint16 port;
    QString protocol; // "tcp" or "udp"
};

struct PortResult {
    PortRequest request;
    bool        success = false;
    QString     message;
    FirewallBackend backend = FirewallBackend::None;
};

class FirewallPortManager : public QObject
{
    Q_OBJECT

public:
    explicit FirewallPortManager(QObject *parent = nullptr);

    /// Detect which backend is available on this system.
    [[nodiscard]] FirewallBackend detectBackend() const;

    /// Request that all given ports be opened.
    /// Results arrive via portOpened() / allPortsProcessed().
    void requestPorts(const QList<PortRequest> &ports);

    /// Returns a human-readable name for the backend.
    [[nodiscard]] static QString backendName(FirewallBackend b);

signals:
    void portOpened(SnapClone::Network::PortResult result);
    void allPortsProcessed(bool allSucceeded);
    void backendDetected(SnapClone::Network::FirewallBackend backend, QString name);

private:
    void requestViaFirewalld(const PortRequest &req);
    void requestViaFirewalldCli(const PortRequest &req);
    void requestViaUfw(const PortRequest &req);
    void requestViaUfwCli(const PortRequest &req);
    void requestViaNftables(const PortRequest &req);
    void requestViaIptablesDirect(const PortRequest &req);
    void requestViaXdgPortal(const PortRequest &req);
    void requestViaPkexec(const PortRequest &req);
    static bool runningAsRoot();
    static QString runCliBlocking(const QString &exe, const QStringList &args,
                                   int timeoutMs = 5000);

    void onPortResult(PortResult result);

    FirewallBackend m_backend  = FirewallBackend::None;
    int             m_pending  = 0;
    bool            m_allOk    = true;
};

} // namespace SnapClone::Network

Q_DECLARE_METATYPE(SnapClone::Network::PortResult)
Q_DECLARE_METATYPE(SnapClone::Network::FirewallBackend)
