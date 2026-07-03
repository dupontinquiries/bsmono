#pragma once
// ─── UI/AmbientInboxDriver.hpp ───────────────────────────────────────────────
//
// V32.30: auto-refresh driver for the Ambient: Inbox synthetic palette
// (Mode A). Subscribes to AppController's snapReceived signal so the
// palette evolves as new snaps arrive — matches the design intent that
// the chrome "drifts toward whoever's been sending you things."
//
// Throttle: 30s minimum between recomputes (spec). A burst of 5 inbound
// snaps within 30s produces ONE derivation; the next derivation fires
// 30s after that. This protects against:
//   • Storage cascades during initial inbox load (V32.20 path which
//     emits snapReceived for each restored snap)
//   • Bulk send-storms (group photo, peer reconnect flushing
//     queued messages)
//
// Lifecycle:
//   • Constructed once by MainWindow; lives until app shutdown
//   • start(AppController*) subscribes to snapReceived
//   • Always-on while running; the throttle handles bursts internally
//   • stop() unsubscribes — used when user picks a non-inbox synthetic
//     or a static palette
//
// Distinction from AmbientCameraDriver:
//   • Camera driver runs a 1Hz timer (frame-driven sample is too noisy
//     to derive every frame)
//   • Inbox driver is event-driven (one snap = one trigger), throttled
//     by recency check rather than periodic polling
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QPointer>
#include <QDateTime>
#include "core/Types.hpp"

namespace SnapClone::Core { class AppController; }

namespace SnapClone::UI {

class AmbientInboxDriver : public QObject {
    Q_OBJECT
public:
    explicit AmbientInboxDriver(QObject *parent = nullptr);
    ~AmbientInboxDriver() override;

    /// Start subscribing to controller's snapReceived. Idempotent.
    /// On start, an immediate derivation runs (uses the current inbox
    /// as seed); subsequent triggers honour the 30s throttle.
    void start(SnapClone::Core::AppController *controller);

    /// Unsubscribe + idle. Subsequent calls to start() rebuild from
    /// the current inbox.
    void stop();

    bool isActive() const { return m_active; }

private slots:
    /// Fired when AppController emits snapReceived. May skip if within
    /// the throttle window.
    void onSnapReceived(SnapClone::Core::Snap snap);

private:
    /// Run a derivation NOW (no throttle check). Used by start() for the
    /// initial pass and by onSnapReceived when the throttle window has
    /// elapsed.
    void deriveNow();

    QPointer<SnapClone::Core::AppController> m_controller;
    qint64 m_lastDeriveMs = 0;
    bool   m_active = false;
    /// Spec throttle — 30s. constexpr-local would shadow if exposed via
    /// API, but it's a tunable knob.
    static constexpr qint64 kMinIntervalMs = 30000;
};

} // namespace SnapClone::UI
