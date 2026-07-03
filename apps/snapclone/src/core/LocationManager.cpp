#include "core/LocationManager.hpp"
#include "core/Timings.hpp"

#include <QGeoPositionInfo>
#include <QGeoPositionInfoSource>
#include <QSettings>
#include <QDateTime>
#include <QTimeZone>
#include <QLoggingCategory>
#include <QtMath>

#if defined(Q_OS_LINUX) && __has_include(<QNetworkInterface>)
#include <QNetworkInterface>
#endif

Q_LOGGING_CATEGORY(lcLoc, "snapclone.location")

namespace SnapClone::Core {

// ─── Haversine distance ───────────────────────────────────────────────────────

double PeerLocation::distanceTo(double toLat, double toLon) const
{
    constexpr double R = 6371000.0; // Earth radius in metres
    const double φ1 = qDegreesToRadians(lat);
    const double φ2 = qDegreesToRadians(toLat);
    const double Δφ = qDegreesToRadians(toLat - lat);
    const double Δλ = qDegreesToRadians(toLon - lon);
    const double a  = qSin(Δφ/2)*qSin(Δφ/2)
                    + qCos(φ1)*qCos(φ2)*qSin(Δλ/2)*qSin(Δλ/2);
    return R * 2.0 * qAtan2(qSqrt(a), qSqrt(1.0 - a));
}

QString PeerLocation::ageString() const
{
    if (!recordedAt.isValid()) return QObject::tr("unknown");
    const qint64 secsAgo = recordedAt.secsTo(QDateTime::currentDateTimeUtc());
    if (secsAgo <  60)  return QObject::tr("just now");
    if (secsAgo < 120)  return QObject::tr("1 min ago");
    if (secsAgo < 3600) return QObject::tr("%1 min ago").arg(secsAgo / 60);
    return QObject::tr("%1 h ago").arg(secsAgo / 3600);
}

// V32.31: Landmark::distanceMTo lives inline in include/core/Types.hpp.
// StorageManager's loadLandmarksNear references it from the storage lib,
// which doesn't link against snapclone_core — putting the body here
// would produce an undefined-reference at storage's link step.

// V32.31: LocationSource string round-trip. Moved out of Types.hpp
// (where it was inline initially) to keep that very-widely-included
// header's footprint small. The helpers are used during storage
// serialization and UI provenance display — both rare paths, so the
// out-of-line cost is negligible.
QString locationSourceToString(LocationSource s)
{
    switch (s) {
    case LocationSource::Gps:      return QStringLiteral("gps");
    case LocationSource::Manual:   return QStringLiteral("manual");
    case LocationSource::Snap:     return QStringLiteral("snap");
    case LocationSource::Geocoded: return QStringLiteral("geocoded");
    case LocationSource::Network:  return QStringLiteral("network");
    case LocationSource::WiFi:     return QStringLiteral("wifi");
    case LocationSource::Landmark: return QStringLiteral("landmark");
    case LocationSource::Derived:  return QStringLiteral("derived");
    case LocationSource::Unknown:  break;
    }
    return QString();
}

LocationSource locationSourceFromString(const QString &s)
{
    if (s == QStringLiteral("gps"))      return LocationSource::Gps;
    if (s == QStringLiteral("manual"))   return LocationSource::Manual;
    if (s == QStringLiteral("snap"))     return LocationSource::Snap;
    if (s == QStringLiteral("geocoded")) return LocationSource::Geocoded;
    if (s == QStringLiteral("network"))  return LocationSource::Network;
    if (s == QStringLiteral("wifi"))     return LocationSource::WiFi;
    if (s == QStringLiteral("landmark")) return LocationSource::Landmark;
    if (s == QStringLiteral("derived"))  return LocationSource::Derived;
    return LocationSource::Unknown;
}

// ─── LocationManager ─────────────────────────────────────────────────────────

LocationManager::LocationManager(QObject *parent)
    : QObject(parent)
{}

void LocationManager::start()
{
    if (m_source) return; // already running

    m_source = QGeoPositionInfoSource::createDefaultSource(this);
    if (!m_source) {
        qCInfo(lcLoc) << "No position source available on this device";
        return;
    }

    m_source->setUpdateInterval(kPollMs);
    m_source->setPreferredPositioningMethods(
        QGeoPositionInfoSource::SatellitePositioningMethods
        | QGeoPositionInfoSource::NonSatellitePositioningMethods);

    connect(m_source, &QGeoPositionInfoSource::positionUpdated,
            this, &LocationManager::onPositionUpdated);

    // Qt6: error signal carries QGeoPositionInfoSource::Error enum
    connect(m_source, &QGeoPositionInfoSource::errorOccurred,
            this, [this](QGeoPositionInfoSource::Error e) {
                onError(static_cast<int>(e));
            });

    m_source->startUpdates();
    qCInfo(lcLoc) << "Location updates started — interval" << kPollMs / 1000 << "s";
}

void LocationManager::stop()
{
    if (m_source) {
        m_source->stopUpdates();
        m_source->deleteLater();
        m_source = nullptr;
        qCInfo(lcLoc) << "Location updates stopped";
    }
}

QString LocationManager::currentSsid() const
{
#if defined(Q_OS_LINUX) && __has_include(<QNetworkInterface>)
    // On Linux, we can read the SSID from /proc/net/wireless or via iw(1).
    // For now, return empty — a future DBus/NetworkManager integration
    // could fill this in. The SSID is purely informational.
#endif
    return QString{};
}

std::optional<PeerLocation> LocationManager::lastKnownLocation(
    const QString &selfId) const
{
    // Manual override path. If the user set location/manualLat and manualLon
    // in QSettings (Settings UI Privacy section, or programmatically), use
    // those as the "fix" instead of the GPS source. This is essential for:
    //   • Desktop testing — Linux desktops without GeoClue have no GPS source
    //     at all, so QGeoPositionInfoSource::createDefaultSource returns null
    //     and m_source stays uninitialised. Manual override lets us still
    //     test the location-sharing UI end to end.
    //   • Privacy — a user might prefer to share an approximate hand-set
    //     location ("near downtown Toronto") rather than precise GPS.
    {
        QSettings s;
        if (s.contains(QStringLiteral("location/manualLat"))
         && s.contains(QStringLiteral("location/manualLon"))) {
            bool okLat = false, okLon = false;
            const double lat = s.value(QStringLiteral("location/manualLat")).toDouble(&okLat);
            const double lon = s.value(QStringLiteral("location/manualLon")).toDouble(&okLon);
            if (okLat && okLon
             && lat >= -90.0 && lat <= 90.0
             && lon >= -180.0 && lon <= 180.0) {
                PeerLocation loc;
                loc.peerId     = selfId;
                loc.lat        = lat;
                loc.lon        = lon;
                loc.accuracyM  = s.value(QStringLiteral("location/manualAccM"), 100.0).toDouble();
                loc.ssid       = currentSsid();
                loc.recordedAt = QDateTime::currentDateTimeUtc();
                loc.source     = QStringLiteral("manual");
                return loc;
            }
        }
    }

    if (!hasFreshFix()) return std::nullopt;

    PeerLocation loc;
    loc.peerId     = selfId;
    loc.lat        = m_lat;
    loc.lon        = m_lon;
    loc.accuracyM  = m_accM;
    loc.ssid       = currentSsid();
    loc.recordedAt = QDateTime::fromMSecsSinceEpoch(m_fixTimeMs, QTimeZone::utc());
    loc.source     = QStringLiteral("gps");
    return loc;
}

bool LocationManager::hasFreshFix() const
{
    if (m_fixTimeMs == 0 || qIsNaN(m_lat) || qIsNaN(m_lon)) return false;
    return (QDateTime::currentMSecsSinceEpoch() - m_fixTimeMs) < kMaxAgeMs;
}

// ─── Slots ───────────────────────────────────────────────────────────────────

void LocationManager::onPositionUpdated(const QGeoPositionInfo &info)
{
    if (!info.isValid()) return;

    const QGeoCoordinate coord = info.coordinate();
    if (!coord.isValid()) return;

    m_lat      = coord.latitude();
    m_lon      = coord.longitude();
    m_fixTimeMs = QDateTime::currentMSecsSinceEpoch();

    // Horizontal accuracy — round to kRoundM to reduce false precision
    m_accM = info.hasAttribute(QGeoPositionInfo::HorizontalAccuracy)
        ? qMax(kRoundM, std::round(info.attribute(
              QGeoPositionInfo::HorizontalAccuracy) / kRoundM) * kRoundM)
        : 0.0;

    const QString ssid = currentSsid();
    qCDebug(lcLoc) << "Fix:" << m_lat << m_lon
                   << "acc" << m_accM << "m  ssid:" << (ssid.isEmpty() ? "(none)" : ssid);

    emit locationUpdated(m_lat, m_lon, m_accM, ssid);
}

void LocationManager::onError(int error)
{
    qCWarning(lcLoc) << "Position source error:" << error;
}

} // namespace SnapClone::Core
