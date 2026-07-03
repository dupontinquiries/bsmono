#include "core/SleepState.hpp"
#include "core/PowerManager.hpp"
#include "core/AppContext.hpp"

#include <QTimer>
#include <QLoggingCategory>
#include <QMetaType>

namespace SnapClone::Core {

Q_LOGGING_CATEGORY(lcSleep, "snapclone.sleepstate")

namespace {
// Register SleepState as a Qt metatype so it can cross queued connections
// (e.g. if a future consumer lives on another thread). Direct connections
// don't need this, but it's free insurance and harmless if redundant.
const bool kSleepStateRegistered = []() {
    qRegisterMetaType<SnapClone::Core::SleepState>("SnapClone::Core::SleepState");
    return true;
}();
} // namespace

const char *sleepStateName(SleepState s)
{
    switch (s) {
    case SleepState::Active:         return "Active";
    case SleepState::ForegroundIdle: return "ForegroundIdle";
    case SleepState::Background:     return "Background";
    case SleepState::ScreenOff:      return "ScreenOff";
    case SleepState::Charging:       return "Charging";
    }
    return "?";
}

SleepStateMachine::SleepStateMachine(PowerManager *power,
                                     AppContextProvider *appContext,
                                     QObject *parent)
    : QObject(parent)
    , m_power(power)
    , m_appContext(appContext)
{
    // Debounce timer: single-shot. When a transition INTO a more
    // conservative state is requested, we arm this; if the inputs are still
    // pointing at that target when it fires, we commit. A transition back
    // toward Active before it fires cancels it (applyTarget restarts/stops).
    m_debounceTimer = new QTimer(this);
    m_debounceTimer->setSingleShot(true);
    m_debounceTimer->setInterval(kDebounceMs);
    connect(m_debounceTimer, &QTimer::timeout, this, [this]() {
        // Commit whatever target we were debouncing, IF it's still the
        // computed target (inputs may have changed during the wait).
        const SleepState nowTarget = computeTarget();
        if (nowTarget == m_pendingTarget && nowTarget != m_state) {
            const SleepState prev = m_state;
            m_state = nowTarget;
            qCInfo(lcSleep) << "→" << sleepStateName(m_state)
                            << "(from" << sleepStateName(prev) << ", debounced)";
            emit sleepStateChanged(m_state);
        }
    });

    // Idle timer: single-shot, armed whenever we enter Active. On timeout
    // (no user action within kIdleAfterMs) we re-evaluate, which will pick
    // ForegroundIdle if still foreground+screen-on.
    m_idleTimer = new QTimer(this);
    m_idleTimer->setSingleShot(true);
    m_idleTimer->setInterval(kIdleAfterMs);
    connect(m_idleTimer, &QTimer::timeout, this, &SleepStateMachine::onIdleTimer);

    // Subscribe to the inputs.
    if (m_power) {
        connect(m_power, &PowerManager::screenActiveChanged,
                this, &SleepStateMachine::reevaluate);
        connect(m_power, &PowerManager::chargingChanged,
                this, &SleepStateMachine::reevaluate);
        connect(m_power, &PowerManager::onAcChanged,
                this, &SleepStateMachine::reevaluate);
    }
    if (m_appContext) {
        connect(m_appContext, &AppContextProvider::contextChanged,
                this, [this](const AppContext &) { reevaluate(); });
    }

    // Seed the initial state from current inputs without debounce — at
    // construction there's nothing to debounce against.
    m_state = computeTarget();
    m_pendingTarget = m_state;
    if (m_state == SleepState::Active) m_idleTimer->start();
    qCInfo(lcSleep) << "initial state" << sleepStateName(m_state);
}

SleepStateMachine::~SleepStateMachine() = default;

int SleepStateMachine::activityRank(SleepState s)
{
    // Higher = more active. Used only for the debounce direction decision.
    // Charging sits above Background/ScreenOff (we're being LESS conservative
    // than those) but below the foreground states.
    switch (s) {
    case SleepState::Active:         return 4;
    case SleepState::ForegroundIdle: return 3;
    case SleepState::Charging:       return 2;
    case SleepState::Background:     return 1;
    case SleepState::ScreenOff:      return 0;
    }
    return 0;
}

SleepState SleepStateMachine::computeTarget() const
{
    // Gather raw inputs, preferring PowerManager for screen/charge (the
    // authoritative hardware source) and AppContext for foreground/idle.
    bool screenOn   = true;
    bool charging   = false;
    if (m_power) {
        screenOn = m_power->isScreenActive();
        charging = m_power->isCharging() || m_power->isOnAc();
    }
    bool appActive  = true;
    bool justActed  = true;   // assume recent interaction if we can't tell
    if (m_appContext) {
        const AppContext &ctx = m_appContext->context();
        appActive = ctx.appActive;
        // If PowerManager is absent, fall back to AppContext's screen mirror.
        if (!m_power) screenOn = ctx.screenOn;
        if (!m_power) charging = ctx.onAc;
        justActed = ctx.userJustActed(kIdleAfterMs);
    }

    // Screen off is the deepest, dominant signal — UNLESS charging, in which
    // case we report Charging (don't aggressively conserve while plugged in).
    if (!screenOn) {
        return charging ? SleepState::Charging : SleepState::ScreenOff;
    }
    // Screen on but app not foreground → Background (or Charging if plugged).
    if (!appActive) {
        return charging ? SleepState::Charging : SleepState::Background;
    }
    // Foreground + screen on: Active if recent interaction, else ForegroundIdle.
    // (Charging does NOT override the foreground states — a plugged-in user
    // actively using the app should still get full Active UI rate.)
    return justActed ? SleepState::Active : SleepState::ForegroundIdle;
}

void SleepStateMachine::reevaluate()
{
    const SleepState target = computeTarget();
    applyTarget(target);

    // Manage the idle timer: it only matters while we're (or are becoming)
    // Active. Restart it on any re-eval that lands on Active so a fresh user
    // action keeps us Active for another kIdleAfterMs.
    if (target == SleepState::Active) {
        m_idleTimer->start();
    } else {
        m_idleTimer->stop();
    }
}

void SleepStateMachine::onIdleTimer()
{
    // The idle window lapsed. Re-evaluate; if still foreground+screen-on,
    // computeTarget() now returns ForegroundIdle (userJustActed is false).
    reevaluate();
}

void SleepStateMachine::applyTarget(SleepState target)
{
    if (target == m_state) {
        // Already there — cancel any pending debounce toward something else.
        if (m_debounceTimer->isActive() && m_pendingTarget != m_state) {
            m_debounceTimer->stop();
            m_pendingTarget = m_state;
        }
        return;
    }

    const bool towardMoreActive = activityRank(target) > activityRank(m_state);
    if (towardMoreActive) {
        // Fire immediately — responsiveness matters; the small cost of
        // waking early is worth not making the user wait. Cancel any
        // in-flight conservative debounce.
        m_debounceTimer->stop();
        const SleepState prev = m_state;
        m_state = target;
        m_pendingTarget = target;
        qCInfo(lcSleep) << "→" << sleepStateName(m_state)
                        << "(from" << sleepStateName(prev) << ", immediate)";
        emit sleepStateChanged(m_state);
    } else {
        // Toward conservation — debounce. Arm (or re-arm) the timer for the
        // new target. A notification peek that flips screen off then on
        // within kDebounceMs never triggers the expensive teardown.
        m_pendingTarget = target;
        m_debounceTimer->start();  // restart fresh window
        qCDebug(lcSleep) << "debouncing →" << sleepStateName(target)
                         << "in" << kDebounceMs << "ms";
    }
}

} // namespace SnapClone::Core
