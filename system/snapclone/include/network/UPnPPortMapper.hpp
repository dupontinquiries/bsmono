#pragma once
// ─── UPnPPortMapper.hpp ───────────────────────────────────────────────────────
// Lightweight UPnP IGD port mapping using pure Qt — no external libraries.
// Discovers the home router via SSDP multicast, then requests a TCP port
// mapping so peers on other networks can reach our WebSocket server.
//
// Sequence:
//   1. Send SSDP M-SEARCH to 239.255.255.250:1900
//   2. Parse LOCATION header from router's response
//   3. HTTP GET the service description XML to find control URL
//   4. HTTP SOAP AddPortMapping to open the port
//   5. Re-map every 10 minutes (UPnP leases expire)
// ─────────────────────────────────────────────────────────────────────────────
#include <QObject>
#include <QUdpSocket>
#include <QNetworkAccessManager>
#include <QTimer>
#include <QHostAddress>

namespace SnapClone::Network {

class UPnPPortMapper : public QObject
{
    Q_OBJECT
public:
    explicit UPnPPortMapper(QObject *parent = nullptr);
    ~UPnPPortMapper() override;

    /// Start UPnP discovery and map the given port.
    /// internalIp: our LAN IP (e.g. 192.168.2.178)
    /// port: the TCP port to forward (9876 or 9877)
    void start(const QString &internalIp, quint16 port);
    void stop();

signals:
    void mappingAdded(quint16 externalPort, const QString &externalIp);
    void mappingFailed(const QString &reason);
    void portMapped(quint16 externalPort, const QString &method); ///< UPnP or NAT-PMP succeeded

private slots:
    void onSsdpResponse();

private:
    void sendSsdpSearch();
    void fetchServiceDescription(const QString &location);
    void addPortMapping(const QString &controlUrl,
                        const QString &serviceType);
    void parseDescriptionXml(const QByteArray &xml, const QString &baseUrl);
    void tryNatPmp(); ///< NAT-PMP fallback when UPnP fails

    QUdpSocket          m_ssdpSocket;
    QNetworkAccessManager m_nam;
    QTimer              m_renewTimer;
    QString             m_internalIp;
    quint16             m_port  = 0;
    quint16             m_wsPort = 0; // kept for NAT-PMP requests
    QString             m_controlUrl;
    QString             m_serviceType;
    bool                m_mapped = false;
    bool        m_natPmpFallbackScheduled = false;
};

} // namespace SnapClone::Network
