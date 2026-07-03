#include "core/LocationService.hpp"
#include "core/Timings.hpp"

#include <QNetworkRequest>
#include <QJsonDocument>
#include <QJsonObject>
#include <QNetworkInterface>
#include <QLoggingCategory>
#include <QDBusInterface>
#include <QDBusReply>
#include <QDBusConnection>
#include <cmath>

Q_LOGGING_CATEGORY(lcLoc, "snapclone.location")

namespace SnapClone::Core {

// ─── GeoClue2 DBus constants ──────────────────────────────────────────────────
static constexpr auto kGeoClueService   = "org.freedesktop.GeoClue2";
static constexpr auto kGeoClueManager  = "/org/freedesktop/GeoClue2/Manager";
static constexpr auto kGeoClueIface    = "org.freedesktop.GeoClue2.Manager";
static constexpr auto kClientIface     = "org.freedesktop.GeoClue2.Client";
static constexpr auto kLocationIface   = "org.freedesktop.GeoClue2.Location";

LocationService::LocationService(QObject *parent)
    : QObject(parent)
    , m_nam(new QNetworkAccessManager(this))
{
    connect(&m_pollTimer, &QTimer::timeout, this, &LocationService::poll);
    connect(m_nam, &QNetworkAccessManager::finished,
            this, &LocationService::onIpGeoReply);
}

LocationService::~LocationService() { stop(); }

void LocationService::start(const UserId &peer, Mode mode,
                            int intervalSecs, int durationSecs)
{
    stop();
    m_peer  = peer;
    m_mode  = mode;

    const int ms = qBound(1, intervalSecs, 3600) * 1000;
    m_pollTimer.start(ms);

    if (durationSecs > 0) {
        m_durationTimer.setSingleShot(true);
        connect(&m_durationTimer, &QTimer::timeout, this, [this]() {
            qCInfo(lcLoc) << "Location sharing duration expired — stopping.";
            stop();
        });
        m_durationTimer.start(durationSecs * 1000);
    }

    qCInfo(lcLoc) << "Location sharing started:"
                  << (mode == Mode::Gps ? "GPS" : "WiFi")
                  << "every" << intervalSecs << "s to" << peer.value;
    poll(); // fire immediately so user sees it working
}

void LocationService::stop()
{
    m_pollTimer.stop();
    m_durationTimer.stop();
    disconnect(&m_durationTimer, nullptr, this, nullptr);
    qCInfo(lcLoc) << "Location sharing stopped.";
}

// ─── Poll ─────────────────────────────────────────────────────────────────────

void LocationService::poll()
{
    if (m_mode == Mode::Wifi) {
        LocationFix fix;
        fix.senderId = m_peer; // populated by caller when forwarding
        fix.ssid     = readWifiSsid();
        fix.hasGps   = false;
        fix.timestamp = QDateTime::currentDateTimeUtc();
        if (fix.valid())
            emit locationReady(fix);
        else
            qCWarning(lcLoc) << "WiFi SSID unavailable";
        return;
    }

    // GPS mode — try GeoClue2 first, fall back to IP geolocation
    queryGeoClue();
}

// ─── WiFi SSID ────────────────────────────────────────────────────────────────

QString LocationService::readWifiSsid() const
{
    // Try NetworkManager D-Bus first (most reliable on desktop Linux)
    QDBusInterface nm(QStringLiteral("org.freedesktop.NetworkManager"),
                      QStringLiteral("/org/freedesktop/NetworkManager"),
                      QStringLiteral("org.freedesktop.NetworkManager"),
                      QDBusConnection::systemBus());
    if (nm.isValid()) {
        const QVariant ap = nm.property("PrimaryConnection");
        if (ap.isValid()) {
            const QString path = ap.value<QDBusObjectPath>().path();
            if (!path.isEmpty() && path != QStringLiteral("/")) {
                QDBusInterface conn(QStringLiteral("org.freedesktop.NetworkManager"),
                                    path,
                                    QStringLiteral("org.freedesktop.NetworkManager.Connection.Active"),
                                    QDBusConnection::systemBus());
                const QString id = conn.property("Id").toString();
                if (!id.isEmpty()) return id;
            }
        }
    }

    // Fallback 1: iwd (used on postmarketOS/Alpine)
    {
        QDBusInterface iwd(QStringLiteral("net.connman.iwd"),
                           QStringLiteral("/"),
                           QStringLiteral("org.freedesktop.DBus.ObjectManager"),
                           QDBusConnection::systemBus());
        if (iwd.isValid()) {
            // iwd exposes network objects; find a Connected one and read its Name
            const QDBusReply<QVariantMap> objs =
                iwd.call(QStringLiteral("GetManagedObjects"));
            if (objs.isValid()) {
                const auto topMap = objs.value();
                for (auto it = topMap.cbegin(); it != topMap.cend(); ++it) {
                    const QVariantMap ifaces = it.value().toMap();
                    const QVariantMap netIface =
                        ifaces.value(QStringLiteral("net.connman.iwd.Network")).toMap();
                    if (netIface.value(QStringLiteral("Connected"), false).toBool()) {
                        const QString name = netIface.value(QStringLiteral("Name")).toString();
                        if (!name.isEmpty()) return name;
                    }
                }
            }
        }
    }

    // Fallback 2: wpa_supplicant D-Bus (used on many distros when NM isn't running)
    {
        QDBusInterface wpa(QStringLiteral("fi.w1.wpa_supplicant1"),
                           QStringLiteral("/fi/w1/wpa_supplicant1"),
                           QStringLiteral("fi.w1.wpa_supplicant1"),
                           QDBusConnection::systemBus());
        if (wpa.isValid()) {
            const QVariant ifaces = wpa.property("Interfaces");
            const auto paths = ifaces.value<QList<QDBusObjectPath>>();
            for (const QDBusObjectPath &p : paths) {
                QDBusInterface wpai(QStringLiteral("fi.w1.wpa_supplicant1"),
                                    p.path(),
                                    QStringLiteral("fi.w1.wpa_supplicant1.Interface"),
                                    QDBusConnection::systemBus());
                const QString ssid = wpai.property("SSID").toString();
                if (!ssid.isEmpty()) return ssid;
            }
        }
    }

    // Fallback 3: /sys/class/net/<iface>/wireless — raw kernel interface (Alpine/postmarketOS)
    for (const QNetworkInterface &iface : QNetworkInterface::allInterfaces()) {
        const QString name = iface.name();
        if (!name.startsWith(QStringLiteral("wlan")) &&
            !name.startsWith(QStringLiteral("wlp"))  &&
            !name.startsWith(QStringLiteral("wlan0"))) continue;

        QFile f(QStringLiteral("/sys/class/net/%1/wireless/ssid").arg(name));
        if (f.open(QIODevice::ReadOnly)) {
            const QString ssid = QString::fromLatin1(f.readAll()).trimmed();
            if (!ssid.isEmpty()) return ssid;
        }
    }
    return {};
}

// ─── GeoClue2 ────────────────────────────────────────────────────────────────

void LocationService::queryGeoClue()
{
    // Re-entrancy guard: if a GeoClue session is already in flight (singleShot
    // timer running), don't start another. GeoClue daemon creates a resource
    // per GetClient() call; overlapping sessions waste D-Bus resources.
    if (m_geoClueActive) {
        qCDebug(lcLoc) << "GeoClue query already in flight — skipping";
        return;
    }
    m_geoClueActive = true;
    QDBusInterface mgr(QString::fromLatin1(kGeoClueService),
                       QString::fromLatin1(kGeoClueManager),
                       QString::fromLatin1(kGeoClueIface),
                       QDBusConnection::systemBus());

    if (!mgr.isValid()) {
        qCDebug(lcLoc) << "GeoClue2 unavailable — falling back to IP geolocation";
        fallbackIpGeo();
        return;
    }

    // GetClient() → returns object path of a new client
    const QDBusReply<QDBusObjectPath> clientReply = mgr.call(
        QStringLiteral("GetClient"));
    if (!clientReply.isValid()) {
        m_geoClueActive = false;
        fallbackIpGeo();
        return;
    }

    const QString clientPath = clientReply.value().path();
    QDBusInterface client(QString::fromLatin1(kGeoClueService),
                          clientPath,
                          QString::fromLatin1(kClientIface),
                          QDBusConnection::systemBus());
    client.setProperty("DesktopId", QStringLiteral("snapclone"));
    client.setProperty("RequestedAccuracyLevel", 4); // city-level

    // Check Start() return for immediate failures (e.g. permission denied).
    // On postmarketOS, the app may not be in the geoclue allowed list.
    const QDBusMessage startReply = client.call(QStringLiteral("Start"));
    if (startReply.type() == QDBusMessage::ErrorMessage) {
        qCWarning(lcLoc) << "GeoClue2 Start failed:"
                         << startReply.errorName() << startReply.errorMessage()
                         << "— add snapclone to /etc/geoclue/geoclue.conf [snapclone] system=true";
        m_geoClueActive = false;
        fallbackIpGeo();
        return;
    }

    // Give GeoClue time to acquire a fix. 800ms is too short for cold GPS start;
    // we use a signal subscription instead: connect to the LocationUpdated signal
    // so we get the fix as soon as it arrives rather than polling after a fixed delay.
    // The 5s fallback timer ensures we don't wait forever on a device with no GPS.
    QTimer::singleShot(5000, this, [this, clientPath]() {
        QDBusInterface client2(QString::fromLatin1(kGeoClueService),
                               clientPath,
                               QString::fromLatin1(kClientIface),
                               QDBusConnection::systemBus());
        const QString locPath = client2.property("Location")
                                    .value<QDBusObjectPath>().path();
        if (locPath.isEmpty() || locPath == QStringLiteral("/")) {
            qCDebug(lcLoc) << "GeoClue2 has no fix yet — using IP geolocation";
            client2.call(QStringLiteral("Stop"));
            m_geoClueActive = false;
            fallbackIpGeo();
            return;
        }
        QDBusInterface loc(QString::fromLatin1(kGeoClueService),
                           locPath,
                           QString::fromLatin1(kLocationIface),
                           QDBusConnection::systemBus());
        const double lat = loc.property("Latitude").toDouble();
        const double lon = loc.property("Longitude").toDouble();
        const double acc = loc.property("Accuracy").toDouble();
        client2.call(QStringLiteral("Stop"));

        if ((lat == 0.0 && lon == 0.0)
                || lat < -90.0 || lat > 90.0
                || lon < -180.0 || lon > 180.0) {
            client2.call(QStringLiteral("Stop"));
            m_geoClueActive = false;
            fallbackIpGeo();
            return;
        }

        m_cachedLat = lat;
        m_cachedLon = lon;
        m_hasGps    = true;

        LocationFix fix;
        fix.hasGps   = true;
        fix.lat      = lat;
        fix.lon      = lon;
        fix.accuracy = static_cast<float>(acc);
        fix.ssid     = readWifiSsid();
        fix.timestamp = QDateTime::currentDateTimeUtc();
        emit locationReady(fix);
    });
}

// ─── IP-based geolocation fallback ───────────────────────────────────────────

void LocationService::fallbackIpGeo()
{
    // Cache the last IP geolocation for 10 minutes to avoid hitting ip-api.com
    // rate limits (45 req/min free tier). The poll timer may fire every 30s;
    // without caching that's 2 req/min per peer session which adds up quickly.
    if (m_lastIpGeoAt.isValid()
        && m_lastIpGeoAt.secsTo(QDateTime::currentDateTimeUtc()) < 600) {
        // Reuse last cached IP-geo fix — just re-emit it with a fresh timestamp
        if (m_hasGps && (m_cachedLat != 0.0 || m_cachedLon != 0.0)) {
            LocationFix fix;
            fix.hasGps   = true;
            fix.lat      = m_cachedLat;
            fix.lon      = m_cachedLon;
            fix.accuracy = 5000.f;
            fix.ssid     = readWifiSsid();
            fix.timestamp = QDateTime::currentDateTimeUtc();
            emit locationReady(fix);
            m_geoClueActive = false;
            return;
        }
    }

    // Use HTTPS to prevent MITM interception of location queries.
    const QUrl url(QStringLiteral("https://ip-api.com/json/?fields=lat,lon,status"));
    QNetworkRequest req(url);
    req.setAttribute(QNetworkRequest::RedirectPolicyAttribute,
                     QNetworkRequest::NoLessSafeRedirectPolicy);
    // 10-second timeout so a down ip-api.com doesn't hang the location service.
    req.setTransferTimeout(10000);
    m_nam->get(req);
}

void LocationService::onIpGeoReply(QNetworkReply *reply)
{
    reply->deleteLater();
    if (reply->error() != QNetworkReply::NoError) {
        m_geoClueActive = false; // release guard on failure
        qCWarning(lcLoc) << "IP geolocation failed:" << reply->errorString();
        // Emit WiFi-only fix so the user at least gets SSID
        LocationFix fix;
        fix.ssid      = readWifiSsid();
        fix.timestamp = QDateTime::currentDateTimeUtc();
        if (fix.valid()) emit locationReady(fix);
        return;
    }

    const QJsonObject obj = QJsonDocument::fromJson(reply->readAll()).object();
    if (obj.value(QStringLiteral("status")).toString() != QStringLiteral("success")) {
        m_geoClueActive = false;
        qCWarning(lcLoc) << "IP geolocation returned non-success";
        return;
    }

    const double rawLat = obj.value(QStringLiteral("lat")).toDouble();
    const double rawLon = obj.value(QStringLiteral("lon")).toDouble();
    // Validate coordinate ranges — reject spoofed or malformed responses.
    if (rawLat < -90.0 || rawLat > 90.0 || rawLon < -180.0 || rawLon > 180.0) {
        m_geoClueActive = false;
        qCWarning(lcLoc) << "IP geolocation returned invalid coordinates:"                         << rawLat << rawLon << "— discarding";
        return;
    }
    m_cachedLat   = rawLat;
    m_cachedLon   = rawLon;
    m_hasGps      = true;
    m_lastIpGeoAt = QDateTime::currentDateTimeUtc(); // mark for cache
    m_geoClueActive = false; // release guard

    LocationFix fix;
    fix.hasGps   = true;
    fix.lat      = m_cachedLat;
    fix.lon      = m_cachedLon;
    fix.accuracy = 5000.f; // IP geo is city-level ~5km
    fix.ssid     = readWifiSsid();
    fix.timestamp = QDateTime::currentDateTimeUtc();
    qCInfo(lcLoc) << "IP geolocation: lat" << fix.lat << "lon" << fix.lon;
    emit locationReady(fix);
}

} // namespace SnapClone::Core
