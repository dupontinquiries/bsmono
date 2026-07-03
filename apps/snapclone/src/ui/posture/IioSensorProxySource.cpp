#include "ui/posture/IioSensorProxySource.hpp"

#include <QtDBus/QDBusConnection>
#include <QtDBus/QDBusConnectionInterface>
#include <QtDBus/QDBusInterface>
#include <QtDBus/QDBusReply>
#include <QVariantMap>
#include <QDir>
#include <QFile>
#include <QByteArray>
#include <cmath>

namespace SnapClone::UI {

namespace {
constexpr char kService[]   = "net.hadess.SensorProxy";
constexpr char kPath[]      = "/net/hadess/SensorProxy";
constexpr char kIface[]     = "net.hadess.SensorProxy";
constexpr char kPropsIface[] = "org.freedesktop.DBus.Properties";
constexpr int  kAccelPollMs = 250;
} // namespace

IioSensorProxySource::IioSensorProxySource(QObject *parent) : PostureSource(parent)
{
    m_accelTimer.setInterval(kAccelPollMs);
    connect(&m_accelTimer, &QTimer::timeout, this, &IioSensorProxySource::pollAccelerometer);
}

IioSensorProxySource::~IioSensorProxySource() { stop(); }

void IioSensorProxySource::start()
{
    if (m_started) return;
    m_started = true;

    QDBusConnection sys = QDBusConnection::systemBus();
    if (!sys.isConnected()) { m_proximityOk = false; return; }

    // Is the proxy even on the bus?
    if (auto *ci = sys.interface()) {
        if (!ci->isServiceRegistered(QString::fromLatin1(kService)).value()) {
            m_proximityOk = false;
            return;
        }
    }

    if (!m_proxy)
        m_proxy = new QDBusInterface(QString::fromLatin1(kService),
                                     QString::fromLatin1(kPath),
                                     QString::fromLatin1(kIface), sys, this);

    // Only proceed if the device actually has a proximity sensor.
    const QVariant has = m_proxy->property("HasProximity");
    if (has.isValid() && !has.toBool()) { m_proximityOk = false; return; }

    if (!claimProximity()) { m_proximityOk = false; return; }
    m_proximityOk = true;

    sys.connect(QString::fromLatin1(kService), QString::fromLatin1(kPath),
                QString::fromLatin1(kPropsIface), QStringLiteral("PropertiesChanged"),
                this, SLOT(onPropertiesChanged(QString, QVariantMap, QStringList)));

    readProximityNow();

    // Motion is a separate, optional best-effort track.
    m_accelBase = findAccelDevice();
    m_haveLast = false;
    if (!m_accelBase.isEmpty())
        m_accelTimer.start();
}

void IioSensorProxySource::stop()
{
    if (!m_started) return;
    m_started = false;
    m_accelTimer.stop();

    QDBusConnection sys = QDBusConnection::systemBus();
    sys.disconnect(QString::fromLatin1(kService), QString::fromLatin1(kPath),
                   QString::fromLatin1(kPropsIface), QStringLiteral("PropertiesChanged"),
                   this, SLOT(onPropertiesChanged(QString, QVariantMap, QStringList)));
    if (m_proxy && m_proximityOk)
        m_proxy->call(QStringLiteral("ReleaseProximity"));
    m_proximityOk = false;
}

bool IioSensorProxySource::claimProximity()
{
    if (!m_proxy) return false;
    QDBusReply<void> reply = m_proxy->call(QStringLiteral("ClaimProximity"));
    return reply.isValid();
}

void IioSensorProxySource::readProximityNow()
{
    if (!m_proxy) return;
    const QVariant v = m_proxy->property("ProximityNear");
    if (!v.isValid()) return;
    const bool near = v.toBool();
    if (near != m_near) { m_near = near; Q_EMIT proximityChanged(m_near); }
}

void IioSensorProxySource::onPropertiesChanged(const QString &iface,
                                               const QVariantMap &changed,
                                               const QStringList &)
{
    if (iface != QString::fromLatin1(kIface)) return;
    const auto it = changed.constFind(QStringLiteral("ProximityNear"));
    if (it == changed.constEnd()) return;
    const bool near = it.value().toBool();
    if (near != m_near) { m_near = near; Q_EMIT proximityChanged(m_near); }
}

// ── motion: best-effort raw accelerometer polling ─────────────────────────────

QString IioSensorProxySource::findAccelDevice() const
{
    QDir devs(QStringLiteral("/sys/bus/iio/devices"));
    const auto entries = devs.entryList(QStringList() << QStringLiteral("iio:device*"),
                                        QDir::Dirs | QDir::NoDotAndDotDot);
    for (const QString &e : entries) {
        const QString base = devs.absoluteFilePath(e);
        if (QFile::exists(base + QStringLiteral("/in_accel_x_raw")))
            return base;
    }
    return QString();
}

bool IioSensorProxySource::readAccel(double &x, double &y, double &z) const
{
    auto rd = [&](const char *axis, double &out) -> bool {
        QFile f(m_accelBase + QStringLiteral("/in_accel_") + QLatin1String(axis)
                + QStringLiteral("_raw"));
        if (!f.open(QIODevice::ReadOnly | QIODevice::Text)) return false;
        bool ok = false;
        out = f.readAll().trimmed().toDouble(&ok);
        return ok;
    };
    return rd("x", x) && rd("y", y) && rd("z", z);
}

void IioSensorProxySource::pollAccelerometer()
{
    double x, y, z;
    if (!readAccel(x, y, z)) return;
    if (!m_haveLast) { m_lastX = x; m_lastY = y; m_lastZ = z; m_haveLast = true; return; }
    const double d = std::sqrt((x - m_lastX) * (x - m_lastX)
                             + (y - m_lastY) * (y - m_lastY)
                             + (z - m_lastZ) * (z - m_lastZ));
    m_lastX = x; m_lastY = y; m_lastZ = z;
    const bool moving = (d > m_motionThreshold);
    if (moving != m_moving) { m_moving = moving; Q_EMIT motionChanged(m_moving); }
}

} // namespace SnapClone::UI
