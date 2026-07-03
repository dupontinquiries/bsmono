#pragma once
#include <QString>
#include <QDateTime>
#include "core/Types.hpp"

// ─── SnapClone location types ─────────────────────────────────────────────────
// Sent as LOCA messages over the existing WebSocket channel.
// GPS coordinates are optional — WiFi SSID alone is valid.

namespace SnapClone::Core {

struct LocationFix {
    UserId      senderId;
    double      lat       = 0.0;  // degrees, 0.0 if unavailable
    double      lon       = 0.0;
    float       accuracy  = -1.f; // metres, <0 if unavailable
    QString     ssid;             // connected WiFi SSID, empty if unavailable
    bool        hasGps    = false;
    QDateTime   timestamp;

    bool valid() const { return hasGps || !ssid.isEmpty(); }
};

/// Haversine great-circle distance in kilometres.
inline double haversineKm(double lat1, double lon1, double lat2, double lon2)
{
    constexpr double R  = 6371.0;
    constexpr double PI = 3.14159265358979323846;
    const double dLat = (lat2 - lat1) * PI / 180.0;
    const double dLon = (lon2 - lon1) * PI / 180.0;
    const double a = std::sin(dLat/2)*std::sin(dLat/2)
                   + std::cos(lat1*PI/180)*std::cos(lat2*PI/180)
                   * std::sin(dLon/2)*std::sin(dLon/2);
    return R * 2.0 * std::atan2(std::sqrt(a), std::sqrt(1.0-a));
}

} // namespace SnapClone::Core
