#pragma once
// ─── ui/posture/IioSensorProxySource.hpp ──────────────────────────────────────
//
// The real PostureSource for postmarketOS / FP5. Proximity comes from
// iio-sensor-proxy over the system bus (net.hadess.SensorProxy: ClaimProximity +
// the ProximityNear property, watched via PropertiesChanged) — the standard,
// well-defined path. Motion is best-effort: we poll raw IIO accelerometer counts
// from sysfs and call it "moving" when the sample-to-sample delta crosses a
// threshold.
//
// EVERYTHING here degrades safely. If the sensor proxy isn't running, or the
// device has no proximity sensor, available() reports false and the engine treats
// the world as far + still (i.e. behaves completely normally — we never blank a
// screen we can't justify). If no accelerometer is found, motion simply stays
// "still", which only means the screensaver won't be interrupted by movement.
//
// ⚠ UNVERIFIED ON HARDWARE. Whether the FP5 exposes proximity through
// iio-sensor-proxy, the exact IIO accel device path, and the motion threshold all
// need on-device validation. The graceful-degradation contract above is what
// makes it safe to ship before that validation.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/posture/PostureSource.hpp"
#include <QString>
#include <QTimer>

class QDBusInterface;

namespace SnapClone::UI {

class IioSensorProxySource : public PostureSource {
    Q_OBJECT
public:
    explicit IioSensorProxySource(QObject *parent = nullptr);
    ~IioSensorProxySource() override;

    void start() override;
    void stop()  override;
    [[nodiscard]] bool available() const override { return m_proximityOk; }

    /// Sample-to-sample raw-count delta above which the device counts as moving.
    /// Device-specific; needs on-device tuning.
    void setMotionThreshold(double t) { m_motionThreshold = t; }

private Q_SLOTS:
    // Matches org.freedesktop.DBus.Properties.PropertiesChanged.
    void onPropertiesChanged(const QString &iface,
                             const QVariantMap &changed,
                             const QStringList &invalidated);
    void pollAccelerometer();

private:
    bool claimProximity();
    void readProximityNow();
    QString findAccelDevice() const;       // sysfs base dir, or empty
    bool    readAccel(double &x, double &y, double &z) const;

    QDBusInterface *m_proxy = nullptr;
    bool   m_proximityOk = false;          // claimed + readable
    bool   m_near        = false;
    bool   m_started     = false;

    QTimer m_accelTimer;
    QString m_accelBase;                   // e.g. /sys/bus/iio/devices/iio:device0
    bool    m_haveLast = false;
    double  m_lastX = 0, m_lastY = 0, m_lastZ = 0;
    bool    m_moving = false;
    double  m_motionThreshold = 120.0;     // raw counts; tune on device
};

} // namespace SnapClone::UI
