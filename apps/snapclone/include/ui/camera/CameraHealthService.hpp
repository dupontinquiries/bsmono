#pragma once

// ─── ui/camera/CameraHealthService.hpp ───────────────────────────────────────
//
// CameraHealthService — process-wide singleton that maintains a cached
// probe of every discovered camera. The default consumer is
// CameraWidget's "which camera should I open first" decision, but any
// component that wants to know "is the rear camera actually working"
// without committing the device to a real session can ask here.
//
// Design philosophy:
//   • The service is lazy-singleton — no work happens until something
//     asks. CameraRegistry discovery still works the same way; we
//     just layer health probes on top.
//   • Probes run serially via CameraTester (the kernel libcamera
//     module typically can't serve two clients per sensor at once).
//   • Results are cached for kStaleAfterMs; older results trigger a
//     reprobe on the next query. This balances "always fresh" with
//     "don't reprobe constantly".
//   • The service NEVER touches CameraRegistry's main capture path.
//     A probe is its own session; tearing it down frees the node
//     for the user's real capture before the service returns.
//
// Why this is a separate class from CameraTester:
//   • CameraTester is a per-instance probe driver — the Settings UI
//     keeps one to back its Test buttons; the health service keeps
//     another for background scanning. Two instances must not collide,
//     so we don't make CameraTester itself a singleton.
//   • CameraHealthService adds the cache-with-TTL policy and the
//     "pick best working device" helper, which doesn't belong in
//     CameraTester (which is a low-level mechanism, not policy).
//
// V32.34+ — replaces the implicit "try every device serially in
// CameraWidget until one delivers frames" pattern. That pattern was
// slow on cold start (5s timeout × N devices = worst case 5N
// seconds before the user sees a viewfinder) and gave no signal at
// all when nothing worked.

#include <QObject>
#include <QString>
#include <QStringList>
#include <QHash>
#include <QSet>
#include <QPointer>
#include <QDateTime>

#include "ui/camera/CameraTester.hpp"   // ProbeResult by value
#include "ui/camera/CameraRegistry.hpp" // Facing enum
#include "ui/camera/CameraBackend.hpp"  // DeviceInfo

namespace SnapClone::UI::Camera {

class CameraTester;

class CameraHealthService : public QObject
{
    Q_OBJECT
public:
    /// Process-wide accessor. Lazily constructed on first call.
    /// The instance is created on whichever thread first invokes
    /// instance() — typically the GUI thread (Settings or CameraWidget
    /// being built). NOT thread-safe; assume GUI thread only.
    static CameraHealthService &instance();

    /// Cached result for a specific device id. Returns Status::Untested
    /// if we've never probed (or the result has aged out).
    [[nodiscard]] ProbeResult result(const QString &deviceId) const;

    /// Best known-working device id, considering all cached results.
    /// Returns empty if no probes have run yet OR no working camera
    /// was found.
    ///
    /// The "best" criteria, in priority order:
    ///   1. A device whose CameraInfo facing matches preferredFacing
    ///      (default Rear).
    ///   2. Smallest timeToFirstFrameMs (fast cameras feel more
    ///      responsive).
    /// Untested devices are NEVER returned by bestWorkingDeviceId
    /// — callers should call ensureProbed() first if they want
    /// fresh data, or use CameraRegistry::primaryRear()'s fallback
    /// otherwise.
    [[nodiscard]] QString bestWorkingDeviceId(Facing preferredFacing = Facing::Rear) const;

    /// Whether we have ANY non-Untested results. Used by CameraWidget
    /// to decide whether to call bestWorkingDeviceId vs falling back
    /// to CameraRegistry's static-table default.
    [[nodiscard]] bool hasResults() const;

    /// Kick off (or refresh) a probe of every CameraRegistry-known
    /// device whose cached result is older than kStaleAfterMs (or
    /// missing entirely). If a scan is already in flight this is a
    /// no-op. Results stream in via resultReady() as each device
    /// finishes; scanComplete() fires when the whole batch is done.
    void ensureProbed();

    /// Probe a single device on demand. Backs the per-device "Test"
    /// buttons in Settings. If a scan is already in flight this
    /// preempts it (the scanned device gets a fresh probe, queued
    /// devices are dropped).
    void probeOne(const QString &deviceId);

    /// Mark a device as "currently in use by the real capture session"
    /// — CameraWidget calls this when starting / stopping the user-
    /// facing viewfinder. Probes never touch in-use devices because:
    ///   • libcamera typically can't serve two clients on the same
    ///     sensor (probing while the viewfinder is running either
    ///     blocks the viewfinder or errors out).
    ///   • If the real session is producing frames, that's already
    ///     stronger empirical evidence of "Working" than any probe.
    ///
    /// When markInUse goes true, the cached result is force-set to
    /// Working with a synthetic 0ms timeToFirstFrame (since we can't
    /// measure it from a live session). When it goes false, the
    /// cached result is preserved — it'll re-probe naturally when
    /// it ages out.
    void setDeviceInUse(const QString &deviceId, bool inUse);

    /// Force-restart a scan from scratch, replacing any cached
    /// results. Useful when the user opens Settings and wants
    /// freshly-confirmed badges.
    void rescan();

signals:
    /// Emitted once per device as its probe finishes. Mirrors the
    /// CameraTester signal of the same name for downstream
    /// composability.
    void resultReady(const QString &deviceId, const SnapClone::UI::Camera::ProbeResult &result);

    /// Emitted when the current batch scan finishes (whether it
    /// confirmed every device or one of them failed/timed out).
    void scanComplete();

private:
    CameraHealthService();

    /// Build the work list from CameraRegistry::available() minus any
    /// device whose cached result is recent enough.
    [[nodiscard]] QList<DeviceInfo> buildWorkList(bool force) const;

    /// Forward CameraTester's per-device signals into our own cache,
    /// then re-emit so callers can subscribe without knowing about
    /// the tester instance.
    void onResultReady(const QString &deviceId, const ProbeResult &result);

    /// Owned probe driver. We never share it — Settings owns its own,
    /// and this one is exclusively for the health service. Parented
    /// to `this` for lifetime.
    QPointer<CameraTester>           m_tester;
    /// Cached results keyed by virtualNodeId. Cleared by rescan();
    /// updated incrementally as resultReady fires.
    QHash<QString, ProbeResult>      m_results;
    /// Set true when a scan is in flight so concurrent ensureProbed()
    /// calls don't double-queue work.
    bool                             m_scanInFlight = false;

    /// Devices currently held by a real capture session (CameraWidget's
    /// viewfinder or PIP). Probes skip these — see setDeviceInUse for
    /// rationale. Set, not hash, because we don't care about reference
    /// counting; CameraWidget's start/stop calls are paired 1:1 with
    /// in-use transitions on a given id.
    ///
    /// Why a separate field rather than encoding "in use" into the
    /// ProbeResult: in-use is a transient property of the live session;
    /// ProbeResult is the cached fact about a probe run. Mixing them
    /// makes "is the camera working" answer wobble between "Working"
    /// (last probe) and "InUse" (now), confusing the Settings UI.
    QSet<QString>                    m_inUseDevices;

    /// How long a probe result is considered fresh. After this, a
    /// device shows up in buildWorkList() again. Tuned for "user
    /// opens settings, scrolls to camera section, expects badges
    /// not to be ancient" — 60 seconds gives time to navigate
    /// between settings tabs without re-scanning every visit.
    static constexpr qint64 kStaleAfterMs = 60'000;
};

} // namespace SnapClone::UI::Camera
