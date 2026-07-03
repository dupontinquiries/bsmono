#include "core/PowerManager.hpp"
#include <QDBusConnection>
#include <QDBusInterface>
#include <QDBusReply>
#include <QDBusUnixFileDescriptor>
#include <QDBusMessage>
#include <QLoggingCategory>
#include <QTimer>
#include <QFile>
#include <QDir>
#include <unistd.h>

Q_LOGGING_CATEGORY(lcPower, "snapclone.power")

namespace SnapClone::Core {

PowerManager::PowerManager(QObject *parent)
    : QObject(parent)
{
    // /sys watchdog: 30-second coarse timer regardless of which D-Bus tier is live.
    // D-Bus signals handle instant updates; this catches anything they miss.
    m_watchdog = new QTimer(this);
    m_watchdog->setInterval(30000);
    m_watchdog->setTimerType(Qt::VeryCoarseTimer);
    connect(m_watchdog, &QTimer::timeout, this, &PowerManager::pollSys);
    m_watchdog->start();

    // Detect Plasma session once — avoids wasting time on D-Bus timeouts
    // when PowerDevil is not running (postmarketOS without KDE).
    const QString plasma = qEnvironmentVariable("PLASMA_PLATFORM");
    const QString xdgSt  = qEnvironmentVariable("XDG_SESSION_DESKTOP");
    m_isPlasma = plasma.startsWith(QLatin1String("phone"))
              || xdgSt.contains(QLatin1String("KDE"),    Qt::CaseInsensitive)
              || xdgSt.contains(QLatin1String("plasma"), Qt::CaseInsensitive);

    if (m_isPlasma) {
        subscribeKdeBattery();   // instant battery signals
        subscribePowerProfile(); // watch power-saver / balanced / performance
    }

    subscribeUpower();     // battery details, remaining time, AC adapter
    subscribeScreenState(); // screen-off detection (camera bridge pause)

    pollSys(); // initial read — state is known before first signal fires

    qCInfo(lcPower) << "PowerManager:"
        << "Plasma=" << m_isPlasma
        << "upower=" << m_upowerConnected
        << "bat="    << m_batteryPct << "%"
        << "AC="     << m_onAc
        << "profile=" << static_cast<int>(m_powerProfile)
        << "health="  << static_cast<int>(m_batteryHealth)
        << "cycles="  << m_cycleCount;
}

PowerManager::~PowerManager() { releaseInhibit(); }

// ── Session lifecycle ─────────────────────────────────────────────────────────

void PowerManager::onSessionStarted()
{
    if (++m_activeSessions == 1) {
        qCInfo(lcPower) << "P2P session started — acquiring suspend inhibit";
        tryInhibitSuspend();
    }
}

void PowerManager::onSessionEnded()
{
    if (m_activeSessions > 0 && --m_activeSessions == 0) {
        qCInfo(lcPower) << "All P2P sessions ended — releasing inhibit";
        releaseInhibit();
    }
}

// ── Tier 1: KDE PowerDevil ────────────────────────────────────────────────────

void PowerManager::subscribeKdeBattery()
{
    QDBusConnection bus = QDBusConnection::sessionBus();
    const QString svc = QStringLiteral("org.kde.Solid.PowerManagement");
    const QString obj = QStringLiteral("/org/kde/Solid/PowerManagement");
    const QString ifc = svc;

    bool ok = bus.connect(svc, obj, ifc,
        QStringLiteral("batteryRemainingTimeChanged"),
        this, SLOT(onKdeBatteryChanged()));
    ok |= bus.connect(svc, obj, ifc,
        QStringLiteral("chargingChanged"),
        this, SLOT(onKdeChargingChanged()));

    if (ok) qCInfo(lcPower) << "KDE PowerDevil battery signals connected";
    else    qCDebug(lcPower) << "KDE PowerDevil not available";
}

void PowerManager::subscribePowerProfile()
{
    // org.kde.Solid.PowerManagement exposes the active profile via
    // currentProfile property and profileChanged signal.
    // Profiles: "performance", "balanced", "power-saver" (string).
    QDBusConnection bus = QDBusConnection::sessionBus();
    bus.connect(
        QStringLiteral("org.kde.Solid.PowerManagement"),
        QStringLiteral("/org/kde/Solid/PowerManagement"),
        QStringLiteral("org.kde.Solid.PowerManagement"),
        QStringLiteral("profileChanged"),
        this, SLOT(onKdeProfileChanged()));

    // Read current profile immediately
    onKdeProfileChanged();
}

void PowerManager::onKdeProfileChanged()
{
    QDBusInterface kde(
        QStringLiteral("org.kde.Solid.PowerManagement"),
        QStringLiteral("/org/kde/Solid/PowerManagement"),
        QStringLiteral("org.kde.Solid.PowerManagement"),
        QDBusConnection::sessionBus());
    if (!kde.isValid()) return;

    const QString profile = kde.property("currentProfile").toString().toLower();
    PowerProfile newProfile = PowerProfile::Balanced;
    if (profile.contains(QLatin1String("performance")))
        newProfile = PowerProfile::Performance;
    else if (profile.contains(QLatin1String("power")) || profile.contains(QLatin1String("saver")))
        newProfile = PowerProfile::PowerSaver;

    if (newProfile != m_powerProfile) {
        m_powerProfile = newProfile;
        if (m_powerProfile == PowerProfile::PowerSaver)
            qCWarning(lcPower) << "Power-saver profile active —"
                               << "STUN/hole-punch performance may be reduced";
        else
            qCInfo(lcPower) << "Power profile:" << profile;
        emit powerProfileChanged(m_powerProfile);
    }
}

void PowerManager::onKdeBatteryChanged()   { pollBattery(); }
void PowerManager::onKdeChargingChanged()  { pollBattery(); }

// ── Tier 2: upower ────────────────────────────────────────────────────────────

void PowerManager::subscribeUpower()
{
    QDBusInterface upower(
        QStringLiteral("org.freedesktop.UPower"),
        QStringLiteral("/org/freedesktop/UPower"),
        QStringLiteral("org.freedesktop.UPower"),
        QDBusConnection::systemBus());

    if (!upower.isValid()) {
        qCDebug(lcPower) << "upower not available";
        return;
    }

    const QDBusReply<QList<QDBusObjectPath>> devReply =
        upower.call(QStringLiteral("EnumerateDevices"));
    if (!devReply.isValid()) return;

    for (const QDBusObjectPath &devPath : devReply.value()) {
        QDBusInterface dev(
            QStringLiteral("org.freedesktop.UPower"),
            devPath.path(),
            QStringLiteral("org.freedesktop.UPower.Device"),
            QDBusConnection::systemBus());

        const uint type = dev.property("Type").toUInt();

        // Type 1 = LinePower (AC adapter)
        if (type == 1) {
            const bool online = dev.property("Online").toBool();
            m_onAc = online;
            // Watch the AC adapter too
            QDBusConnection::systemBus().connect(
                QStringLiteral("org.freedesktop.UPower"),
                devPath.path(),
                QStringLiteral("org.freedesktop.DBus.Properties"),
                QStringLiteral("PropertiesChanged"),
                this, SLOT(onUpowerPropertiesChanged()));
        }

        // Type 2 = Battery
        if (type == 2 && m_upowerBatteryPath.isEmpty()) {
            const bool ok = QDBusConnection::systemBus().connect(
                QStringLiteral("org.freedesktop.UPower"),
                devPath.path(),
                QStringLiteral("org.freedesktop.DBus.Properties"),
                QStringLiteral("PropertiesChanged"),
                this, SLOT(onUpowerPropertiesChanged()));
            if (ok) {
                m_upowerConnected  = true;
                m_upowerBatteryPath = devPath.path();
                qCInfo(lcPower) << "upower battery:" << devPath.path();
                readUpowerBatteryDetails(); // initial rich read
            }
        }
    }
}

void PowerManager::readUpowerBatteryDetails()
{
    if (m_upowerBatteryPath.isEmpty()) return;

    QDBusInterface dev(
        QStringLiteral("org.freedesktop.UPower"),
        m_upowerBatteryPath,
        QStringLiteral("org.freedesktop.UPower.Device"),
        QDBusConnection::systemBus());
    if (!dev.isValid()) return;

    // Remaining time (seconds until empty)
    const int tte = dev.property("TimeToEmpty").toInt();
    if (tte != m_remainingSecs) {
        m_remainingSecs = tte;
        emit remainingTimeChanged(tte);
        if (tte > 0 && tte < 300 && !m_charging)
            qCWarning(lcPower) << "Battery: less than 5 minutes remaining!";
    }

    // Energy rate in Watts (positive = discharging, negative = charging)
    m_energyRateW = dev.property("EnergyRate").toFloat();

    // Battery health string: "Good", "Unknown", "Critical", etc.
    const QString healthStr = dev.property("BatteryLevel").toString();
    if (healthStr.isEmpty()) {
        // Older upower: read from /sys directly
    } else {
        const BatteryHealth h = healthFromString(healthStr);
        if (h != m_batteryHealth) {
            m_batteryHealth = h;
            emit batteryHealthChanged(h);
        }
    }
}

void PowerManager::onUpowerPropertiesChanged()
{
    pollBattery();
    readUpowerBatteryDetails();

    // Re-read AC adapter state from all LinePower nodes
    QDBusInterface upower(
        QStringLiteral("org.freedesktop.UPower"),
        QStringLiteral("/org/freedesktop/UPower"),
        QStringLiteral("org.freedesktop.UPower"),
        QDBusConnection::systemBus());
    if (!upower.isValid()) return;
    const QDBusReply<QList<QDBusObjectPath>> devReply =
        upower.call(QStringLiteral("EnumerateDevices"));
    if (!devReply.isValid()) return;
    for (const QDBusObjectPath &devPath : devReply.value()) {
        QDBusInterface dev(
            QStringLiteral("org.freedesktop.UPower"),
            devPath.path(),
            QStringLiteral("org.freedesktop.UPower.Device"),
            QDBusConnection::systemBus());
        if (dev.property("Type").toUInt() == 1) { // LinePower
            const bool online = dev.property("Online").toBool();
            if (online != m_onAc) {
                m_onAc = online;
                qCInfo(lcPower) << "AC adapter:" << (online ? "connected" : "removed");
                emit onAcChanged(online);
            }
            break;
        }
    }
}

// ── Screen-off detection ───────────────────────────────────────────────────────

void PowerManager::subscribeScreenState()
{
    // Method 1: org.kde.screensaver — Plasma / KDE
    // screensaverStateChanged(bool active) — true = screensaver on = screen off
    QDBusConnection::sessionBus().connect(
        QStringLiteral("org.kde.screensaver"),
        QStringLiteral("/ScreenSaver"),
        QStringLiteral("org.kde.screensaver"),
        QStringLiteral("screensaverStateChanged"),
        this,
        // Direct slot would need bool arg — use lambda via QDBusConnection trick:
        // Instead connect to a slot and read the state.
        SLOT(onLogindIdleHintChanged()));

    // Method 2: systemd-logind IdleHint — works on all systemd distros
    // IdleHintChanged(bool hint) — true = session went idle (screen off)
    QDBusConnection::systemBus().connect(
        QStringLiteral("org.freedesktop.login1"),
        QStringLiteral("/org/freedesktop/login1"),
        QStringLiteral("org.freedesktop.login1.Manager"),
        QStringLiteral("IdleHintChanged"),
        this, SLOT(onLogindIdleHintChanged()));

    qCDebug(lcPower) << "Screen-state signals subscribed";
}

void PowerManager::onLogindIdleHintChanged()
{
    // Read IdleHint from logind to determine screen state.
    // IdleHint=true means the session has gone idle (screensaver on or locked).
    QDBusInterface logind(
        QStringLiteral("org.freedesktop.login1"),
        QStringLiteral("/org/freedesktop/login1"),
        QStringLiteral("org.freedesktop.login1.Manager"),
        QDBusConnection::systemBus());

    bool idle = false;
    if (logind.isValid()) {
        idle = logind.property("IdleHint").toBool();
    } else {
        // Fallback: ask org.kde.screensaver
        QDBusInterface ss(
            QStringLiteral("org.kde.screensaver"),
            QStringLiteral("/ScreenSaver"),
            QStringLiteral("org.kde.screensaver"),
            QDBusConnection::sessionBus());
        if (ss.isValid()) {
            const QDBusReply<bool> reply = ss.call(QStringLiteral("GetActive"));
            if (reply.isValid()) idle = reply.value();
        }
    }

    const bool screenOn = !idle;
    if (screenOn != m_screenActive) {
        m_screenActive = screenOn;
        qCInfo(lcPower) << "Screen:" << (screenOn ? "on" : "OFF");
        emit screenActiveChanged(screenOn);
        // When screen turns off: slow down watchdog to save wakeups
        m_watchdog->setInterval(screenOn ? 30000 : 90000);
    }
}

// ── Suspend inhibition ────────────────────────────────────────────────────────

void PowerManager::tryInhibitSuspend()
{
    if (m_kdeInhibitCookie != 0 || m_logindInhibitFd >= 0) return;

    if (m_isPlasma) {
        QDBusInterface kde(
            QStringLiteral("org.kde.Solid.PowerManagement"),
            QStringLiteral("/org/kde/Solid/PowerManagement/PolicyAgent"),
            QStringLiteral("org.kde.Solid.PowerManagement.PolicyAgent"),
            QDBusConnection::sessionBus());
        if (kde.isValid()) {
            // Flags 7 = ChangeProfile(1) | InterruptSession(2) | SuspendSession(4)
            const QDBusReply<uint> reply = kde.call(
                QStringLiteral("AddInhibition"), 7u,
                QStringLiteral("SnapClone"),
                QStringLiteral("Active peer-to-peer session"));
            if (reply.isValid()) {
                m_kdeInhibitCookie = reply.value();
                qCInfo(lcPower) << "KDE inhibit acquired (cookie="
                                << m_kdeInhibitCookie << ", flags=7)";
                return;
            }
        }
    }

    QDBusInterface logind(
        QStringLiteral("org.freedesktop.login1"),
        QStringLiteral("/org/freedesktop/login1"),
        QStringLiteral("org.freedesktop.login1.Manager"),
        QDBusConnection::systemBus());
    if (logind.isValid()) {
        const QDBusReply<QDBusUnixFileDescriptor> reply = logind.call(
            QStringLiteral("Inhibit"),
            QStringLiteral("sleep:idle"),
            QStringLiteral("SnapClone"),
            QStringLiteral("Active P2P session in progress"),
            QStringLiteral("block"));
        if (reply.isValid()) {
            m_logindInhibitFd = ::dup(reply.value().fileDescriptor());
            qCInfo(lcPower) << "logind inhibit acquired (fd=" << m_logindInhibitFd << ")";
            return;
        }
    }

    qCWarning(lcPower) << "No suspend inhibition backend available";
}

void PowerManager::releaseInhibit()
{
    if (m_kdeInhibitCookie != 0) {
        QDBusInterface kde(
            QStringLiteral("org.kde.Solid.PowerManagement"),
            QStringLiteral("/org/kde/Solid/PowerManagement/PolicyAgent"),
            QStringLiteral("org.kde.Solid.PowerManagement.PolicyAgent"),
            QDBusConnection::sessionBus());
        kde.call(QStringLiteral("ReleaseInhibition"), m_kdeInhibitCookie);
        m_kdeInhibitCookie = 0;
        qCInfo(lcPower) << "KDE inhibit released";
    }
    if (m_logindInhibitFd >= 0) {
        ::close(m_logindInhibitFd);
        m_logindInhibitFd = -1;
        qCInfo(lcPower) << "logind inhibit released";
    }
}

// ── /sys polling ──────────────────────────────────────────────────────────────

void PowerManager::pollSys() { pollThermal(); pollBattery(); }

void PowerManager::pollThermal()
{
    float maxTemp = 0.f;
    const QDir thermalDir(QStringLiteral("/sys/class/thermal"));
    const QStringList zones = thermalDir.entryList(
        QStringList{QStringLiteral("thermal_zone*")}, QDir::Dirs);

    for (const QString &zone : zones) {
        const QString base = QStringLiteral("/sys/class/thermal/") + zone;
        QFile typeFile(base + QStringLiteral("/type"));
        QString zoneType;
        if (typeFile.open(QIODevice::ReadOnly))
            zoneType = QString::fromLatin1(typeFile.readAll()).trimmed().toLower();

        QFile tempFile(base + QStringLiteral("/temp"));
        if (!tempFile.open(QIODevice::ReadOnly)) continue;
        const float degC = tempFile.readAll().trimmed().toFloat() / 1000.f;
        if (degC <= 0.f || degC > 150.f) continue;

        const bool isCpuZone = zoneType.contains(QLatin1String("cpu"))
                            || zoneType.contains(QLatin1String("soc"))
                            || zoneType.contains(QLatin1String("tsens"))
                            || zoneType.contains(QLatin1String("gpu"))
                            || zoneType.contains(QLatin1String("skin"));
        if (isCpuZone || zones.size() == 1)
            maxTemp = qMax(maxTemp, degC);
        else if (maxTemp == 0.f)
            maxTemp = degC;
    }

    const ThermalState newState =
        (maxTemp <= 0.f) ? ThermalState::Unknown :
        (maxTemp < 45.f) ? ThermalState::Normal  :
        (maxTemp < 60.f) ? ThermalState::Warm    :
        (maxTemp < 75.f) ? ThermalState::Hot     : ThermalState::Critical;

    if (newState != m_thermalState) {
        m_thermalState = newState;
        qCInfo(lcPower) << "Thermal:" << maxTemp << "°C ->"
                        << static_cast<int>(m_thermalState);
        emit thermalStateChanged(m_thermalState);
        if (m_thermalState == ThermalState::Critical) emit thermalCritical();
        else if (m_thermalState == ThermalState::Hot) emit thermalHot();
    }
    m_lastTemp = maxTemp;
}

void PowerManager::pollBattery()
{
    const QDir psDir(QStringLiteral("/sys/class/power_supply"));
    const QStringList nodes = psDir.entryList(QDir::Dirs | QDir::NoDotAndDotDot);

    bool foundAc = false;
    for (const QString &node : nodes) {
        const QString base = QStringLiteral("/sys/class/power_supply/") + node;

        QFile typeFile(base + QStringLiteral("/type"));
        if (!typeFile.open(QIODevice::ReadOnly)) continue;
        const QByteArray typeRaw = typeFile.readAll().trimmed().toLower();

        // AC adapter detection via /sys
        if (typeRaw == "mains" || typeRaw == "usb") {
            QFile onlineFile(base + QStringLiteral("/online"));
            if (onlineFile.open(QIODevice::ReadOnly)) {
                const bool ac = onlineFile.readAll().trimmed() == "1";
                if (!foundAc) {
                    foundAc = true;
                    if (ac != m_onAc) {
                        m_onAc = ac;
                        emit onAcChanged(ac);
                    }
                }
            }
            continue;
        }

        if (typeRaw != "battery") continue;

        // Capacity
        QFile capFile(base + QStringLiteral("/capacity"));
        if (capFile.open(QIODevice::ReadOnly)) {
            const int pct = capFile.readAll().trimmed().toInt();
            if (pct != m_batteryPct) {
                const bool wasLow = (m_batteryPct <= 10);
                m_batteryPct = pct;
                emit batteryLevelChanged(pct);
                if (pct <= 10 && !wasLow) emit batteryLow();
                if (pct <= 5)             emit batteryCritical();
            }
        }

        // Status
        QFile statusFile(base + QStringLiteral("/status"));
        if (statusFile.open(QIODevice::ReadOnly)) {
            const QString status = QString::fromLatin1(statusFile.readAll()).trimmed();
            const bool charging = (status == QLatin1String("Charging")
                                || status == QLatin1String("Full"));
            if (charging != m_charging) {
                m_charging = charging;
                emit chargingChanged(charging);
            }
        }

        // Charge stop threshold (FP5: charge_control_end_threshold)
        QFile threshFile(base + QStringLiteral("/charge_control_end_threshold"));
        if (threshFile.open(QIODevice::ReadOnly)) {
            const int t = threshFile.readAll().trimmed().toInt();
            if (t > 0 && t != m_chargeThreshold) {
                m_chargeThreshold = t;
                qCInfo(lcPower) << "Charge stop threshold:" << t << "%";
                emit chargeThresholdChanged(t);
            }
        }

        // Cycle count (battery wear indicator — FP5, most modern phones)
        QFile cyclesFile(base + QStringLiteral("/cycle_count"));
        if (cyclesFile.open(QIODevice::ReadOnly)) {
            const int c = cyclesFile.readAll().trimmed().toInt();
            if (c > 0 && c != m_cycleCount) {
                m_cycleCount = c;
                qCInfo(lcPower) << "Battery cycle count:" << c;
            }
        }

        // Health string — Linux kernel exposes "Good", "Unknown",
        // "Overheat", "Dead", "Over voltage", "Unspecified failure", "Cold"
        QFile healthFile(base + QStringLiteral("/health"));
        if (healthFile.open(QIODevice::ReadOnly)) {
            const QString h = QString::fromLatin1(healthFile.readAll()).trimmed();
            const BatteryHealth bh = healthFromString(h);
            if (bh != m_batteryHealth) {
                m_batteryHealth = bh;
                if (bh == BatteryHealth::Poor || bh == BatteryHealth::Dead)
                    qCWarning(lcPower) << "Battery health:" << h;
                else
                    qCInfo(lcPower) << "Battery health:" << h;
                emit batteryHealthChanged(bh);
            }
        }

        break; // one battery node
    }
}

BatteryHealth PowerManager::healthFromString(const QString &s) const
{
    const QString lower = s.toLower();
    if (lower == QLatin1String("good"))    return BatteryHealth::Good;
    if (lower.contains(QLatin1String("dead")))        return BatteryHealth::Dead;
    if (lower.contains(QLatin1String("overheat"))
     || lower.contains(QLatin1String("cold"))
     || lower.contains(QLatin1String("failure"))
     || lower.contains(QLatin1String("voltage")))     return BatteryHealth::Poor;
    if (lower.contains(QLatin1String("warm")))        return BatteryHealth::Fair;
    return BatteryHealth::Unknown;
}

} // namespace SnapClone::Core
