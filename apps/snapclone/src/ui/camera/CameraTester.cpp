#include "ui/camera/CameraTester.hpp"
#include "ui/camera/GstLaunchBackend.hpp"
#include "ui/camera/CameraPolicy.hpp"

#include <QTimer>
#include <QDateTime>
#include <QLoggingCategory>

namespace SnapClone::UI::Camera {

namespace {
Q_LOGGING_CATEGORY(lcCamTest, "snapclone.camera.tester")

/// Translate a Status to a short user-facing string, used in log lines
/// (and the Settings UI consumes the enum directly to pick a colour).
const char *statusName(ProbeResult::Status s)
{
    switch (s) {
        case ProbeResult::Status::Untested:      return "Untested";
        case ProbeResult::Status::Working:       return "Working";
        case ProbeResult::Status::NoFrames:      return "NoFrames";
        case ProbeResult::Status::ProcessFailed: return "ProcessFailed";
        case ProbeResult::Status::EarlyExit:     return "EarlyExit";
        case ProbeResult::Status::TimedOut:      return "TimedOut";
    }
    return "?";
}
} // namespace

CameraTester::CameraTester(QObject *parent)
    : QObject(parent)
{
    // Deadline timer — single-shot, owned by us, re-armed per probe.
    // Built once in the ctor so we don't churn allocations per probe.
    m_deadline = new QTimer(this);
    m_deadline->setSingleShot(true);
    connect(m_deadline, &QTimer::timeout, this, [this]() {
        // Deadline fired — backend never signaled. Treat as TimedOut.
        // CameraTester::finalize handles backend teardown + queue
        // advancement, so we just route through it.
        qCWarning(lcCamTest) << "Probe deadline hit for" << m_currentId;
        finalize(ProbeResult::Status::TimedOut,
                 tr("backend never signaled within deadline"));
    });
}

CameraTester::~CameraTester() = default;

void CameraTester::probe(const QString &deviceId)
{
    // Single-shot probe. If something's already in flight, drop it
    // (TimedOut on the old id) and start fresh on the new one.
    if (m_inFlight) {
        qCInfo(lcCamTest) << "probe(" << deviceId << "): preempting in-flight"
                          << m_currentId;
        finalize(ProbeResult::Status::TimedOut,
                 tr("preempted by new probe request"));
    }
    DeviceInfo di;
    di.id = deviceId;
    di.displayName = deviceId;
    m_queue.clear();
    m_queue.append(di);
    startNext();
}

void CameraTester::probeAll(const QList<DeviceInfo> &devices)
{
    if (m_inFlight) {
        qCInfo(lcCamTest) << "probeAll: preempting in-flight" << m_currentId;
        finalize(ProbeResult::Status::TimedOut,
                 tr("preempted by probeAll"));
    }
    m_queue = devices;
    if (m_queue.isEmpty()) {
        // Nothing to do — still emit done() so listeners that wait on
        // it don't hang forever.
        QTimer::singleShot(0, this, [this]() { emit done(); });
        return;
    }
    startNext();
}

void CameraTester::cancel()
{
    if (!m_inFlight) {
        m_queue.clear();
        return;
    }
    qCInfo(lcCamTest) << "Cancel requested — abandoning probe of" << m_currentId;
    m_queue.clear();  // don't process remaining queue
    finalize(ProbeResult::Status::TimedOut, tr("cancelled"));
}

ProbeResult CameraTester::result(const QString &deviceId) const
{
    return m_cache.value(deviceId);  // default-constructed = Untested
}

void CameraTester::clearResults()
{
    m_cache.clear();
}

void CameraTester::startNext()
{
    // Pop next from queue; if empty we've finished the batch.
    if (m_queue.isEmpty()) {
        m_inFlight = false;
        emit done();
        return;
    }
    const DeviceInfo di = m_queue.takeFirst();
    m_currentId = di.id;
    m_inFlight  = true;

    // Build a fresh probe-mode backend. Parented to us so it gets
    // destroyed if the tester is destroyed mid-flight. QPointer
    // protects against the case where the backend deletes itself
    // before we tear it down (shouldn't happen with current code,
    // but defensive).
    GstLaunchBackend *probe = GstLaunchBackend::makeProbe(this);
    m_currentBackend = probe;

    // Wire the three signals we care about. All connections are
    // unique_connection-by-context (the lambda captures `this`,
    // and Qt destroys the backend when WE go away).
    connect(probe, &CameraBackend::firstFrameReceived,
            this, [this]() {
        if (!m_currentBackend) return;  // race: signal queued before teardown
        finalize(ProbeResult::Status::Working, QString());
    });
    connect(probe, &CameraBackend::error,
            this, [this](const QString &msg) {
        if (!m_currentBackend) return;
        // Distinguish "process didn't start" from "process started
        // but crashed early". The backend's error() signal fires for
        // both; we use elapsed time to disambiguate.
        const auto status = (m_currentTimer.isValid() && m_currentTimer.elapsed() < 100)
            ? ProbeResult::Status::ProcessFailed
            : ProbeResult::Status::EarlyExit;
        finalize(status, msg);
    });
    connect(probe, &CameraBackend::stopped,
            this, [this]() {
        if (!m_currentBackend) return;
        // stopped() without firstFrameReceived() = no frames before exit.
        // If firstFrameReceived fired first, the corresponding lambda
        // already called finalize() which cleared m_currentBackend, so
        // this guard catches the stop-after-success case as a no-op.
        finalize(ProbeResult::Status::NoFrames,
                 tr("backend stopped before producing a frame"));
    });

    // Compute the deadline. Backend will emit one of the three signals
    // above within its OWN policy timeouts (probeStartupTimeoutMs +
    // probeFirstFrameTimeoutMs); we add a small slack so we don't race
    // the backend's timer.
    const CameraPolicy policy = CameraPolicyStore::instance().snapshot();
    const int deadlineMs = policy.probeStartupTimeoutMs
                         + policy.probeFirstFrameTimeoutMs
                         + 500;  // slack

    qCInfo(lcCamTest) << "Probing" << m_currentId
                      << "deadline=" << deadlineMs << "ms";

    m_currentTimer.restart();
    m_deadline->start(deadlineMs);

    // Kick the backend. If start() returns false synchronously (no
    // gst-launch on PATH, no devices, etc.), finalize immediately.
    if (!probe->start(m_currentId)) {
        finalize(ProbeResult::Status::ProcessFailed,
                 tr("backend rejected start() synchronously"));
    }
}

void CameraTester::finalize(const ProbeResult::Status status, const QString &reason)
{
    if (!m_inFlight) {
        // Re-entrant call (e.g. stopped() fires after firstFrameReceived()
        // already finalized). Ignore.
        return;
    }

    ProbeResult r;
    r.status = status;
    r.reason = reason;
    r.probedAtMs = QDateTime::currentMSecsSinceEpoch();
    if (status == ProbeResult::Status::Working && m_currentTimer.isValid()) {
        r.timeToFirstFrameMs = static_cast<int>(m_currentTimer.elapsed());
    }
    // V32.34+: capture the negotiated frame dimensions. The probe
    // backend stashes the first frame in lastFrame(); if it's not
    // null we read the width/height. For non-Working statuses these
    // stay at the default -1 (lastFrame() is also null on failure
    // paths, so the guard does double duty).
    if (m_currentBackend) {
        const QImage img = m_currentBackend->lastFrame();
        if (!img.isNull()) {
            r.framePixelWidth  = img.width();
            r.framePixelHeight = img.height();
        }
    }
    m_cache.insert(m_currentId, r);

    qCInfo(lcCamTest) << "Probe" << m_currentId << "→" << statusName(status)
                      << (status == ProbeResult::Status::Working
                              ? QStringLiteral("ttff=%1ms %2x%3")
                                    .arg(r.timeToFirstFrameMs)
                                    .arg(r.framePixelWidth)
                                    .arg(r.framePixelHeight)
                              : reason);

    // Disarm the deadline timer if it hasn't fired yet.
    m_deadline->stop();

    // Tear down the backend. Use deleteLater so any in-flight queued
    // signals from it have a chance to drop into the event loop first
    // (where our guard QPointer check skips them).
    if (m_currentBackend) {
        GstLaunchBackend *b = m_currentBackend.data();
        // Disconnect everything so the stop() inside the backend
        // dtor (or any pending queued signals) doesn't re-enter
        // finalize().
        disconnect(b, nullptr, this, nullptr);
        b->stop();
        b->deleteLater();
        m_currentBackend = nullptr;
    }

    const QString finishedId = m_currentId;
    m_currentId.clear();
    m_inFlight = false;

    emit resultReady(finishedId, r);

    // Advance the queue. If empty, startNext() will emit done(). We
    // schedule via singleShot(0) so the resultReady receiver runs to
    // completion before the next probe starts — matters for the UI,
    // which may be rebuilding badges off the just-emitted result.
    if (!m_queue.isEmpty()) {
        QTimer::singleShot(0, this, [this]() { startNext(); });
    } else {
        // Final result in the batch — still emit done() on next tick
        // so the receiver chain runs in a consistent order
        // (resultReady → done, not interleaved).
        QTimer::singleShot(0, this, [this]() { emit done(); });
    }
}

} // namespace SnapClone::UI::Camera
