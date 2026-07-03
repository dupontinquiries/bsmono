#pragma once
#include "core/Types.hpp"
#include <QObject>
#include <QString>
#include <optional>

QT_BEGIN_NAMESPACE
class QGeoPositionInfo;
class QGeoPositionInfoSource;
class QNetworkAccessManager;
QT_END_NAMESPACE

namespace SnapClone::Core {

// ─── LocationManager ──────────────────────────────────────────────────────────
//
// Thin wrapper around QGeoPositionInfoSource that:
//   • polls for position every 60 s while the app is active
//   • caches the last fix with a 5-minute TTL
//   • provides the current SSID for co-location hints
//   • exposes lastKnownLocation() for attaching to outgoing snaps
//
// Privacy design (see doc/LOCATION_PRIVACY.md):
//   • Location is NEVER sent without an explicit user action (sending a snap)
//   • Fixes are cached in RAM only; StorageManager writes to DB on snap send
//   • The cache is cleared on app exit — no background history is kept
//   • Accuracy is rounded to 100m before storage to reduce precision
// ─────────────────────────────────────────────────────────────────────────────

class LocationManager : public QObject
{
    Q_OBJECT
public:
    static constexpr int kMaxAgeMs    = 5 * 60 * 1000; // 5 minutes
    static constexpr int kPollMs      = 60 * 1000;      // poll every 60 s
    static constexpr double kRoundM   = 100.0;          // round accuracy to 100 m

    explicit LocationManager(QObject *parent = nullptr);
    ~LocationManager() override = default;

    /// Start listening for position updates.
    void start();
    void stop();

    /// Current SSID of the active WiFi network, empty if wired/cellular.
    [[nodiscard]] QString currentSsid() const;

    /// Last GPS fix, or nullopt if unavailable or older than kMaxAgeMs.
    [[nodiscard]] std::optional<PeerLocation> lastKnownLocation(
        const QString &selfId) const;

    /// True if we have a fresh fix (< kMaxAgeMs old).
    [[nodiscard]] bool hasFreshFix() const;

signals:
    /// Emitted when a new position fix arrives.
    void locationUpdated(double lat, double lon, double accuracyM, QString ssid);

private slots:
    void onPositionUpdated(const QGeoPositionInfo &info);
    void onError(int error);

private:
    QGeoPositionInfoSource *m_source    = nullptr;
    double   m_lat         = qQNaN();
    double   m_lon         = qQNaN();
    double   m_accM        = 0.0;
    qint64   m_fixTimeMs   = 0;   // Unix ms of last fix, 0 = never
};

} // namespace SnapClone::Core
