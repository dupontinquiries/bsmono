#include "ui/camera/CameraHealthService.hpp"
#include "ui/camera/CameraTester.hpp"
#include "ui/camera/CameraRegistry.hpp"

#include <QLoggingCategory>
#include <QDateTime>
#include <algorithm>
#include <limits>

namespace SnapClone::UI::Camera {

namespace {
Q_LOGGING_CATEGORY(lcCamHealth, "snapclone.camera.health")
} // namespace

CameraHealthService &CameraHealthService::instance()
{
    // Function-local static — thread-safe initialization since C++11
    // (Itanium ABI), zero-overhead after first construction. We don't
    // try to control destruction order: at process shutdown Qt will
    // disconnect signals before our dtor runs.
    static CameraHealthService s;
    return s;
}

CameraHealthService::CameraHealthService()
{
    // Lazy: don't probe in the ctor. The first ensureProbed() call
    // (or rescan()) drives the actual work. This keeps singleton
    // construction cheap — anyone touching CameraHealthService::instance()
    // for a passive read doesn't pay for a probe pass.
    m_tester = new CameraTester(this);
    connect(m_tester.data(), &CameraTester::resultReady,
            this, &CameraHealthService::onResultReady);
    connect(m_tester.data(), &CameraTester::done,
            this, [this]() {
        m_scanInFlight = false;
        qCInfo(lcCamHealth) << "Scan complete; cached results:" << m_results.size();
        emit scanComplete();
    });
}

ProbeResult CameraHealthService::result(const QString &deviceId) const
{
    // QHash::value returns default-constructed ProbeResult (Untested)
    // on miss; that's exactly what callers want for the "never probed"
    // case.
    return m_results.value(deviceId);
}

bool CameraHealthService::hasResults() const
{
    // "Hasn't yet emitted scanComplete" doesn't equal "no results".
    // Some results may have streamed in before the batch finished if
    // the caller queried mid-scan.
    return !m_results.isEmpty();
}

QString CameraHealthService::bestWorkingDeviceId(Facing preferredFacing) const
{
    // Pull all available device metadata up front. We need both the
    // CameraInfo (for facing) AND the cached ProbeResult (for ttff).
    // The join is by virtualNodeId.
    const auto devices = CameraRegistry::instance().available();
    if (devices.isEmpty()) return {};

    // Single linear pass. Track the best in each of two priority
    // tiers (matches the docstring's criteria order):
    //   tier 1: facing == preferredFacing AND Working
    //   tier 2: any other Working camera
    // Within a tier, smallest timeToFirstFrameMs wins. If none of the
    // probed devices is Working, return empty (NOT a guess) — caller
    // must fall back to CameraRegistry::primaryRear() or similar.
    QString bestPreferred;
    int     bestPreferredTtff = std::numeric_limits<int>::max();
    QString bestAny;
    int     bestAnyTtff = std::numeric_limits<int>::max();

    for (const auto &dev : devices) {
        const auto r = m_results.value(dev.virtualNodeId);
        if (r.status != ProbeResult::Status::Working) continue;
        const int ttff = r.timeToFirstFrameMs >= 0
            ? r.timeToFirstFrameMs : std::numeric_limits<int>::max();
        if (dev.facing == preferredFacing && ttff < bestPreferredTtff) {
            bestPreferred = dev.virtualNodeId;
            bestPreferredTtff = ttff;
        }
        if (ttff < bestAnyTtff) {
            bestAny = dev.virtualNodeId;
            bestAnyTtff = ttff;
        }
    }
    return !bestPreferred.isEmpty() ? bestPreferred : bestAny;
}

QList<DeviceInfo> CameraHealthService::buildWorkList(bool force) const
{
    const auto devices = CameraRegistry::instance().available();
    const qint64 nowMs = QDateTime::currentMSecsSinceEpoch();
    QList<DeviceInfo> out;
    out.reserve(devices.size());
    for (const auto &dev : devices) {
        // V32.34+: skip devices currently held by a real session.
        // The viewfinder's success IS the working proof; probing
        // would either block it or fail outright on the libcamera
        // single-client constraint.
        if (m_inUseDevices.contains(dev.virtualNodeId)) {
            qCDebug(lcCamHealth) << "buildWorkList: skipping in-use device"
                                 << dev.virtualNodeId;
            continue;
        }
        if (force) {
            DeviceInfo di;
            di.id          = dev.virtualNodeId;
            di.displayName = dev.displayName;
            out.append(di);
            continue;
        }
        // Filter to only devices whose cached result is missing OR stale.
        const auto it = m_results.constFind(dev.virtualNodeId);
        if (it == m_results.constEnd()
            || it->status == ProbeResult::Status::Untested
            || (nowMs - it->probedAtMs) > kStaleAfterMs) {
            DeviceInfo di;
            di.id          = dev.virtualNodeId;
            di.displayName = dev.displayName;
            out.append(di);
        }
    }
    return out;
}

void CameraHealthService::setDeviceInUse(const QString &deviceId, bool inUse)
{
    if (deviceId.isEmpty()) return;
    if (inUse) {
        m_inUseDevices.insert(deviceId);
        // Live capture is stronger evidence of "Working" than any
        // probe could be. Force-set the cache to Working with a 0ms
        // ttff (the "we didn't measure it" sentinel — distinguishable
        // from a probe-measured value since real probes always take
        // some ms). framePixelWidth/Height left at -1 — we don't
        // know what the live session negotiated.
        ProbeResult r = m_results.value(deviceId);
        r.status = ProbeResult::Status::Working;
        r.timeToFirstFrameMs = 0;
        r.reason.clear();
        r.probedAtMs = QDateTime::currentMSecsSinceEpoch();
        m_results.insert(deviceId, r);
        qCInfo(lcCamHealth) << "setDeviceInUse(true)" << deviceId
                            << "— marked Working (live session)";
        emit resultReady(deviceId, r);
    } else {
        m_inUseDevices.remove(deviceId);
        // Preserve the existing cached result. If we were going to
        // demote it (e.g. mark Untested), we'd lose the genuine
        // confidence the live session gave us — and the next
        // ensureProbed would re-probe a known-working device,
        // wasted work. The kStaleAfterMs timer handles staleness
        // naturally.
        qCDebug(lcCamHealth) << "setDeviceInUse(false)" << deviceId;
    }
}

void CameraHealthService::ensureProbed()
{
    if (m_scanInFlight) {
        qCDebug(lcCamHealth) << "ensureProbed: scan already in flight";
        return;
    }
    const auto work = buildWorkList(/*force=*/false);
    if (work.isEmpty()) {
        qCDebug(lcCamHealth) << "ensureProbed: nothing to do (all results fresh)";
        // Don't emit scanComplete here — we didn't actually scan.
        // Callers waiting on the signal interpret "no signal" as
        // "results already there"; the empty queue case is the
        // valid no-op.
        return;
    }
    qCInfo(lcCamHealth) << "ensureProbed: probing" << work.size() << "devices";
    m_scanInFlight = true;
    m_tester->probeAll(work);
}

void CameraHealthService::probeOne(const QString &deviceId)
{
    if (deviceId.isEmpty()) return;
    // V32.34+: refuse to probe an in-use device. The live session
    // already proves it works; a probe would either block (libcamera
    // serializes) or evict the live session (libcamera errors the
    // existing client out). We synthesize a Working result if the
    // cache somehow doesn't already have one — so the Settings UI
    // immediately shows the right badge.
    if (m_inUseDevices.contains(deviceId)) {
        qCInfo(lcCamHealth) << "probeOne: refusing to probe in-use device"
                            << deviceId;
        ProbeResult r = m_results.value(deviceId);
        if (r.status != ProbeResult::Status::Working) {
            r.status = ProbeResult::Status::Working;
            r.timeToFirstFrameMs = 0;
            r.probedAtMs = QDateTime::currentMSecsSinceEpoch();
            r.reason.clear();
            m_results.insert(deviceId, r);
        }
        emit resultReady(deviceId, r);
        emit scanComplete();  // keep the testAllBtn UI consistent
        return;
    }
    qCInfo(lcCamHealth) << "probeOne:" << deviceId;
    m_scanInFlight = true;
    m_tester->probe(deviceId);
}

void CameraHealthService::rescan()
{
    // If a scan is already running we cancel it — the user explicitly
    // asked for fresh data, so finishing a stale scan first wastes time.
    if (m_scanInFlight && m_tester) {
        m_tester->cancel();
    }
    // V32.34+: clear everything EXCEPT in-use entries. In-use entries
    // represent live sessions whose Working status is observably true
    // (frames are arriving as we speak). Wiping them just to immediately
    // re-set them to Working in the loop below is pointless churn AND
    // produces a flicker in any UI watching resultReady.
    if (m_inUseDevices.isEmpty()) {
        m_results.clear();
    } else {
        QHash<QString, ProbeResult> preserved;
        for (const QString &id : m_inUseDevices) {
            const auto it = m_results.constFind(id);
            if (it != m_results.constEnd()) preserved.insert(id, *it);
        }
        m_results = std::move(preserved);
    }
    if (m_tester) m_tester->clearResults();

    const auto work = buildWorkList(/*force=*/true);
    if (work.isEmpty()) {
        // No devices to scan; immediately emit scanComplete so any
        // listener waiting on the signal unblocks.
        emit scanComplete();
        return;
    }
    qCInfo(lcCamHealth) << "rescan: probing" << work.size() << "devices"
                        << "(" << m_inUseDevices.size() << "in-use skipped)";
    m_scanInFlight = true;
    m_tester->probeAll(work);
}

void CameraHealthService::onResultReady(const QString &deviceId,
                                        const ProbeResult &result)
{
    // Update cache, then forward. The forward happens AFTER the cache
    // update so any listener that calls result(id) inside its handler
    // sees the latest data.
    m_results.insert(deviceId, result);
    emit resultReady(deviceId, result);
}

} // namespace SnapClone::UI::Camera
