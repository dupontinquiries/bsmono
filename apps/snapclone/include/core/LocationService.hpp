#pragma once
#include "core/LocationTypes.hpp"
#include <QObject>
#include <QTimer>
#include <QString>
#include <QNetworkAccessManager>
#include <QNetworkReply>

// ─── LocationService ──────────────────────────────────────────────────────────
// Polls the device position on a timer and emits locationReady().
//
// GPS path  : GeoClue2 via QDBusInterface (Linux/postmarketOS/Flatpak).
//             Falls back to ip-api.com geolocation if DBus unavailable.
// WiFi path : QNetworkInterface::allInterfaces() — reads SSID from the
//             OS wireless extensions (works on Linux without extra deps).
//
// No Qt6::Positioning dependency — only Qt6::Network and Qt6::DBus.

namespace SnapClone::Core {

class LocationService : public QObject
{
    Q_OBJECT
public:
    enum class Mode { Gps, Wifi };

    explicit LocationService(QObject *parent = nullptr);
    ~LocationService() override;

    /// Start sharing location with the given peer.
    /// @param intervalSecs  How often to sample + send (1–3600).
    ///                      0 = stop after one shot (not used in this flow).
    /// @param durationSecs  Total sharing duration; 0 = indefinite.
    void start(const UserId &peer, Mode mode, int intervalSecs, int durationSecs);
    void stop();

    bool active() const { return m_pollTimer.isActive(); }
    UserId activePeer() const { return m_peer; }

signals:
    void locationReady(SnapClone::Core::LocationFix fix);
    void error(const QString &msg);

private slots:
    void poll();
    void onIpGeoReply(QNetworkReply *reply);

private:
    QString       readWifiSsid() const;
    void          queryGeoClue();
    void          fallbackIpGeo();

    QTimer                  m_pollTimer;
    QTimer                  m_durationTimer;
    QNetworkAccessManager  *m_nam  = nullptr;
    UserId                  m_peer;
    Mode                    m_mode = Mode::Gps;
    double                  m_cachedLat  = 0.0;
    double                  m_cachedLon  = 0.0;
    bool                    m_hasGps     = false;
    bool                    m_geoClueActive = false;  // re-entrancy guard
    QDateTime               m_lastIpGeoAt;            // throttle ip-api.com requests
};

} // namespace SnapClone::Core
