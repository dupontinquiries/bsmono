// ─── Core/AppContext.cpp ─────────────────────────────────────────────────────
//
// Implementation of AppContextProvider. See header for design notes.
//
// Key invariants:
//   • m_ctx is only written from the main thread (Qt object thread affinity).
//   • Every write goes through applyIfChanged() so the contextChanged
//     signal only fires on real transitions — keeps subscriber slots cheap.
//   • The "did anything change" check is field-by-field, NOT operator==,
//     because comparing two QDateTimes by equality is brittle (subsecond
//     timestamps). We treat recent-event timestamps as "if the new one is
//     newer than the old one, it changed."
// ─────────────────────────────────────────────────────────────────────────────

#include "core/AppContext.hpp"
#include "core/PowerManager.hpp"

#include <QGuiApplication>
#include <QNetworkInformation>
#include <QLoggingCategory>

Q_LOGGING_CATEGORY(lcAppCtx, "snapclone.appctx")

namespace SnapClone::Core {

namespace {

AppContext::ConnectionType mapTransport(QNetworkInformation::TransportMedium t)
{
    switch (t) {
    case QNetworkInformation::TransportMedium::Ethernet:
        return AppContext::ConnectionType::Ethernet;
    case QNetworkInformation::TransportMedium::WiFi:
        return AppContext::ConnectionType::Wifi;
    case QNetworkInformation::TransportMedium::Cellular:
        return AppContext::ConnectionType::Cellular;
    case QNetworkInformation::TransportMedium::Bluetooth:
        return AppContext::ConnectionType::Bluetooth;
    case QNetworkInformation::TransportMedium::Unknown:
    default:
        return AppContext::ConnectionType::Unknown;
    }
}

const char *connectionTypeName(AppContext::ConnectionType t)
{
    switch (t) {
    case AppContext::ConnectionType::Ethernet:  return "ethernet";
    case AppContext::ConnectionType::Wifi:      return "wifi";
    case AppContext::ConnectionType::Cellular:  return "cellular";
    case AppContext::ConnectionType::Bluetooth: return "bluetooth";
    case AppContext::ConnectionType::Other:     return "other";
    case AppContext::ConnectionType::None:      return "none";
    case AppContext::ConnectionType::Unknown:   break;
    }
    return "unknown";
}

} // anon

AppContextProvider::AppContextProvider(QObject *parent)
    : QObject(parent)
{
    // Initial defaults — appActive=true / screenOn=true are optimistic.
    // The hookup methods below replace these with real values as soon
    // as they're called. Until then, consumers see the optimistic
    // defaults, which is the safe direction (they'll be normally
    // aggressive at startup, which is what the user wants right after
    // launching the app anyway).
}

AppContextProvider::~AppContextProvider() = default;

// ─── Hookups ─────────────────────────────────────────────────────────────────

void AppContextProvider::attachPowerManager(PowerManager *pm)
{
    if (!pm) return;
    // Seed with current values so callers don't see stale defaults until
    // the first signal fires.
    AppContext next = m_ctx;
    next.screenOn   = pm->isScreenActive();
    next.onAc       = pm->isOnAc();
    next.powerSaver = pm->isBatterySaver()
                   || pm->powerProfile() == PowerProfile::PowerSaver;
    applyIfChanged(next);

    connect(pm, &PowerManager::screenActiveChanged,
            this, [this](bool active) {
        AppContext n = m_ctx;
        n.screenOn = active;
        applyIfChanged(n);
    });
    connect(pm, &PowerManager::onAcChanged,
            this, [this](bool onAc) {
        AppContext n = m_ctx;
        n.onAc = onAc;
        applyIfChanged(n);
    });
    connect(pm, &PowerManager::powerProfileChanged,
            this, [this, pm](PowerProfile p) {
        AppContext n = m_ctx;
        n.powerSaver = (p == PowerProfile::PowerSaver) || pm->isBatterySaver();
        applyIfChanged(n);
    });
    connect(pm, &PowerManager::batteryLevelChanged,
            this, [this, pm](int /*pct*/) {
        // batteryLevelChanged doesn't directly change powerSaver, but
        // isBatterySaver() depends on the pct threshold, so re-derive.
        AppContext n = m_ctx;
        n.powerSaver = pm->isBatterySaver()
                    || pm->powerProfile() == PowerProfile::PowerSaver;
        applyIfChanged(n);
    });
}

void AppContextProvider::attachApplicationState()
{
    auto *qg = QGuiApplication::instance();
    if (!qg) return;
    // Seed once.
    AppContext next = m_ctx;
    next.appActive =
        (QGuiApplication::applicationState() == Qt::ApplicationActive);
    applyIfChanged(next);

    // QGuiApplication exposes applicationStateChanged on its instance. We
    // cast through QGuiApplication* (the static cast is required because
    // the signal is declared on QGuiApplication, not QCoreApplication).
    auto *gui = qobject_cast<QGuiApplication *>(qg);
    if (!gui) return;
    connect(gui, &QGuiApplication::applicationStateChanged,
            this, [this](Qt::ApplicationState s) {
        AppContext n = m_ctx;
        n.appActive = (s == Qt::ApplicationActive);
        applyIfChanged(n);
    });
}

void AppContextProvider::attachNetworkInformation()
{
    auto *ni = QNetworkInformation::instance();
    if (!ni) {
        qCInfo(lcAppCtx) << "QNetworkInformation not loaded — transport "
                            "medium / metered fields will stay at defaults";
        return;
    }

    // Seed.
    AppContext next = m_ctx;
    if (ni->supports(QNetworkInformation::Feature::TransportMedium)) {
        next.connectionType = mapTransport(ni->transportMedium());
    }
    if (ni->supports(QNetworkInformation::Feature::Metered)) {
        next.metered = ni->isMetered();
    }
    applyIfChanged(next);

    if (ni->supports(QNetworkInformation::Feature::TransportMedium)) {
        connect(ni, &QNetworkInformation::transportMediumChanged,
                this, [this](QNetworkInformation::TransportMedium t) {
            AppContext n = m_ctx;
            n.connectionType = mapTransport(t);
            applyIfChanged(n);
        });
    }
    if (ni->supports(QNetworkInformation::Feature::Metered)) {
        connect(ni, &QNetworkInformation::isMeteredChanged,
                this, [this](bool m) {
            AppContext n = m_ctx;
            n.metered = m;
            applyIfChanged(n);
        });
    }
    // Reachability is already tracked by AppController and forwarded via
    // noteReachabilityFlap; we don't double-listen here.
}

// ─── Manual notes ────────────────────────────────────────────────────────────

void AppContextProvider::setCurrentPage(const QString &page)
{
    if (m_ctx.currentPage == page) return;
    AppContext next = m_ctx;
    next.currentPage = page;
    applyIfChanged(next);
}

void AppContextProvider::noteUserAction()
{
    // Always update — recency is the signal, so even if we already noted
    // a user action 50 ms ago, refreshing to "100 ms ago" keeps the
    // userJustActed window alive. Skip the emit if literally no time
    // has passed (rapid burst of camera-shutter taps in the same ms).
    const auto now = QDateTime::currentDateTimeUtc();
    if (m_ctx.lastUserAction == now) return;
    AppContext next = m_ctx;
    next.lastUserAction = now;
    applyIfChanged(next);
}

void AppContextProvider::noteSsidChange()
{
    AppContext next = m_ctx;
    next.lastSsidChange = QDateTime::currentDateTimeUtc();
    applyIfChanged(next);
}

void AppContextProvider::noteReachabilityFlap()
{
    AppContext next = m_ctx;
    next.lastReachabilityFlap = QDateTime::currentDateTimeUtc();
    applyIfChanged(next);
}

// ─── Apply + emit ────────────────────────────────────────────────────────────

void AppContextProvider::applyIfChanged(const AppContext &next)
{
    // Field-by-field comparison; cheaper to write out than to operator==
    // a struct with QDateTime members (which involves multiple level-2
    // comparisons internally). Bail early on the most-likely-to-differ
    // fields so the common no-change path is fast.
    if (next.appActive == m_ctx.appActive
     && next.screenOn == m_ctx.screenOn
     && next.currentPage == m_ctx.currentPage
     && next.connectionType == m_ctx.connectionType
     && next.metered == m_ctx.metered
     && next.powerSaver == m_ctx.powerSaver
     && next.onAc == m_ctx.onAc
     && next.lastSsidChange == m_ctx.lastSsidChange
     && next.lastReachabilityFlap == m_ctx.lastReachabilityFlap
     && next.lastUserAction == m_ctx.lastUserAction) {
        return;
    }

    // Log transitions at Info level for the first few weeks of deploy
    // so we can see how the context evolves in real traces. Once
    // patterns are clear, demote to Debug.
    if (next.appActive != m_ctx.appActive)
        qCInfo(lcAppCtx) << "appActive ->" << next.appActive;
    if (next.screenOn != m_ctx.screenOn)
        qCInfo(lcAppCtx) << "screenOn ->" << next.screenOn;
    if (next.connectionType != m_ctx.connectionType)
        qCInfo(lcAppCtx) << "connectionType ->"
                         << connectionTypeName(next.connectionType);
    if (next.metered != m_ctx.metered)
        qCInfo(lcAppCtx) << "metered ->" << next.metered;
    if (next.powerSaver != m_ctx.powerSaver)
        qCInfo(lcAppCtx) << "powerSaver ->" << next.powerSaver;
    if (next.currentPage != m_ctx.currentPage)
        qCDebug(lcAppCtx) << "currentPage ->" << next.currentPage;

    m_ctx = next;
    emit contextChanged(m_ctx);
}

} // namespace SnapClone::Core
