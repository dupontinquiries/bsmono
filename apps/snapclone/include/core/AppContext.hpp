#pragma once

// ─── Core/AppContext.hpp ─────────────────────────────────────────────────────
//
// V32.21: app-state context for adaptive network decisions.
//
// The network FSM (FSMValidator + per-peer Phase machine) historically saw
// only protocol-level signals: which frame arrived, which phase the peer
// is in, did the timer fire. It did NOT see the broader app state that
// SHOULD inform retry timing, punch aggression, reconnect immediacy:
//
//   • Is the user actively using the app right now, or did they pocket
//     their phone five minutes ago?
//   • Did the user just hit the shutter / send button — meaning they're
//     *expecting* a snappy delivery?
//   • Are we on cellular vs WiFi vs ethernet?
//   • Is the network metered (don't burn data on bulky retries)?
//   • Is the system in power-saver mode (don't burn battery on probes)?
//   • Did we just hop SSIDs (recent reachability flap — back off until
//     things settle)?
//
// AppContext is the snapshot struct carrying all of this. AppContextProvider
// is the singleton-flavoured QObject that listens to Qt's system signals
// plus our own PowerManager / CameraWidget / send-button events and emits
// `contextChanged(AppContext)` whenever the snapshot meaningfully changes.
//
// Consumers (NatTraversal punch loop, UdpChannel retransmit timer,
// AppController reconnect scheduler, DHT bootstrap retry) hold a const
// reference or subscribe to the signal and adjust their own backoff /
// aggression accordingly. The provider does NOT make decisions itself —
// it just observes and broadcasts. Consumers retain authority over their
// own behaviour, but with a richer view of the world.
//
// Design notes:
//   • AppContext is a value-type struct, not a QObject. Copyable, cheap.
//     Fits in a Qt signal emission without overhead.
//   • The provider lives in core/ (not network/) because it touches
//     PowerManager + QGuiApplication state, which neither network nor
//     UI layers own outright.
//   • Recent-event flags (recentSsidChangeMs, userJustActedMs) are
//     stored as a "milliseconds since this happened" rolling counter,
//     not raw timestamps — callers can ask `userJustActedRecently(2000)`
//     without doing wall-clock arithmetic themselves.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QDateTime>
#include <QString>

namespace SnapClone::Core {

// ─── Snapshot of all the inputs we care about ────────────────────────────────
//
// "Recent" timestamps are stored as the QDateTime of the event, not the age.
// The provider only updates these on real transitions, so a stale field is
// safe to read — it just means "this happened long ago." Use the helpers
// at the bottom to ask "did X happen in the last N ms".

struct AppContext {

    // ── Foreground / visibility ───────────────────────────────────────────
    /// True if the application has a visible, active window. Tracks
    /// QGuiApplication::applicationState ∈ {ApplicationActive}.
    bool appActive = true;

    /// True if the system screen is on / unlocked. Mirrors
    /// PowerManager::isScreenActive().
    bool screenOn  = true;

    /// Identifier of the currently-visible page in the app (one of
    /// "camera", "inbox", "friends", "wayfinder", "settings", or "" for
    /// startup / unknown). Set by MainWindow when SwipeNavigator changes
    /// pages. Useful so e.g. the camera retry loop can be more eager
    /// while the camera page is visible, less when the user has swiped
    /// to settings.
    QString currentPage;

    // ── Network type ──────────────────────────────────────────────────────
    enum class ConnectionType {
        Unknown,
        Ethernet,
        Wifi,
        Cellular,
        Bluetooth,
        Other,
        None,           ///< reachable=offline, no transport
    };
    ConnectionType connectionType = ConnectionType::Unknown;

    /// True if the OS reports the connection as metered (cellular data,
    /// hotspot, capped plan). When true: be stingy with retries, suppress
    /// background DHT probing, prefer relay-cached paths.
    bool metered = false;

    // ── Power state ───────────────────────────────────────────────────────
    /// True if the system is in any kind of power-saver mode. Mirrors
    /// PowerManager::isBatterySaver() OR PowerProfile::PowerSaver.
    bool powerSaver = false;

    /// True if the device is plugged in to AC. Lets us be more aggressive
    /// with discovery work (DHT bootstrap probes, gossip multicast bursts)
    /// since we don't care about battery drain.
    bool onAc = false;

    // ── Recent event timestamps ───────────────────────────────────────────
    /// When the SSID last changed (Wi-Fi network switch detected). Default-
    /// constructed QDateTime means "never seen one". A switch typically
    /// produces a flurry of reachability flaps as DHCP runs; consumers can
    /// back off for the first ~5s after a switch.
    QDateTime lastSsidChange;

    /// When network reachability last flipped (online ↔ offline). Even
    /// without a full SSID change, reachability flapping (e.g. weak
    /// signal at the edge of coverage) tells us the network is unstable
    /// and aggressive probing won't help.
    QDateTime lastReachabilityFlap;

    /// When the user last performed an action that implies they want
    /// snappy delivery: pressed shutter, hit send, opened a snap.
    /// Consumers can use this to push past their normal exponential
    /// backoff for the next few seconds.
    QDateTime lastUserAction;

    // ── Helpers ───────────────────────────────────────────────────────────
    /// True if `lastUserAction` is set and within `ms` of now.
    [[nodiscard]] bool userJustActed(int ms) const {
        if (!lastUserAction.isValid()) return false;
        return lastUserAction.msecsTo(QDateTime::currentDateTimeUtc()) < ms;
    }

    /// True if `lastSsidChange` was within `ms` of now. Useful for
    /// "did we just hop networks → probably DHCP is still settling".
    [[nodiscard]] bool ssidJustChanged(int ms) const {
        if (!lastSsidChange.isValid()) return false;
        return lastSsidChange.msecsTo(QDateTime::currentDateTimeUtc()) < ms;
    }

    /// True if `lastReachabilityFlap` was within `ms` of now.
    [[nodiscard]] bool reachabilityRecentlyFlapped(int ms) const {
        if (!lastReachabilityFlap.isValid()) return false;
        return lastReachabilityFlap.msecsTo(QDateTime::currentDateTimeUtc()) < ms;
    }

    /// True if the app is in a "background and the user doesn't care
    /// right now" state. The strongest signal to back off everything
    /// except keepalives. Doesn't itself include `powerSaver` because
    /// power saver is a stronger demand for thrift.
    [[nodiscard]] bool isIdle() const {
        return !appActive || !screenOn;
    }

    /// Convenience: combined "be conservative" predicate. True when
    /// the user isn't watching AND we'd burn data/battery for it.
    [[nodiscard]] bool prefersConservative() const {
        return isIdle() && (metered || powerSaver
                            || connectionType == ConnectionType::Cellular);
    }
};

// ─── Provider — owns the snapshot, broadcasts changes ────────────────────────
//
// Single instance, owned by AppController (lives as long as the app does).
// Hooked up to the rest of the app via `attachPowerManager` /
// `setCurrentPage` / `noteUserAction` / `noteSsidChange` /
// `noteReachabilityFlap` — explicit calls rather than auto-discovery so the
// data flow is grep-able.
//
// Why no signal-listening on PowerManager directly inside the provider?
// Because the alternative (provider connects to PowerManager signals) would
// require PowerManager to be constructed before the provider, hard-coding
// init order. The attach pattern lets either be built first; AppController
// just wires them at the right moment in initialize().

class PowerManager;

class AppContextProvider : public QObject
{
    Q_OBJECT

public:
    explicit AppContextProvider(QObject *parent = nullptr);
    ~AppContextProvider() override;

    /// Current snapshot. Cheap; the struct is just POD-ish data.
    [[nodiscard]] const AppContext &context() const { return m_ctx; }

    // ── Hookups ───────────────────────────────────────────────────────────
    /// Attach to a PowerManager so power/screen/profile changes update the
    /// snapshot. Safe to call once after both objects exist. Does not take
    /// ownership.
    void attachPowerManager(PowerManager *pm);

    /// Begin listening to Qt application-state changes (foreground/background).
    /// Called from AppController::initialize after QGuiApplication exists.
    void attachApplicationState();

    /// Begin listening to QNetworkInformation for transport-medium / metered
    /// changes. Safe to call even if the backend isn't loaded — the call is
    /// a no-op in that case.
    void attachNetworkInformation();

    // ── Manual notes from elsewhere in the app ────────────────────────────
    /// MainWindow calls this when SwipeNavigator changes pages.
    void setCurrentPage(const QString &page);

    /// Anyone with reason to know the user just did something snappable
    /// (camera shutter, send tap, snap open) calls this. Cheap; just
    /// updates a timestamp.
    void noteUserAction();

    /// AppController already detects SSID changes via its IP-changed
    /// poller — forward those to us so the context tracks them too.
    void noteSsidChange();

    /// AppController detects reachability flaps via QNetworkInformation
    /// (and the poll timer fallback). Forward.
    void noteReachabilityFlap();

signals:
    /// Emitted whenever the snapshot meaningfully changes. Consumers can
    /// either re-read `context()` on each emit or wire to this signal
    /// directly and use the parameter.
    ///
    /// "Meaningfully" means: a field actually flipped. We don't emit on
    /// every redundant set (avoids signal storms from PowerManager's
    /// chatty pollers reporting unchanged values).
    void contextChanged(const SnapClone::Core::AppContext &ctx);

private:
    /// Helper: compare `next` to `m_ctx`, swap if changed, emit signal.
    /// Saves us from sprinkling change-check logic at every update site.
    void applyIfChanged(const AppContext &next);

    AppContext m_ctx;
};

} // namespace SnapClone::Core
