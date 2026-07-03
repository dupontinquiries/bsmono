#pragma once
// ─── SleepState.hpp ───────────────────────────────────────────────────────────
//
// V32.34+ battery: a single coherent power/activity state machine that the
// whole app shares, instead of each component independently interpreting the
// raw screen / app-foreground / battery / charging signals.
//
// Before this, the conservation logic was scattered: AmbientBlobs subscribed
// to QGuiApplication::applicationStateChanged on its own, WayfinderPage did
// the same, CameraWidget subscribed to PowerManager::screenActiveChanged,
// DhtResourceManager had its own multi-signal evaluate(). Each interpreted
// "should I be doing expensive work right now?" slightly differently, and
// the policy was impossible to read in one place.
//
// SleepStateMachine folds the inputs (PowerManager screen/charge + AppContext
// foreground/idle) into ONE enum and emits ONE signal. Components subscribe
// to `sleepStateChanged` and switch on the state. The mapping from raw
// signals to state lives here and only here — auditable, testable, tunable.
//
// States (most → least active):
//
//   Active          app foreground, screen on, recent user interaction.
//                   Everything runs at full rate.
//   ForegroundIdle  app foreground, screen on, but no interaction for
//                   kIdleAfterMs. Drop animation framerates, pause the
//                   landmark scorer; keep networking Full (user is here).
//   Background      app NOT foreground (user switched apps). Screen may be
//                   on. Stop camera + ambient/animations; networking eases
//                   toward Maintenance.
//   ScreenOff       device screen off, regardless of foreground. The deep
//                   pocket-sleep state: stop camera (sensor power-down),
//                   stop all cosmetic timers, networking → Minimal/Off by
//                   battery. Only the mailbox heartbeat should survive.
//   Charging        on AC / charging. OVERRIDES the conservation bias —
//                   even screen-off we can afford to be a good network
//                   citizen (stay reachable). Distinct top-level state so
//                   consumers can special-case "plugged in" trivially.
//
// Charging precedence: if charging, we report Charging UNLESS the app is
// actively foreground+interacting (then Active is more useful to consumers
// that want full UI rate). i.e. Charging means "idle/backgrounded/asleep but
// plugged in — don't conserve aggressively."
//
// Transition debounce: rapid screen on/off (a notification peek, an
// accidental tap) shouldn't thrash expensive start/stop cycles (camera
// pipeline teardown+rebuild is ~250ms and visible). Transitions INTO a more
// conservative state are delayed by kDebounceMs; transitions toward MORE
// active fire immediately (responsiveness matters more than the tiny battery
// cost of waking early).

#include <QObject>
#include <QString>

QT_BEGIN_NAMESPACE
class QTimer;
QT_END_NAMESPACE

namespace SnapClone::Core {

class PowerManager;
class AppContextProvider;

enum class SleepState {
    Active,          ///< foreground, screen on, recent interaction
    ForegroundIdle,  ///< foreground, screen on, idle ≥ kIdleAfterMs
    Background,      ///< app not foreground
    ScreenOff,       ///< device screen off (deep sleep)
    Charging,        ///< plugged in + idle/backgrounded/asleep
};

/// Human-readable name for logs / debug HUD.
[[nodiscard]] const char *sleepStateName(SleepState s);

class SleepStateMachine : public QObject
{
    Q_OBJECT
public:
    /// Both inputs are non-owning and may be null (degrades gracefully:
    /// a null PowerManager means we never see screen-off/charging and lean
    /// on AppContext alone; a null AppContext means we lean on PowerManager
    /// alone). Parent owns lifetime.
    explicit SleepStateMachine(PowerManager *power,
                               AppContextProvider *appContext,
                               QObject *parent = nullptr);
    ~SleepStateMachine() override;

    [[nodiscard]] SleepState state() const { return m_state; }

    /// True if expensive cosmetic work (animations, ambient derive,
    /// landmark scoring) should run. Convenience for the common consumer
    /// check — equivalent to state == Active.
    [[nodiscard]] bool allowsCosmeticWork() const { return m_state == SleepState::Active; }

    /// True if the camera pipeline should be allowed to run (page-visibility
    /// is still the component's own call; this is the power gate on top).
    /// False in Background and ScreenOff.
    [[nodiscard]] bool allowsCamera() const {
        return m_state == SleepState::Active
            || m_state == SleepState::ForegroundIdle
            || m_state == SleepState::Charging;
    }

    // Tunables (ms). Exposed for tests; not expected to be changed at runtime.
    static constexpr int kIdleAfterMs  = 30'000; ///< Active → ForegroundIdle
    static constexpr int kDebounceMs   = 2'500;  ///< delay into a more-conservative state

signals:
    /// Emitted only when the state actually changes (after debounce).
    void sleepStateChanged(SnapClone::Core::SleepState state);

private slots:
    void reevaluate();        ///< recompute target from inputs (no debounce here)
    void onIdleTimer();       ///< Active → ForegroundIdle when interaction lapses

private:
    /// Compute the target state purely from current inputs.
    [[nodiscard]] SleepState computeTarget() const;
    /// Apply a target through the debounce policy (immediate toward Active,
    /// delayed toward conservation).
    void applyTarget(SleepState target);
    /// Rank states by activity level (higher = more active) for the
    /// "toward active fires immediately" debounce decision.
    static int activityRank(SleepState s);

    PowerManager        *m_power;
    AppContextProvider  *m_appContext;
    SleepState           m_state        = SleepState::Active;
    SleepState           m_pendingTarget = SleepState::Active;
    QTimer              *m_debounceTimer = nullptr; ///< single-shot, into-conservative
    QTimer              *m_idleTimer     = nullptr; ///< Active → ForegroundIdle
};

} // namespace SnapClone::Core
