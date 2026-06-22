#pragma once

// ─── ui/camera/CameraTester.hpp ──────────────────────────────────────────────
//
// CameraTester — runs short, non-destructive probes against a set of
// camera nodes and reports per-node health: works/fails, time-to-first-
// frame, failure reason. Backs the "Test camera" Settings UI and the
// post-startup health snapshot that the CameraWidget consults when
// choosing a default.
//
// Design:
//   • One CameraTester instance can probe many nodes serially OR
//     a single node on demand. Caller chooses.
//   • Each probe spins up a probe-mode GstLaunchBackend (short
//     timeouts, auto-stop on first frame), wires its signals, and
//     waits for either firstFrameReceived() or error() / timeout.
//   • Results are emitted via resultReady(deviceId, ProbeResult)
//     as each finishes, plus an aggregated done() when the whole
//     batch completes.
//   • Cache: results are kept in-memory for the session. UI can
//     ask currentResult(deviceId) to render the badge without
//     re-running. Cache is cleared on policy change.
//
// Threading: GUI thread only. The probe backends spawn subprocesses
// but signal back to us via Qt::QueuedConnection (Qt default), so
// from our point of view everything is single-threaded.
//
// V32.34+ — replaces the implicit "start it and see if frames come"
// pattern that the camera widget used. Decoupling probing from the
// real session means we can run a quick scan at startup, cache the
// results, and feed CameraWidget a pre-validated default.

#include <QObject>
#include <QHash>
#include <QString>
#include <QStringList>
#include <QPointer>
#include <QElapsedTimer>

#include "ui/camera/CameraBackend.hpp"   // DeviceInfo

namespace SnapClone::UI::Camera {

class GstLaunchBackend;

/// Outcome of one probe run.
struct ProbeResult {
    enum class Status {
        Untested,        ///< Never probed in this session.
        Working,         ///< Got first frame within timeout.
        NoFrames,        ///< Backend started but no frame arrived (stuck driver).
        ProcessFailed,   ///< gst-launch / subprocess didn't start.
        EarlyExit,       ///< Process exited shortly after start (bad node, bad caps).
        TimedOut,        ///< Tester deadline hit before any backend signal.
    };

    Status      status              = Status::Untested;
    /// Milliseconds from probe start to firstFrameReceived (only meaningful
    /// for Working status). -1 otherwise.
    int         timeToFirstFrameMs  = -1;
    /// Width × height of the captured first frame (only meaningful for
    /// Working status; -1 / -1 otherwise). Captured by reading the
    /// probe backend's lastFrame() after firstFrameReceived(). When
    /// these differ from CameraPolicy::captureWidth/Height, the
    /// pipeline negotiated to something other than what the policy
    /// requested — usually fine (videoscale handles it) but worth
    /// surfacing for users debugging "why is my preview the wrong shape".
    int         framePixelWidth     = -1;
    int         framePixelHeight    = -1;
    /// Human-readable reason for non-Working statuses. Empty for Working.
    QString     reason;
    /// When the probe ran (msecs-since-epoch). Lets UI fade old results.
    qint64      probedAtMs          = 0;
};

class CameraTester : public QObject
{
    Q_OBJECT
public:
    explicit CameraTester(QObject *parent = nullptr);
    ~CameraTester() override;

    // ── Probing ──────────────────────────────────────────────────────

    /// Probe a single device. Returns immediately; result delivered via
    /// resultReady(). If already probing this id, the previous call is
    /// finalized with TimedOut and the new one starts.
    void probe(const QString &deviceId);

    /// Probe every device in the provided list, serially. Results
    /// stream via resultReady() as each finishes; done() fires after
    /// the whole list completes. Serial (not parallel) because the
    /// libcamera kernel module typically can't serve two clients
    /// against the same sensor at once.
    void probeAll(const QList<DeviceInfo> &devices);

    /// Cancel any in-flight probe. Reset state; cached results are
    /// preserved.
    void cancel();

    /// True if a probe is currently in flight.
    [[nodiscard]] bool isProbing() const { return m_inFlight; }

    // ── Result cache ─────────────────────────────────────────────────

    /// Most recent result for the given device id, or Status::Untested
    /// if we've never probed it.
    [[nodiscard]] ProbeResult result(const QString &deviceId) const;

    /// Wipe all cached results. Useful after a CameraPolicy change
    /// (the new timeouts mean old results may no longer be valid).
    void clearResults();

signals:
    /// Emitted when an individual probe finishes (success OR failure).
    void resultReady(const QString &deviceId, const SnapClone::UI::Camera::ProbeResult &result);

    /// Emitted after probeAll() completes the full list.
    void done();

private:
    void startNext();
    void finalize(const ProbeResult::Status status, const QString &reason);

    QHash<QString, ProbeResult> m_cache;

    // Active probe state.
    QPointer<GstLaunchBackend>  m_currentBackend;
    QString                     m_currentId;
    QElapsedTimer               m_currentTimer;
    /// Deadline timer that fires if the backend never signals success or
    /// failure (defense against a backend hang). Bounded by probe
    /// policy + small slack.
    QTimer                     *m_deadline = nullptr;
    /// Queue of remaining devices for probeAll(); empty during single
    /// probes.
    QList<DeviceInfo>           m_queue;
    bool                        m_inFlight = false;
};

} // namespace SnapClone::UI::Camera
