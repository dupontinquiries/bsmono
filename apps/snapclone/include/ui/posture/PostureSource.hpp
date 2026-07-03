#pragma once
// ─── ui/posture/PostureSource.hpp ─────────────────────────────────────────────
//
// Abstract source of physical "posture" signals for the PostureEngine. The
// engine only needs two booleans about the world:
//   • proximity near  — something is close to the front sensor (ear / cover / face
//                        down on a desk),
//   • device moving   — the phone is being moved right now (vs sitting still).
//
// The real source (IioSensorProxySource) reads these from iio-sensor-proxy /
// IIO. FakePostureSource lets tests and the desktop build drive them by hand, so
// the whole engine is exercisable without hardware.
//
// start()/stop() bracket delivery: the engine calls start() only while it's armed
// (a voice context is live) and stop() when it disarms, so the sensors are only
// claimed when actually needed — no battery cost the rest of the time.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>

namespace SnapClone::UI {

class PostureSource : public QObject {
    Q_OBJECT
public:
    explicit PostureSource(QObject *parent = nullptr) : QObject(parent) {}
    ~PostureSource() override = default;

    virtual void start() = 0;   ///< claim sensors + begin emitting changes
    virtual void stop()  = 0;   ///< release sensors

    /// Whether this source actually has a usable proximity sensor. If false, the
    /// engine should treat the world as "far + still" (i.e. behave normally) — we
    /// never want to blank the screen on a device whose proximity we can't read.
    [[nodiscard]] virtual bool available() const = 0;

Q_SIGNALS:
    void proximityChanged(bool near);   ///< true = something close
    void motionChanged(bool moving);    ///< true = device is being moved
};

// ── A hand-driven source for tests and the desktop build ──────────────────────
class FakePostureSource : public PostureSource {
    Q_OBJECT
public:
    using PostureSource::PostureSource;

    void start() override { m_running = true; }
    void stop()  override { m_running = false; }
    [[nodiscard]] bool available() const override { return true; }
    [[nodiscard]] bool running() const { return m_running; }

    void setProximityNear(bool near)
    {
        if (near == m_near) return;
        m_near = near;
        Q_EMIT proximityChanged(m_near);
    }
    void setMoving(bool moving)
    {
        if (moving == m_moving) return;
        m_moving = moving;
        Q_EMIT motionChanged(m_moving);
    }

private:
    bool m_running = false;
    bool m_near    = false;
    bool m_moving  = false;
};

} // namespace SnapClone::UI
