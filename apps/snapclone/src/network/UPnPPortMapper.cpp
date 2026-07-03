#include "network/UPnPPortMapper.hpp"

#include <QLoggingCategory>
#include <QNetworkInterface>
#include <QNetworkReply>
#include <QNetworkRequest>
#include <QUrl>
#include <QXmlStreamReader>
#include <QRegularExpression>

Q_LOGGING_CATEGORY(lcUpnp, "snapclone.upnp")

namespace SnapClone::Network {

static constexpr int kLeaseSeconds = 7200;
static constexpr int kRenewMs      = 10 * 60 * 1000;

UPnPPortMapper::UPnPPortMapper(QObject *parent)
    : QObject(parent)
{
    connect(&m_ssdpSocket, &QUdpSocket::readyRead,
            this, &UPnPPortMapper::onSsdpResponse);
    connect(&m_renewTimer, &QTimer::timeout, this, [this]() {
        if (!m_controlUrl.isEmpty())
            addPortMapping(m_controlUrl, m_serviceType);
        else
            sendSsdpSearch();
    });
}

UPnPPortMapper::~UPnPPortMapper() { stop(); }

void UPnPPortMapper::start(const QString &internalIp, quint16 port)
{
    m_internalIp = internalIp;
    m_port       = port;
    m_mapped     = false;

    if (!m_ssdpSocket.bind(QHostAddress::AnyIPv4, 0,
                           QUdpSocket::ShareAddress | QUdpSocket::ReuseAddressHint)) {
        qCWarning(lcUpnp) << "Failed to bind SSDP socket:" << m_ssdpSocket.errorString();
    }

    sendSsdpSearch();
    m_renewTimer.start(kRenewMs);
}

void UPnPPortMapper::stop()
{
    m_renewTimer.stop();
    m_ssdpSocket.close();
}

void UPnPPortMapper::sendSsdpSearch()
{
    const QStringList targets = {
        QStringLiteral("urn:schemas-upnp-org:service:WANIPConnection:1"),
        QStringLiteral("urn:schemas-upnp-org:service:WANIPConnection:2"),
        QStringLiteral("urn:schemas-upnp-org:device:InternetGatewayDevice:1"),
        QStringLiteral("upnp:rootdevice"),
    };
    for (const QString &st : targets) {
        const QByteArray msg =
            "M-SEARCH * HTTP/1.1\r\n"
            "HOST: 239.255.255.250:1900\r\n"
            "MAN: \"ssdp:discover\"\r\n"
            "MX: 3\r\n"
            "ST: " + st.toUtf8() + "\r\n"
            "\r\n";
        m_ssdpSocket.writeDatagram(msg,
            QHostAddress(QStringLiteral("239.255.255.250")), 1900);
    }
    qCInfo(lcUpnp) << "SSDP M-SEARCH sent";
    // Schedule NAT-PMP fallback only on the FIRST SSDP search.
    // The renew timer also calls sendSsdpSearch() every 10 minutes, which
    // would create a new 10s timer each time — causing tryNatPmp() to fire
    // repeatedly and accumulate parallel NAT-PMP sockets.
    if (!m_natPmpFallbackScheduled) {
        m_natPmpFallbackScheduled = true;
        QTimer::singleShot(10000, this, [this]() {
            if (m_controlUrl.isEmpty()) {
                qCWarning(lcUpnp) << "No UPnP IGD responded — trying NAT-PMP as fallback...";
                m_natPmpFallbackScheduled = false; // allow retry on next start()
                tryNatPmp();
            }
        });
    }
}

void UPnPPortMapper::onSsdpResponse()
{
    while (m_ssdpSocket.hasPendingDatagrams()) {
        QByteArray data;
        data.resize(static_cast<int>(m_ssdpSocket.pendingDatagramSize()));
        m_ssdpSocket.readDatagram(data.data(), data.size());

        const QString response = QString::fromLatin1(data);
        static const QRegularExpression kLocRx(
            QStringLiteral("LOCATION:\\s*(\\S+)"),
            QRegularExpression::CaseInsensitiveOption);
        const auto m = kLocRx.match(response);
        if (!m.hasMatch()) continue;

        const QString location = m.captured(1);
        qCInfo(lcUpnp) << "UPnP IGD found at:" << location;
        fetchServiceDescription(location);
        break;
    }
}

void UPnPPortMapper::fetchServiceDescription(const QString &location)
{
    // Use braces to avoid vexing-parse: QNetworkRequest req(QUrl(x)) is a
    // function declaration; QNetworkRequest req{QUrl{x}} is a variable.
    QNetworkRequest req{ QUrl{ location } };
    req.setTransferTimeout(5000);
    auto *reply = m_nam.get(req);
    connect(reply, &QNetworkReply::finished, this,
            [this, reply, location]() {
        reply->deleteLater();
        if (reply->error() != QNetworkReply::NoError) {
            qCWarning(lcUpnp) << "UPnP description fetch failed:"
                               << reply->errorString();
            emit mappingFailed(reply->errorString());
            return;
        }
        const QUrl locUrl{ location };
        const QString base = QStringLiteral("%1://%2:%3")
            .arg(locUrl.scheme(), locUrl.host())
            .arg(locUrl.port(80));
        parseDescriptionXml(reply->readAll(), base);
    });
}

void UPnPPortMapper::parseDescriptionXml(const QByteArray &xml,
                                          const QString    &baseUrl)
{
    QXmlStreamReader reader(xml);
    QString currentServiceType;
    QString controlUrl;
    bool inService = false;

    while (!reader.atEnd()) {
        reader.readNext();
        if (reader.isStartElement()) {
            const auto name = reader.name();
            if (name == QLatin1String("service"))
                inService = true;
            else if (inService && name == QLatin1String("serviceType"))
                currentServiceType = reader.readElementText();
            else if (inService && name == QLatin1String("controlURL"))
                controlUrl = reader.readElementText();
        } else if (reader.isEndElement()
                   && reader.name() == QLatin1String("service")) {
            if (currentServiceType.contains(QLatin1String("WANIPConnection")) ||
                currentServiceType.contains(QLatin1String("WANPPPConnection"))) {
                const QString full = controlUrl.startsWith(QLatin1Char('/'))
                    ? baseUrl + controlUrl : controlUrl;
                qCInfo(lcUpnp) << "WANIPConnection control URL:" << full;
                m_controlUrl  = full;
                m_serviceType = currentServiceType;
                addPortMapping(full, currentServiceType);
                return;
            }
            inService = false;
            currentServiceType.clear();
            controlUrl.clear();
        }
    }
    qCWarning(lcUpnp) << "No WANIPConnection service found in router description";
    emit mappingFailed(QStringLiteral("No WANIPConnection service found"));
}

void UPnPPortMapper::addPortMapping(const QString &controlUrl,
                                     const QString &serviceType)
{
    const QString portStr = QString::number(m_port);
    const QString body = QStringLiteral(
        "<?xml version=\"1.0\"?>"
        "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" "
        "s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">"
        "<s:Body><u:AddPortMapping xmlns:u=\"%1\">"
        "<NewRemoteHost></NewRemoteHost>"
        "<NewExternalPort>%2</NewExternalPort>"
        "<NewProtocol>TCP</NewProtocol>"
        "<NewInternalPort>%2</NewInternalPort>"
        "<NewInternalClient>%3</NewInternalClient>"
        "<NewEnabled>1</NewEnabled>"
        "<NewPortMappingDescription>SnapClone</NewPortMappingDescription>"
        "<NewLeaseDuration>%4</NewLeaseDuration>"
        "</u:AddPortMapping></s:Body></s:Envelope>")
        .arg(serviceType, portStr, m_internalIp)
        .arg(kLeaseSeconds);

    QNetworkRequest req{ QUrl{ controlUrl } };
    req.setHeader(QNetworkRequest::ContentTypeHeader,
                  QStringLiteral("text/xml; charset=\"utf-8\""));
    req.setRawHeader("SOAPAction",
        QStringLiteral("\"%1#AddPortMapping\"").arg(serviceType).toUtf8());
    req.setTransferTimeout(5000);

    auto *reply = m_nam.post(req, body.toUtf8());
    connect(reply, &QNetworkReply::finished, this,
            [this, reply, portStr]() {
        reply->deleteLater();
        const QByteArray respBody = reply->readAll();
        if (reply->error() != QNetworkReply::NoError) {
            const int status =
                reply->attribute(QNetworkRequest::HttpStatusCodeAttribute).toInt();
            if (status == 500 && respBody.contains("ConflictInMappingEntry")) {
                qCInfo(lcUpnp) << "Port" << portStr << "already mapped — OK";
                m_mapped = true;
                emit mappingAdded(m_port, {});
                return;
            }
            qCWarning(lcUpnp) << "UPnP AddPortMapping failed:"
                               << reply->errorString() << respBody.left(256);
            emit mappingFailed(reply->errorString());
            return;
        }
        qCInfo(lcUpnp) << "UPnP mapped external TCP" << portStr
                        << "→" << m_internalIp << ":" << portStr;
        m_mapped = true;
        emit mappingAdded(m_port, {});
    });
}


// ─── NAT-PMP fallback ─────────────────────────────────────────────────────────
// RFC 6886: NAT Port Mapping Protocol
// Simpler than UPnP — one UDP packet to gateway port 5351.
// Bell Home Hub 3000/4000 support NAT-PMP since ~2019 firmware.
// Apple devices use it extensively (AirPlay, Back to My Mac).

void UPnPPortMapper::tryNatPmp()
{
    // Find the default gateway (usually 192.168.x.1 or 10.x.x.1)
    // We try all interfaces and send to the broadcast/gateway addr.
    QString gatewayIp;
    const auto ifaces = QNetworkInterface::allInterfaces();
    for (const QNetworkInterface &iface : ifaces) {
        if (!(iface.flags() & QNetworkInterface::IsUp)) continue;
        if (!(iface.flags() & QNetworkInterface::IsRunning)) continue;
        if (iface.flags() & QNetworkInterface::IsLoopBack) continue;
        for (const QNetworkAddressEntry &entry : iface.addressEntries()) {
            if (entry.ip().protocol() != QAbstractSocket::IPv4Protocol) continue;
            // Gateway is typically ip with last octet = 1
            QString ip = entry.ip().toString();
            QStringList parts = ip.split(QLatin1Char('.'));
            if (parts.size() == 4) {
                parts[3] = QStringLiteral("1");
                gatewayIp = parts.join(QLatin1Char('.'));
                break;
            }
        }
        if (!gatewayIp.isEmpty()) break;
    }

    if (gatewayIp.isEmpty()) {
        qCWarning(lcUpnp) << "NAT-PMP: could not determine gateway IP";
        qCWarning(lcUpnp) << "TCP:9876 may not be reachable. Set up port forwarding manually.";
        return;
    }

    qCInfo(lcUpnp) << "NAT-PMP: trying gateway" << gatewayIp << "port 5351";

    auto *sock  = new QUdpSocket(this);
    auto *timer = new QTimer(this);
    timer->setSingleShot(true);
    auto done = std::make_shared<bool>(false);

    // NAT-PMP Map TCP+UDP request: v0, opcode=1(UDP)+2(TCP) — we send two requests
    // Request format: version(1=0x00) opcode(1) reserved(2) internalPort(2) externalPort(2) lifetime(4)
    auto buildNatPmpRequest = [this](quint8 opcode) -> QByteArray {
        QByteArray req(12, 0);
        req[0] = 0x00; // version
        req[1] = opcode; // 1=UDP, 2=TCP
        // reserved[2-3] = 0
        // internalPort = our listen port (set via start())
        req[4] = static_cast<char>((m_port >> 8) & 0xFF);
        req[5] = static_cast<char>(m_port & 0xFF);
        // externalPort = same (hint to router; 0 = auto-assign)
        req[6] = static_cast<char>((m_port >> 8) & 0xFF);
        req[7] = static_cast<char>(m_port & 0xFF);
        // lifetime = 7200s (2 hours)
        req[8]  = 0x00;
        req[9]  = 0x00;
        req[10] = 0x1C;
        req[11] = 0x20;
        return req;
    };

    // Send UDP mapping request first, then TCP
    sock->writeDatagram(buildNatPmpRequest(1), QHostAddress(gatewayIp), 5351);
    sock->writeDatagram(buildNatPmpRequest(2), QHostAddress(gatewayIp), 5351);

    connect(sock, &QUdpSocket::readyRead, this, [=, this]() {
        if (*done) return;
        while (sock->hasPendingDatagrams()) {
            QByteArray resp(static_cast<int>(sock->pendingDatagramSize()), Qt::Uninitialized);
            sock->readDatagram(resp.data(), resp.size());
            if (resp.size() >= 16) {
                const quint16 result = (static_cast<quint8>(resp[2]) << 8) | static_cast<quint8>(resp[3]);
                const quint8  opcode = static_cast<quint8>(resp[1]);
                const quint16 extPort = (static_cast<quint8>(resp[14]) << 8) | static_cast<quint8>(resp[15]);
                if (result == 0) { // success
                    *done = true;
                    timer->stop();
                    sock->deleteLater();
                    timer->deleteLater();
                    const QString proto = (opcode == 129) ? QStringLiteral("UDP") : QStringLiteral("TCP");
                    qCInfo(lcUpnp) << "NAT-PMP: mapped" << proto << "external port" << extPort
                                   << "→ internal" << m_wsPort << "(lifetime 2h)";
                    emit portMapped(extPort, QStringLiteral("NAT-PMP"));
                    return;
                }
            }
        }
    });

    connect(timer, &QTimer::timeout, this, [=, this]() {
        if (*done) return;
        *done = true;
        sock->deleteLater();
        timer->deleteLater();
        qCWarning(lcUpnp) << "NAT-PMP: no response from gateway" << gatewayIp;
        qCWarning(lcUpnp) << "Neither UPnP nor NAT-PMP succeeded.";
        qCWarning(lcUpnp) << "Set up manual port forwarding: TCP+UDP port 9876 → internal IP.";
    });

    timer->start(3000);
}

} // namespace SnapClone::Network
