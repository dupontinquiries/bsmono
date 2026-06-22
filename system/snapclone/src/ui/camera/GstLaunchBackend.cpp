#include "ui/camera/GstLaunchBackend.hpp"
#include "ui/camera/FramePixelOps.hpp"   // V32.39+: shared SIMD R/B swap

#include <QProcess>
#include <QCoreApplication>  // V32.34+: closingDown() shutdown guard
#include <QStandardPaths>
#include <QLoggingCategory>
#include <QStringList>
#include <QTimer>
#include <QDateTime>
#include <QSettings>     // V32.34+: read camera/policy/swapRB override
#include <QMutex>        // V32.34+: gstElementAvailable cache guard
#include <QHash>         // V32.34+: gstElementAvailable memo map
#include <array>
#include <algorithm>
#include "ui/camera/CameraBridgeManager.hpp"
#include "ui/camera/CameraRegistry.hpp"  // V32.32: single source of camera metadata
#include "ui/camera/CameraPolicy.hpp"    // V32.34+: named timeouts / limits

namespace SnapClone::UI::Camera {

namespace PixelOps = SnapClone::Camera::PixelOps;  // V32.39+: shared SIMD pixel ops

Q_LOGGING_CATEGORY(lcGstCam, "snapclone.camera.gst")

// V32.34+ GPU accel: probe-once whether a gst element is installed.
// gst-inspect-1.0 <element> exits 0 if present, non-zero otherwise.
// Memoized in a static map (guarded for the unlikely concurrent-start
// case) so repeated start() cycles don't re-spawn gst-inspect — that
// would add ~50ms × N elements to every camera start.
bool GstLaunchBackend::gstElementAvailable(const QString &element)
{
    static QMutex mu;
    static QHash<QString, bool> cache;
    QMutexLocker lock(&mu);
    const auto it = cache.constFind(element);
    if (it != cache.constEnd()) return it.value();

    bool present = false;
    const QString inspect = QStandardPaths::findExecutable(
        QStringLiteral("gst-inspect-1.0"));
    if (!inspect.isEmpty()) {
        QProcess p;
        p.start(inspect, { element });
        // Bounded wait — gst-inspect is fast, but never block the GUI
        // thread indefinitely if something is wedged.
        if (p.waitForFinished(2000)) {
            present = (p.exitStatus() == QProcess::NormalExit
                       && p.exitCode() == 0);
        } else {
            p.kill();
            p.waitForFinished(500);
        }
    }
    cache.insert(element, present);
    qCInfo(lcGstCam) << "gst element" << element
                     << (present ? "available" : "NOT available");
    return present;
}

// V32.34+ GPU accel + tint fix: build the conversion + scale segment.
//
// Two paths:
//
//   GPU (preferred when GL elements present + policy.preferGpuConvert):
//     glupload ! glcolorconvert ! glcolorscale ! gldownload !
//     video/x-raw,format=BGRx,width=W,height=H
//   The GL elements run YUV→RGB and scaling on the Adreno GPU. The
//   final gldownload + capsfilter pins the CPU-side format to BGRx so
//   our QImage reader sees exactly what it expects. This is both faster
//   (CPU freed for frame delivery → higher sustainable fps) and tint-
//   proof (gldownload's output caps are explicit, no passthrough
//   ambiguity).
//
//   CPU (fallback):
//     videoconvert ! video/x-raw,format=BGRx ! [videoflip] !
//     videoscale ! videoconvert ! video/x-raw,format=BGRx,width=W,height=H
//   The deterministic dual-videoconvert path that fixed the earlier
//   tint. Kept verbatim as the safe fallback.
//
// flipMethod is the videoflip method string ("" = no rotation). On the
// GPU path we use glvideoflip if available, else fall back to inserting
// the CPU videoflip after gldownload (cheap on the already-small frame).
QStringList GstLaunchBackend::buildConvertScaleSegment(
    const QString &flipMethod) const
{
    QStringList seg;
    // V32.34+: if the GL path already failed to link this run, force CPU
    // regardless of policy — a device whose GL stack can't negotiate the
    // download should recover to a working camera, not keep failing.
    const bool wantGpu = m_policy.preferGpuConvert && !m_gpuConvertFailed;
    const bool haveGl  = wantGpu
        && gstElementAvailable(QStringLiteral("glupload"))
        && gstElementAvailable(QStringLiteral("glcolorconvert"))
        && gstElementAvailable(QStringLiteral("gldownload"));

    if (haveGl) {
        m_usingGpuPath = true;
        qCInfo(lcGstCam) << "Using GPU colorspace path (glupload/"
                            "glcolorconvert/glcolorscale/gldownload)";
        // V32.39+ PERF DIAGNOSTIC: quantify the GPU→CPU readback. gldownload
        // transfers this whole buffer to sysmem every frame and is the dominant
        // per-frame sync cost on the FP5; it scales with the (capped) output
        // dimensions, which is why previewMaxWidth/Height is the key lever.
        qCInfo(lcGstCam) << "GPU readback ≈"
                         << QString::number(static_cast<double>(m_frameWidth)
                                            * m_frameHeight * 4.0 / (1024.0 * 1024.0),
                                            'f', 2)
                         << "MB/frame at" << m_frameWidth << "x" << m_frameHeight
                         << "(BGRx). If preview still lags, compare against the CPU"
                            " path: set camera/policy/preferGpuConvert=false.";
        // CRITICAL ordering (V32.34+ fix): glcolorconvert / glcolorscale
        // operate on GL MEMORY. The conversion-to-BGRx and the resize MUST
        // be expressed via a capsfilter that stays IN GL memory
        // (`video/x-raw(memory:GLMemory),...`) BEFORE gldownload. gldownload
        // itself does NOT convert format or scale — it only transfers the
        // already-converted GL frame to system memory. The earlier version
        // put `format=BGRx,width,height` as gldownload's OUTPUT caps, which
        // gldownload can't satisfy → "could not link gldownloadelement0 to
        // fdsink0, can't handle caps ... BGRx 1280x720" and the whole
        // pipeline failed to start on device.
        seg << QStringLiteral("glupload") << QStringLiteral("!");
        // Scale on GPU first (if available), then convert colorspace.
        if (gstElementAvailable(QStringLiteral("glcolorscale"))) {
            seg << QStringLiteral("glcolorscale") << QStringLiteral("!");
        }
        seg << QStringLiteral("glcolorconvert") << QStringLiteral("!");
        // GPU-side flip if available (still in GL memory).
        const bool haveGlFlip =
            !flipMethod.isEmpty()
            && gstElementAvailable(QStringLiteral("glvideoflip"));
        if (haveGlFlip) {
            seg << QStringLiteral("glvideoflip")
                << QStringLiteral("method=") + flipMethod
                << QStringLiteral("!");
        }
        // Pin the GL-memory caps: format + target size, computed ON the GPU.
        // The `(memory:GLMemory)` feature is what forces glcolorscale/
        // glcolorconvert above to actually produce this on-GPU. gldownload
        // then transfers it to sysmem unchanged.
        // V32.35+: when m_relaxCaps is set (after a negotiation failure), drop
        // the width/height pin so libcamera can pick a size it CAN produce —
        // the sink-caps parser already adapts to whatever size actually arrives.
        seg << (m_relaxCaps
                    ? QStringLiteral("video/x-raw(memory:GLMemory),format=BGRx")
                    : QStringLiteral("video/x-raw(memory:GLMemory),format=BGRx,width=%1,height=%2")
                          .arg(m_frameWidth).arg(m_frameHeight))
            << QStringLiteral("!")
            << QStringLiteral("gldownload") << QStringLiteral("!");
        // CPU flip fallback if GL flip wasn't available (operates on the
        // already-small downloaded frame — cheap).
        if (!flipMethod.isEmpty() && !haveGlFlip) {
            seg << QStringLiteral("videoflip")
                << QStringLiteral("method=") + flipMethod
                << QStringLiteral("!");
        }
        // V32.34+ TILING FIX (GPU path): gldownload transfers GL memory to
        // sysmem, but GL textures are aligned to 8/16/32/64 px, so the
        // downloaded buffer's ROW STRIDE is often padded BEYOND width*4 — and
        // its total size beyond width*height*4. Our fdsink reader slices the
        // pipe at width*height*4 and assumes stride==width*4, so a padded
        // download makes one frame render as several shrunken tiled copies
        // (the persistent symptom). Force a CPU videoconvert to a tightly
        // packed buffer with EXPLICIT width/height pinned, so the bytes hitting
        // fdsink are deterministic width*height*4 — exactly what the reader
        // expects. videoconvert repacks to the standard tight stride; cost is
        // one pass over an already-small (preview-sized) frame.
        seg << QStringLiteral("videoconvert") << QStringLiteral("!")
            << (m_relaxCaps
                    ? QStringLiteral("video/x-raw,format=BGRx")
                    : QStringLiteral("video/x-raw,format=BGRx,width=%1,height=%2")
                          .arg(m_frameWidth).arg(m_frameHeight))
            << QStringLiteral("!");
        return seg;
    }

    // ── CPU fallback path (deterministic dual-videoconvert) ───────────────
    m_usingGpuPath = false;
    qCInfo(lcGstCam) << "Using CPU colorspace path (videoconvert ×2)"
                     << (wantGpu ? "(GL elements unavailable)" : "(GPU disabled by policy)");
    seg << QStringLiteral("videoconvert") << QStringLiteral("!")
        << QStringLiteral("video/x-raw,format=BGRx") << QStringLiteral("!");
    if (!flipMethod.isEmpty()) {
        seg << QStringLiteral("videoflip")
            << QStringLiteral("method=") + flipMethod
            << QStringLiteral("!");
    }
    seg << QStringLiteral("videoscale")
        << QStringLiteral("add-borders=true") << QStringLiteral("!")
        << QStringLiteral("videoconvert") << QStringLiteral("!")
        << (m_relaxCaps
                ? QStringLiteral("video/x-raw,format=BGRx")
                : QStringLiteral("video/x-raw,format=BGRx,width=%1,height=%2")
                      .arg(m_frameWidth).arg(m_frameHeight))
        << QStringLiteral("!");
    return seg;
}


// V32.33+: removed local queryPwDump() — CameraRegistry now owns
// pw-dump scanning AND libcamera_input.* fallback discovery.

GstLaunchBackend::GstLaunchBackend(QObject *parent)
    : CameraBackend(parent)
{
    // Initial frame size — gets overridden at start() time from the
    // captured policy snapshot. Default values seed m_frameBytes so
    // any pre-start() inspection works.
    m_frameBytes = m_frameWidth * m_frameHeight * 4;

    // React to camera-bridge death — when it crashes or is killed, our
    // gst-launch is reading from a stale PipeWire node and frames stop.
    // Kill our process too and let CameraWidget retry via deviceListChanged.
    connect(&CameraBridgeManager::instance(),
            &CameraBridgeManager::bridgeExited,
            this, [this](int code) {
        qCWarning(lcGstCam) << "camera-bridge exited (code" << code
                            << ") — stopping gst-launch and signaling retry";
        stop();
        // After bridge restart, virtual cams will re-register; CameraWidget
        // listens for deviceListChanged and will call start() again.
        QTimer::singleShot(2000, this, [this]() {
            emit deviceListChanged();
        });
    });

    // V32.34+: bridge reports it can't initialise libcamera. There's no
    // point in retrying — the user needs to install a package or fix
    // their device. Cancel any pending retry timers and surface a
    // clear, actionable error.
    connect(&CameraBridgeManager::instance(),
            &CameraBridgeManager::bridgeFailedToInitialize,
            this, [this](const QString &reason) {
        qCWarning(lcGstCam) << "Bridge failed to initialise:" << reason;
        m_bridgeWaitScheduled = false;
        m_consecutiveStartRetries = kMaxStartRetries + 1;  // disable further retries
        emit error(reason);
    });
}

GstLaunchBackend *GstLaunchBackend::makeProbe(QObject *parent)
{
    // V32.34+: probe-mode factory. CameraTester uses this to quickly
    // verify a node works without committing to a long-running session.
    // Probe mode means:
    //   • short timeouts (policy.probe*)
    //   • auto-stop after first frame
    //   • no framePreview emission (consumer reads via lastFrame())
    auto *b = new GstLaunchBackend(parent);
    b->m_probeMode = true;
    return b;
}

GstLaunchBackend::~GstLaunchBackend()
{
    stop();
}

QList<DeviceInfo> GstLaunchBackend::availableDevices() const
{
    QList<DeviceInfo> devs;
    // V32.33+: rely entirely on CameraRegistry which now does:
    //   1. virtual-cam-* scan (the bridge model)
    //   2. libcamera_input.* fallback (real names, no bridge needed)
    //   3. per-device overrides (env var, config file)
    //
    // We still own the speculative-listing-during-grace policy for the
    // virtual-cam path — if pw-dump returns nothing because the bridge
    // is still starting, show baseline cameras anyway so the dropdown
    // isn't empty for a few seconds.
    const auto &registry = CameraRegistry::instance();
    const auto avail = registry.available();
    for (const auto &cam : avail) {
        devs.append({ cam.virtualNodeId, cam.displayName });
    }
    if (!devs.isEmpty()) {
        qCInfo(lcGstCam) << "availableDevices: registry returned"
                         << devs.size() << "devices";
        return devs;
    }

    // Registry returned nothing — either pw-dump is still cold-booting
    // OR neither virtual-cam-* nor libcamera_input.* nodes are visible
    // yet. Apply the speculative grace policy for the virtual-cam path.
    constexpr qint64 kGraceMs = 5000;
    if (!m_graceTimer.isValid()) {
        m_graceTimer.start();
    }
    const bool inGrace = m_graceTimer.elapsed() < kGraceMs;
    qCInfo(lcGstCam) << "availableDevices: nothing found, in-grace:" << inGrace
                     << "elapsed-ms:" << m_graceTimer.elapsed();

    if (inGrace) {
        for (const auto &cam : registry.allKnown()) {
            if (cam.baseline) {
                devs.append({ cam.virtualNodeId, cam.displayName });
            }
        }
    }
    return devs;
}

QString GstLaunchBackend::defaultDeviceId() const
{
    // V32.33+: use the registry's primaryRear which now considers BOTH
    // virtual-cam-rear AND any libcamera_input.* node heuristically
    // classified as rear (FP5 fallback). This makes "default rear camera"
    // work on devices where the camera-bridge isn't operational.
    const auto rear = CameraRegistry::instance().primaryRear();
    if (rear) return rear->virtualNodeId;
    // No rear available — fall back to first device.
    const auto devs = availableDevices();
    if (!devs.isEmpty()) return devs.first().id;
    return {};
}

bool GstLaunchBackend::start(const QString &deviceId)
{
    // V32.36+: consume the recovery-restart flag once, up front. It's set by
    // our own recovery relaunches (settle / relaxed / GPU / quality) right
    // before their singleShot fires start(). Snapshotting + clearing here makes
    // it a reliable one-shot regardless of which early-return path this start()
    // takes, and lets the fresh-session reset below tell a real new session
    // apart from our own recovery re-entry.
    const bool recoveryRestart = m_isRecoveryRestart;
    m_isRecoveryRestart = false;

    // V32.34+: switch-cooldown gate. If we've started/restarted within
    // kSwitchCooldownMs, drop this call. Catches the "camera mode is
    // flapping" pattern where some upstream signal (health re-probe,
    // page focus race, user double-tap) fires start() many times in
    // quick succession. Each start() resets libcamera/pipewire
    // negotiation; if the sensor doesn't have time to settle into a
    // stable color mode, we observe the red/green/purple flapping the
    // user reported.
    //
    // We deliberately gate on the time-since-last-start regardless of
    // node, because even same-node restarts thrash libcamera. Caller-
    // visible: the call returns the previous m_active state and the
    // pipeline doesn't churn.
    //
    // Diagnostic counter m_droppedStarts increments so we can see in
    // the log how aggressively upstream is hammering us. Logged every
    // dropped call so the journal shows the rate.
    const qint64 nowMs = QDateTime::currentMSecsSinceEpoch();
    const qint64 sinceLast = nowMs - m_lastStartMs;
    if (m_lastStartMs > 0 && sinceLast < kSwitchCooldownMs) {
        ++m_droppedStarts;
        qCInfo(lcGstCam) << "start() COOLDOWN DROP — only" << sinceLast
                         << "ms since last start (cooldown"
                         << kSwitchCooldownMs << "ms). Total drops:"
                         << m_droppedStarts
                         << "requested-node:" << deviceId
                         << "currently-active:" << (m_active ? m_currentNode : QStringLiteral("(none)"));
        return m_active;
    }
    m_lastStartMs = nowMs;
    if (m_droppedStarts > 0) {
        qCInfo(lcGstCam) << "start() passing cooldown after" << sinceLast
                         << "ms quiet — previous burst dropped"
                         << m_droppedStarts << "calls";
        m_droppedStarts = 0;
    }

    // V32.33+ Phase 1 audit: if the caller explicitly passed a deviceId,
    // they made a CHOICE — don't auto-fall-back to a different node if
    // this one fails. Reset the retried flag only when called with
    // empty deviceId (the "default device, please" path).
    if (deviceId.isEmpty()) {
        m_retriedAlternateNode = false;
    } else {
        // User picked this specifically. Mark as already-retried so an
        // early-exit doesn't silently switch to a different camera.
        m_retriedAlternateNode = true;
    }
    // V32.35+: relaxed-caps retry bookkeeping. The relaxed retry path calls
    // start(sameNode) WITHOUT clearing m_relaxCaps (so the relaunch actually
    // uses relaxed caps). Any OTHER start — a different node, or the same node
    // requested fresh by the user/UI rather than by our retry timer — should
    // begin un-relaxed and reset the one-shot relaxed-retry guard. We detect
    // "this is our own relaxed retry" by m_relaxCaps already being true AND the
    // node being the one we just relaxed; otherwise it's a fresh request.
    if (!(m_relaxCaps && deviceId == m_lastRelaxedNode)) {
        m_relaxCaps = false;
        m_retriedRelaxed = false;
    }
    if (m_relaxCaps) {
        m_lastRelaxedNode = deviceId;  // remember which node we're relaxing
    }
    // V32.34+: log the requested switch with FULL context, so we can
    // correlate "start fired for the front cam" with what the
    // surrounding code was doing at the time. Goes alongside the
    // cooldown log above so each accepted start AND each rejected one
    // is in the journal.
    qCInfo(lcGstCam) << "start() ACCEPTED — requested-device:"
                     << (deviceId.isEmpty() ? QStringLiteral("(default)") : deviceId)
                     << "currently-active:" << (m_active ? m_currentNode : QStringLiteral("(none)"))
                     << "probe-mode:" << m_probeMode;
    m_lastStartNode = deviceId.isEmpty() ? QStringLiteral("(default)") : deviceId;
    QString node = deviceId.isEmpty() ? defaultDeviceId() : deviceId;
    // V32.34+: reset the auto-restart-used flag when the node changes.
    // The "first" mode of autoQualityRestart should mean "one restart
    // per user-initiated attempt at this node", not "one restart ever
    // for the backend's lifetime". When the user switches cameras,
    // they get a fresh quality-correction attempt.
    if (node != m_currentNode) {
        m_qualityRestartUsed = false;
        // V32.35+ AUDIT FIX: same per-session reset for the GPU-colorspace
        // latch. m_gpuConvertFailed was set to true on a GL-link failure but
        // NEVER reset anywhere — so a single TRANSIENT GPU failure (GL context
        // not ready during a fast switch, a momentary driver hiccup) forced the
        // slower CPU colorspace path for the ENTIRE app lifetime, across every
        // subsequent camera start, with no recovery short of restarting the app.
        // The FP5's normal path IS the GPU one (glupload/glcolorconvert/…), so
        // this silently degraded all future sessions. Reset on node change gives
        // a fresh GPU attempt per user-initiated session WITHOUT re-enabling it
        // during the same-node 100ms recovery retry (which keeps node ==
        // m_currentNode, so the latch correctly persists and we don't loop).
        m_gpuConvertFailed = false;
        // V32.35+ (device-logs): fresh node → fresh negotiation-settle retry.
        // NOT reset on our own same-node settle/relaxed re-entry (node ==
        // m_currentNode there), so a node that fails even after settling
        // progresses to relaxed→swap instead of looping the settle retry.
        m_negotiationSettleRetried = false;
        m_negotiationBackoffCount  = 0;   // V32.37+: fresh node → fresh backoff budget
    }
    // V32.36+ (device-logs FP5): ALSO reset the per-session settle-retry guard
    // when starting a FRESH session on the SAME node (start() while inactive).
    // BUG this fixes: stop() never clears m_currentNode, so resuming the camera
    // page on the same physical sensor (rear→background→rear) takes the
    // node==m_currentNode branch above and SKIPS the reset — inheriting a
    // m_negotiationSettleRetried=true left over from an earlier session's
    // recovery. The next not-negotiated then finds the guard already set, skips
    // the settle-retry entirely, and the transient failure becomes terminal
    // (observed: 19:18:28 + 19:18:57 camera_29 failed with NO "device-settle
    // retry" line, while the same sensor streamed fine before and 7 min later).
    // Gate on !m_active so this fires only for genuine new sessions, never
    // during our own same-node recovery relaunches (those keep m_active and the
    // settle/relax timers drive them), so we don't reopen a retry loop.
    if (!m_active && !recoveryRestart) {
        m_negotiationSettleRetried = false;
        m_retriedRelaxed = false;
        m_relaxCaps = false;
        m_negotiationBackoffCount = 0;   // V32.37+: fresh session → fresh backoff budget
    }
    if (node.isEmpty()) {
        // V32.33+: no virtual-cam AND no libcamera nodes visible. Try
        // spawning the bridge — but ONLY if it's actually installed.
        // On devices where the bridge isn't operational (FP5 with
        // current builds), this just fails and we want the user to
        // know to use the libcamera fallback by setting the
        // SNAPCLONE_CAMERA_REAR env var or editing the override file.
        qCInfo(lcGstCam) << "No virtual-cam or libcamera_input nodes "
                            "found — trying camera-bridge fallback";
        const bool spawned = CameraBridgeManager::instance().ensureRunning();
        if (!spawned) {
            emit error(tr("No camera nodes found. "
                          "If your device exposes libcamera_input.* nodes "
                          "in PipeWire, set SNAPCLONE_CAMERA_REAR=<node-name> "
                          "or edit ~/.config/snapclone/camera_overrides.ini "
                          "to specify them explicitly."));
            return false;
        }
        // Bridge is starting but nodes won't appear instantly. Schedule a retry
        // and notify caller via deviceListChanged once they show up.
        //
        // V32.34+: guard against stacked retries. Without the flag, each
        // failing start() call schedules a fresh 1500ms timer. The UI
        // typically calls start() from MULTIPLE event sources (becoming
        // visible, deviceListChanged signal, page-focus, etc.) — without
        // dedup we'd accumulate many timers all firing in sequence.
        //
        // Also cap consecutive retries. If we've tried N times and the
        // bridge never comes up, stop retrying — emit error and let the
        // user retry manually. Otherwise we'd churn forever on a
        // permanently-broken bridge install (missing libcamera, etc).
        if (m_bridgeWaitScheduled) {
            qCDebug(lcGstCam) << "Bridge wait already scheduled — not stacking another";
            return false;
        }
        if (++m_consecutiveStartRetries > kMaxStartRetries) {
            qCWarning(lcGstCam) << "Camera-bridge never produced nodes after"
                                << kMaxStartRetries << "attempts — giving up. "
                                << "Check: is libcamera installed? Did "
                                << "camera-bridge log a startup error?";
            m_consecutiveStartRetries = 0;
            emit error(tr("Camera unavailable — the camera-bridge isn't "
                          "publishing any PipeWire nodes. Check logs for "
                          "the bridge's startup error."));
            return false;
        }
        m_bridgeWaitScheduled = true;
        QTimer::singleShot(1500, this, [this]() {
            m_bridgeWaitScheduled = false;
            const QString n = defaultDeviceId();
            if (!n.isEmpty()) {
                emit deviceListChanged();  // tell UI to refresh selector
            }
        });
        return false;  // not yet active — caller should retry on deviceListChanged
    }

    // Successful path past the "is node available" check — reset
    // the consecutive-retry counter so a future failure doesn't
    // inherit the count from a prior unrelated outage.
    m_consecutiveStartRetries = 0;

    // Already running with the same node? No-op.
    if (m_active && m_currentNode == node) return true;

    // Kill previous process if switching nodes
    if (m_proc) {
        // V32.34+: disconnect FIRST. Without this, a queued
        // QProcess::finished signal from the old process can fire
        // after we've set up the new session — and the lambda
        // captures `this`, so it reads the NEW m_startTimer (which
        // we just started for the new session). That makes
        // m_startTimer.elapsed() < earlyExitMs trivially true, and
        // the lambda thinks the new session early-exited, triggering
        // a retry → potential infinite restart loop. Disconnecting
        // before kill ensures the old process's finished signal
        // (which kill() may synchronously emit) is a no-op.
        m_proc->disconnect(this);
        m_proc->kill();
        m_proc->waitForFinished(500);
        m_proc->deleteLater();
        m_proc = nullptr;
    }
    m_buf.clear();
    m_buf.squeeze();      // V32.37+: actually release the buffer capacity
    m_stderrBuf.clear();  // V32.34+: reset so stale errors don't bleed into new sessions
    m_firstFrameSeen = false;
    m_sessionFrameCount = 0;
    m_active = false;
    // V32.37+ memory: m_lastFrame holds a full ~3.7MB frame (used by
    // frameCaptured). It was never released on stop(), so a stopped/backgrounded
    // camera kept a whole frame resident — wasteful on a memory-constrained
    // phone. Capture has already consumed it by the time we stop; drop it.
    if (!m_lastFrame.isNull()) m_lastFrame = QImage();

    // V32.34+: reset defensive-quality tracking. m_qualityRestartUsed
    // is NOT reset here — the "first" mode is per-SESSION, but we
    // want it to persist across the auto-restart itself (so we don't
    // restart-restart-restart in a loop). It IS reset when the user
    // explicitly stops and picks a new device, via the public stop()
    // path resetting it. m_sourceCapsParsed and m_negotiated* DO reset
    // because the new session will renegotiate.
    m_sourceCapsParsed       = false;
    m_negotiatedSourceWidth  = 0;
    m_negotiatedSourceHeight = 0;
    // V32.34+: reset sink-caps parser too, so a fresh pipeline gets a
    // fresh "what did we negotiate" log line. The auto-swap decision
    // is RE-MADE every session (libcamera builds vary, the user may
    // have swapped to a different camera, etc).
    m_sinkCapsParsed         = false;
    // V32.34+ TILING FIX: clear negotiated sink dims so a fresh pipeline
    // re-parses them. Until the new caps line arrives the extraction loop
    // falls back to the requested m_frameWidth/m_frameHeight.
    m_sinkWidth   = 0;
    m_sinkHeight  = 0;
    m_sinkStride  = 0;
    m_inferredStride = 0;   // V32.34+: re-infer stride for the new pipeline
    // V32.35+: manual stride override for on-device tiling diagnosis. The
    // tiling bug (diagonal skew / repeat near bottom) is a row-stride mismatch:
    // the producer pads each row to an alignment the naive reader doesn't know.
    // We normally INFER the pad from buffer bloat, but if inference picks the
    // wrong value for a given board the tiling persists across reloads. This
    // env var lets you pin the true stride on-device to (a) confirm it IS a
    // stride bug and (b) discover the correct value, the same way
    // SNAPCLONE_CAMERA_ROTATION debugs rotation. Value is bytes-per-row.
    m_forcedStride = 0;
    {
        const QByteArray sv = qgetenv("SNAPCLONE_CAMERA_STRIDE");
        if (!sv.isEmpty()) {
            bool ok = false;
            const int parsed = sv.trimmed().toInt(&ok);
            if (ok && parsed > 0) {
                m_forcedStride = parsed;
                qCInfo(lcGstCam) << "Stride FORCED via SNAPCLONE_CAMERA_STRIDE ="
                                 << m_forcedStride << "bytes/row (overrides inference)";
            } else {
                qCWarning(lcGstCam) << "SNAPCLONE_CAMERA_STRIDE invalid:" << sv
                                    << "(want a positive integer, bytes/row)";
            }
        }
    }
    m_bufBloatLogged = false;
    {
        // Honor a user-forced override from settings. If the key isn't
        // present (default), m_swapRedBlueOnReceive starts false and
        // the sink-caps parser will auto-enable swap if the SINK shows
        // an RGB* format. If the key IS present, that wins — useful
        // when a phone's bytes-on-wire don't match the announced caps
        // (rare driver bug we've seen once on Phone Red).
        QSettings s;
        const QVariant v = s.value(QStringLiteral("camera/policy/swapRB"));
        if (v.isValid()) {
            m_swapRedBlueOnReceive = v.toBool();
            m_swapRBForced = true;   // user pinned it — auto-detect must not override
            qCInfo(lcGstCam) << "swap-R/B forced from settings:" << m_swapRedBlueOnReceive;
        } else {
            m_swapRedBlueOnReceive = false;  // auto-decide from sink caps
            m_swapRBForced = false;
        }
    }
    // Framerate tracking — clear samples; we'll re-accumulate.
    m_frameTimes.clear();
    m_framesSinceHealthCheck = 0;
    m_frameRateDegradedLogged = false;
    m_bufBloatLogged = false;

    // V32.34+: clean up stale framedrop watchdog from any prior start().
    // Without this, start()-after-start() leaks QTimers parented to us
    // (they'd only be reaped at backend destruction). Set to nullptr
    // after deleteLater so any in-flight queued timeout sees the null
    // and bails via the guard in stop().
    if (m_framedropTimer) {
        m_framedropTimer->stop();
        m_framedropTimer->deleteLater();
        m_framedropTimer = nullptr;
    }

    // V32.34+: snapshot the current policy. We DON'T re-read between
    // here and stop() — backends are insulated from live policy edits
    // (the user must restart capture to see new timeouts). This avoids
    // a class of bugs where the watchdog timer sees one value and the
    // early-exit handler sees another mid-flight.
    m_policy = CameraPolicyStore::instance().snapshot();
    m_frameWidth  = m_policy.captureWidth;
    m_frameHeight = m_policy.captureHeight;

    // V32.39+ PERF: clamp the preview/capture frame to previewMax* (aspect
    // preserved, scale DOWN only). Because the GPU glcolorscale produces this
    // target size BEFORE gldownload, the GPU→CPU readback, the CPU repack
    // videoconvert, and the per-frame QImage all shrink with s² — the biggest
    // single lever on preview latency. Capped here once at start(); the snap
    // path uses the same frame, so the cap is logged loudly (see field docs).
    if (m_policy.previewMaxWidth > 0 && m_policy.previewMaxHeight > 0
            && m_frameWidth > 0 && m_frameHeight > 0
            && (m_frameWidth > m_policy.previewMaxWidth
                || m_frameHeight > m_policy.previewMaxHeight)) {
        const double sx = static_cast<double>(m_policy.previewMaxWidth)  / m_frameWidth;
        const double sy = static_cast<double>(m_policy.previewMaxHeight) / m_frameHeight;
        const double s  = std::min(sx, sy);
        const int capW = std::max(80, static_cast<int>(m_frameWidth  * s));
        const int capH = std::max(80, static_cast<int>(m_frameHeight * s));
        qCInfo(lcGstCam) << "Preview cap: output" << m_frameWidth << "x" << m_frameHeight
                         << "→" << capW << "x" << capH
                         << "(fits" << m_policy.previewMaxWidth << "x" << m_policy.previewMaxHeight
                         << "; GPU scales pre-readback). NOTE: snaps use this size — set "
                            "camera/policy/previewMaxWidth=0 for full"
                         << m_policy.captureWidth << "x" << m_policy.captureHeight;
        m_frameWidth  = capW;
        m_frameHeight = capH;
    }

    // V32.34+ tint fix: stride alignment. The fd pipe carries raw bytes
    // with NO stride metadata; our QImage reader assumes bytesPerLine ==
    // width×4. If GStreamer pads each row up to a SIMD boundary (it can,
    // depending on the negotiated format and the element chain), the
    // reader drifts by (actual_stride − width×4) bytes per row → a
    // progressive diagonal color shear that looks like the resolution-
    // dependent green/red tint, and that doesn't survive transcode
    // (the transcode path re-reads via GStreamer's stride-aware meta).
    //
    // For packed 32-bit BGRx the stride is GST_ROUND_UP_4(width×4),
    // which equals width×4 whenever width×4 is a multiple of 4 — always
    // true. BUT some converter elements emit the GstVideoMeta with a
    // larger stride for alignment to 8/16/64 px regardless. Rounding the
    // output WIDTH down to a multiple of strideAlignPx (default 4, i.e.
    // 16 bytes for BGRx) guarantees the packed stride matches and the
    // meta won't pad. The few lost pixels (≤3) are invisible.
    if (m_policy.strideAlignPx > 1) {
        const int aligned = (m_frameWidth / m_policy.strideAlignPx)
                          * m_policy.strideAlignPx;
        if (aligned != m_frameWidth && aligned >= 80) {
            qCInfo(lcGstCam) << "Stride-aligning output width"
                             << m_frameWidth << "→" << aligned
                             << "(multiple of" << m_policy.strideAlignPx << ")";
            m_frameWidth = aligned;
        }
        // Height needn't be aligned — stride is per-row, height just
        // counts rows. But align it too for symmetry on transpose
        // (90° rotations swap W/H downstream).
        const int alignedH = (m_frameHeight / m_policy.strideAlignPx)
                           * m_policy.strideAlignPx;
        if (alignedH != m_frameHeight && alignedH >= 80) {
            m_frameHeight = alignedH;
        }
    }
    m_frameBytes  = m_frameWidth * m_frameHeight * 4;

    const QString gstBin = QStandardPaths::findExecutable(QStringLiteral("gst-launch-1.0"));
    if (gstBin.isEmpty()) {
        emit error(tr("gst-launch-1.0 not found in PATH"));
        return false;
    }

    m_currentNode = node;
    m_proc = new QProcess(this);

    // V32.33+: pipeline hardening.
    //
    // Changes from previous:
    //
    //   1. Bounded queue with leaky=downstream BEFORE videoconvert.
    //      Without this, a slow downstream (videoconvert+videoscale at
    //      full res) backs up and pipewiresrc starves, eventually
    //      stalling the entire pipeline. With leaky=downstream the queue
    //      drops old frames rather than blocking — preview gets a bit
    //      jittery on slow CPUs but never freezes.
    //
    //   2. No format constraint on the SOURCE side. pipewiresrc
    //      auto-negotiates with the node's native format. Forcing a
    //      format here (like the old "format=BGRx" caps before
    //      videoconvert) makes negotiation fail on libcamera_input
    //      nodes that emit NV12/YUY2/etc. videoconvert handles the
    //      conversion to BGRx for the sink.
    //
    //   3. Optional rotation via videoflip — controlled by env var
    //      SNAPCLONE_CAMERA_ROTATION=<none|90l|90r|180>. Some libcamera
    //      backends emit frames in sensor-native orientation (rotated
    //      90° from the screen). Without rotation handling the preview
    //      is sideways.
    //
    //   4. Explicit "qos=false" on the sink — prevents the sink from
    //      sending QoS upstream and asking pipewiresrc to drop frames
    //      that we WANT (we already drop via the queue's leaky policy).
    //
    // V32.34+: rotation resolution is layered:
    //   1. SNAPCLONE_CAMERA_ROTATION env var — debug / one-off override
    //   2. CameraPolicy::rotation — user's Settings choice
    //   3. None (no videoflip inserted)
    const QString rotEnv    = QString::fromUtf8(qgetenv("SNAPCLONE_CAMERA_ROTATION")).trimmed().toLower();
    const QString rotChosen = !rotEnv.isEmpty() ? rotEnv : m_policy.rotation.toLower();
    QString flipMethod;
    if (rotChosen == QLatin1String("90l") || rotChosen == QLatin1String("ccw"))
        flipMethod = QStringLiteral("counterclockwise");
    else if (rotChosen == QLatin1String("90r") || rotChosen == QLatin1String("cw"))
        flipMethod = QStringLiteral("clockwise");
    else if (rotChosen == QLatin1String("180"))
        flipMethod = QStringLiteral("rotate-180");

    QStringList args;
    // V32.34+: dropped "-q" — quiet mode SUPPRESSES warnings and most
    // error output. We need stderr to see negotiation failures, missing
    // nodes, plugin not found, etc. The stderr handler buffers + log-
    // category-gates the chatter, so verbose output costs nothing in
    // user-visible noise.
    //
    // V32.34+: -v turns on per-element verbose output, which emits the
    // negotiated caps ONCE at pipeline start in the form:
    //   /GstPipeline:pipeline0/GstPipeWireSrc:pipewiresrc0.GstPad:src:
    //     caps = video/x-raw,format=NV12,width=1280,height=720,...
    // We parse that line in the stderr handler to surface what the
    // SOURCE actually negotiated to. Costs a small fixed amount of
    // stderr output at session start (no per-frame cost).
    //
    // V32.34+ TILING FIX: this is now UNCONDITIONAL (was gated on
    // logFirstFrameInfo). The fdsink caps line it produces is the ONLY way we
    // learn the actual negotiated frame width/height/stride, which the
    // frame-extraction loop needs to slice the pipe on true frame boundaries.
    // Without it we'd guess from the requested dims and render tiled copies
    // whenever GStreamer negotiated a different size. The output is stderr-only
    // and log-category-gated, so there's no user-visible cost.
    args << QStringLiteral("-v");
    args << QStringLiteral("pipewiresrc")
         << QStringLiteral("target-object=") + node
         // do-timestamp=true: helps with libcamera sources that don't
         // produce proper timestamps. fdsink with sync=false ignores
         // them anyway but having them present prevents gst warnings
         // that flood the logs.
         << QStringLiteral("do-timestamp=true")
         << QStringLiteral("!");

    // V32.34+: source-target caps. Without these, pipewiresrc accepts
    // whatever resolution libcamera offered by default — which is
    // typically a sensor's LOWEST "preview" mode (320×240 / 640×480
    // depending on the driver). That's where "the camera works but
    // looks bad" comes from. Asking explicitly for sourceTargetWidth
    // × sourceTargetHeight @ sourceTargetFps drives libcamera to
    // negotiate a better-matching sensor mode.
    //
    // When any of these is 0 (or all of them), we DON'T add the caps
    // — that preserves the old behavior of "let the source pick" for
    // backends/cameras where the constraint would fail negotiation
    // entirely. Most modern phone sensors have 720p available; if
    // yours doesn't, set sourceTargetWidth to 0 in Settings.
    if (m_policy.sourceTargetWidth > 0 && m_policy.sourceTargetHeight > 0) {
        QString srcCaps = QStringLiteral("video/x-raw,width=%1,height=%2")
                              .arg(m_policy.sourceTargetWidth)
                              .arg(m_policy.sourceTargetHeight);
        if (m_policy.sourceTargetFps > 0) {
            // Framerate as fraction "N/1" — gst-launch caps syntax.
            srcCaps += QStringLiteral(",framerate=%1/1").arg(m_policy.sourceTargetFps);
        }
        args << srcCaps << QStringLiteral("!");
        qCInfo(lcGstCam) << "Source-target caps ENABLED:"
                         << m_policy.sourceTargetWidth << "x"
                         << m_policy.sourceTargetHeight
                         << "@" << m_policy.sourceTargetFps << "fps. "
                         << "If you see 'Format negotiation failed', the "
                         << "device's libcamera doesn't publish this mode; "
                         << "set sourceTargetWidth=0 in camera/policy to disable.";
    } else {
        qCInfo(lcGstCam) << "Source-target caps DISABLED — letting "
                         << "pipewiresrc negotiate freely with libcamera "
                         << "(default since V32.34+ device testing). "
                         << "To enable: edit ~/.config/SnapCloneProject/"
                         << "SnapClone.conf [camera/policy] sourceTargetWidth=W "
                         << "sourceTargetHeight=H sourceTargetFps=F";
    }

    // V32.39+ PERF: optional preview fps cap. Dropping excess frames HERE —
    // before glupload — means they never pay the GPU upload/convert/readback
    // cost. drop-only=true never duplicates, so a source slower than the cap is
    // untouched; placed ahead of the queue so the leaky queue still regulates
    // whatever survives. Gated on the element being present (gst-plugins-base,
    // normally is) so a stripped install can't break pipeline parsing.
    if (m_policy.previewMaxFps > 0
            && gstElementAvailable(QStringLiteral("videorate"))) {
        args << QStringLiteral("videorate")
             << QStringLiteral("drop-only=true")
             << QStringLiteral("max-rate=") + QString::number(m_policy.previewMaxFps)
             << QStringLiteral("!");
        qCInfo(lcGstCam) << "Preview fps cap ENABLED:" << m_policy.previewMaxFps
                         << "fps (videorate drop-only, before GPU stage).";
    }

    args << QStringLiteral("queue")
         << QStringLiteral("max-size-buffers=4")
         << QStringLiteral("max-size-time=0")
         << QStringLiteral("max-size-bytes=0")
         << QStringLiteral("leaky=downstream")
         << QStringLiteral("!");

    // V32.34+ GPU accel + tint fix: the colorspace-conversion + scale
    // segment is now built by buildConvertScaleSegment(), which chooses
    // a GPU path (glupload ! glcolorconvert ! glcolorscale ! gldownload)
    // when the GL elements are installed and policy allows, else falls
    // back to the deterministic dual-videoconvert CPU path. Both paths
    // end with an explicit `video/x-raw,format=BGRx,width=W,height=H !`
    // so the bytes reaching fdsink — and thus our QImage reader — are
    // unambiguous BGRx at exactly the dimensions we expect. Centralizing
    // the choice means the GPU/CPU decision and the tint-proofing live
    // in one auditable place. The segment ends with a trailing "!" so
    // it links straight into fdsink below.
    args << buildConvertScaleSegment(flipMethod);
    args << QStringLiteral("fdsink")
         << QStringLiteral("fd=1")
         << QStringLiteral("sync=false")
         << QStringLiteral("qos=false");
    // NOTE: async-handling was tried here in an earlier draft. fdsink
    // doesn't have that property (it's on GstBin, not GstBaseSink).
    // Adding it made gst-launch fail parsing the pipeline with
    // "WARNING: erroneous pipeline: no property \"async-handling\" in
    // element \"fdsink\"" — which masked every other failure and made
    // it look like gst-plugin-pipewire was missing. If you want async
    // behavior on the sink, use `async=false` (which IS a base sink
    // property) — but we don't need it here: fdsink with sync=false
    // already runs without blocking on clock.

    m_proc->setProcessChannelMode(QProcess::SeparateChannels);

    connect(m_proc, &QProcess::readyReadStandardOutput,
            this, &GstLaunchBackend::onStdoutData);
    connect(m_proc, &QProcess::readyReadStandardError, this, [this]() {
        // Null check: stop() sets m_proc=nullptr, but a queued signal could
        // still be in flight. Direct-connection signals can also fire during
        // QProcess::kill()'s synchronous teardown after we've started
        // tearing down state.
        if (!m_proc) return;
        const QByteArray err = m_proc->readAllStandardError();
        if (err.isEmpty()) return;
        qCDebug(lcGstCam) << "stderr:" << err.trimmed();

        // NOTE: GL-colorspace-link-failure detection + CPU fallback lives in
        // the finished() handler, NOT here. On a fast same-tick exit the
        // final stderr bytes can arrive only via the finished handler's
        // drain (not this readyRead slot), so the finished handler is the
        // reliable detection point. Handling it here too would race a
        // double-restart. We just log stderr here.

        // V32.34+: parse the negotiated source caps from gst-launch
        // verbose output. With -v, gst-launch emits a line like:
        //   /GstPipeline:pipeline0/GstPipeWireSrc:pipewiresrc0.GstPad:src: caps = video/x-raw,format=NV12,width=1280,height=720,framerate=30/1,...
        // We only want the pipewiresrc-src caps (FIRST element's
        // negotiated output, == the source resolution we actually got).
        // Match once per session via the m_sourceCapsParsed guard so
        // even pipelines with caps re-negotiations don't keep firing.
        if (!m_sourceCapsParsed) {
            const QByteArray needle = "pipewiresrc";
            const int idx = err.indexOf(needle);
            if (idx >= 0) {
                // Look for "caps = video/x-raw" later on the same line.
                const int eol = err.indexOf('\n', idx);
                const QByteArray line = err.mid(idx, eol < 0 ? -1 : (eol - idx));
                const int capsAt = line.indexOf("caps = video/x-raw");
                if (capsAt >= 0) {
                    // Extract width and height. The order in the caps
                    // string is not guaranteed; we search for each.
                    auto extractInt = [&line](const QByteArray &key) -> int {
                        const int at = line.indexOf(key);
                        if (at < 0) return 0;
                        const int valStart = at + key.size();
                        // Skip past "=" or "(int)" prefix gst-launch
                        // uses for typed values.
                        int p = valStart;
                        while (p < line.size() && (line[p] == '=' || line[p] == ' '
                                                || line[p] == '(' || (line[p] >= 'a' && line[p] <= 'z')
                                                || line[p] == ')')) ++p;
                        // Read digits.
                        int val = 0;
                        while (p < line.size() && line[p] >= '0' && line[p] <= '9') {
                            val = val * 10 + (line[p] - '0');
                            ++p;
                        }
                        return val;
                    };
                    const int negW = extractInt("width");
                    const int negH = extractInt("height");
                    // V32.34+: also extract the negotiated format.
                    // This is the single most useful diagnostic when
                    // green/red/purple tint appears — the user can
                    // see in the journal that pipewiresrc gave us
                    // (say) NV12 instead of the BGRx we asked for,
                    // and we know videoconvert downstream needs to
                    // handle that conversion.
                    QString negFormat;
                    {
                        const int at = line.indexOf("format");
                        if (at >= 0) {
                            // V32.34+ PARSER FIX: skip the "(string)" type tag
                            // (was extracting "string" as the format — see the
                            // matching fix in the sink-caps parser).
                            int p = at + 6;  // "format" len
                            while (p < line.size()) {
                                const char c = line[p];
                                if (c == '=' || c == ' ') { ++p; continue; }
                                if (c == '(') {
                                    while (p < line.size() && line[p] != ')') ++p;
                                    if (p < line.size()) ++p;
                                    continue;
                                }
                                break;
                            }
                            int end = p;
                            while (end < line.size()
                                   && ((line[end] >= 'A' && line[end] <= 'Z')
                                       || (line[end] >= 'a' && line[end] <= 'z')
                                       || (line[end] >= '0' && line[end] <= '9'))) ++end;
                            negFormat = QString::fromUtf8(line.mid(p, end - p));
                        }
                    }
                    // V32.39+: also pull the negotiated framerate numerator
                    // (caps print it as "framerate=(fraction)30/1"). This is how
                    // many frames/sec the source feeds into the expensive
                    // conversion path — i.e. the headroom a previewMaxFps cap
                    // would reclaim, and a clue if the sensor is stuck at an
                    // unexpected rate.
                    int negFps = 0;
                    {
                        const int at = line.indexOf("framerate");
                        if (at >= 0) {
                            int p = at + 9; // "framerate" len
                            while (p < line.size() && (line[p] == '=' || line[p] == ' '
                                    || line[p] == '(' || (line[p] >= 'a' && line[p] <= 'z')
                                    || line[p] == ')')) ++p;
                            while (p < line.size() && line[p] >= '0' && line[p] <= '9') {
                                negFps = negFps * 10 + (line[p] - '0'); ++p;
                            }
                        }
                    }
                    if (negW > 0 && negH > 0) {
                        m_sourceCapsParsed = true;
                        m_negotiatedSourceWidth  = negW;
                        m_negotiatedSourceHeight = negH;
                        qCInfo(lcGstCam) << "Negotiated source caps:"
                                         << negW << "x" << negH
                                         << "@" << (negFps > 0 ? negFps : 0) << "fps"
                                         << "format=" << (negFormat.isEmpty() ? QStringLiteral("(unknown)") : negFormat)
                                         << "(target was"
                                         << m_policy.sourceTargetWidth
                                         << "x" << m_policy.sourceTargetHeight << ")";
                        // V32.34+: low-quality detection. If the
                        // negotiated source is far below our target,
                        // optionally restart the pipeline ONCE to try
                        // again with a fresh negotiation — some
                        // libcamera drivers latch into preview mode
                        // and a restart clears it.
                        checkQualityAndMaybeRestart();
                    }
                }
            }
        }

        // V32.34+: ALSO parse the fdsink-side caps. The source caps tell
        // us what pipewiresrc gave us; the sink caps tell us what fdsink
        // is actually writing to stdout AFTER both videoconverts have
        // run. If the sink caps don't read `format=BGRx`, something
        // upstream is bypassing our caps filter (videoconvert running
        // as passthrough, perhaps because its sink-pad and src-pad
        // negotiated compatible-but-not-our-target caps). In that case
        // the bytes hitting our QImage Format_RGB32 decoder will be in
        // some other byte order — producing the red / purple / green
        // tint the user has been observing.
        //
        // Match-once via m_sinkCapsParsed. Logged at info so it appears
        // in the camera-app.txt grep that the journal-puller collects.
        if (!m_sinkCapsParsed) {
            const QByteArray needle = "fdsink";
            const int idx = err.indexOf(needle);
            if (idx >= 0) {
                const int eol = err.indexOf('\n', idx);
                const QByteArray line = err.mid(idx, eol < 0 ? -1 : (eol - idx));
                const int capsAt = line.indexOf("caps = video/x-raw");
                if (capsAt >= 0) {
                    // Reusable typed-int extractor (mirrors the source-caps
                    // parser): handles gst's "key=(int)NNN" / "key = NNN".
                    auto extractInt = [&line](const QByteArray &key) -> int {
                        const int at = line.indexOf(key);
                        if (at < 0) return 0;
                        int p = at + key.size();
                        while (p < line.size() && (line[p] == '=' || line[p] == ' '
                                                || line[p] == '(' || (line[p] >= 'a' && line[p] <= 'z')
                                                || line[p] == ')')) ++p;
                        int val = 0;
                        while (p < line.size() && line[p] >= '0' && line[p] <= '9') {
                            val = val * 10 + (line[p] - '0'); ++p;
                        }
                        return val;
                    };
                    QString sinkFormat;
                    const int at = line.indexOf("format", capsAt);
                    if (at >= 0) {
                        int p = at + 6;
                        // V32.34+ PARSER FIX: gst prints typed caps as
                        // `format=(string)BGRx`. The old scan stopped at the
                        // first alpha char — which is the 's' in "(string)" —
                        // so it extracted "string" as the format and the R/B
                        // swap auto-detect NEVER fired (startsWith("RGB") on
                        // "string" is false). Explicitly skip a "(string)" /
                        // "(...)" type tag and the '=' / spaces first, THEN read
                        // the real token.
                        // Skip separators and any parenthesised type tag.
                        while (p < line.size()) {
                            const char c = line[p];
                            if (c == '=' || c == ' ') { ++p; continue; }
                            if (c == '(') {            // skip "(type)" wholesale
                                while (p < line.size() && line[p] != ')') ++p;
                                if (p < line.size()) ++p; // consume ')'
                                continue;
                            }
                            break;                     // real value starts here
                        }
                        int end = p;
                        while (end < line.size()
                               && ((line[end] >= 'A' && line[end] <= 'Z')
                                   || (line[end] >= 'a' && line[end] <= 'z')
                                   || (line[end] >= '0' && line[end] <= '9'))) ++end;
                        sinkFormat = QString::fromUtf8(line.mid(p, end - p));
                    }
                    // V32.34+ TILING FIX: pull the ACTUAL negotiated dims off
                    // the same caps line. These — not the requested ones — are
                    // the true layout of the bytes in the pipe.
                    const int sinkW = extractInt("width");
                    const int sinkH = extractInt("height");

                    m_sinkCapsParsed = true;
                    qCInfo(lcGstCam) << "Negotiated SINK caps: format="
                                     << (sinkFormat.isEmpty() ? QStringLiteral("(unknown)") : sinkFormat)
                                     << "width=" << sinkW << "height=" << sinkH
                                     << "(requested" << m_frameWidth << "x" << m_frameHeight << ")";

                    // If GStreamer negotiated dimensions different from what we
                    // requested, our m_frameBytes / slice width / QImage stride
                    // were ALL wrong — that's the tiled-framebuffer bug. Adopt
                    // the real dims, recompute the frame size, and DROP the
                    // buffer so the next slice starts on a true frame boundary.
                    if (sinkW > 0 && sinkH > 0
                        && (sinkW != m_frameWidth || sinkH != m_frameHeight)) {
                        qCWarning(lcGstCam)
                            << "SINK dims differ from requested — re-syncing frame"
                            << "layout from" << m_frameWidth << "x" << m_frameHeight
                            << "to" << sinkW << "x" << sinkH
                            << "(this fixes tiled/garbled preview)";
                        m_sinkWidth   = sinkW;
                        m_sinkHeight  = sinkH;
                        m_frameWidth  = sinkW;
                        m_frameHeight = sinkH;
                        m_sinkStride  = sinkW * 4;   // tight BGRx assumption (NOT
                                                     // an announced stride — gst -v
                                                     // caps don't expose row padding;
                                                     // real padding is caught by the
                                                     // bloat inference below)
                        m_frameBytes  = m_frameWidth * m_frameHeight * 4;
                        // V32.38g TILING FIX: the layout just changed, so any
                        // stride inference done for the OLD dimensions is now
                        // invalid. Reset both the inferred stride AND the
                        // one-shot bloat latch so the NEW dimensions get a fresh
                        // inference pass. Without this, if frame data triggered
                        // the bloat path before this caps line was parsed, the
                        // latch stays set and padded stride for the new size is
                        // never detected → tiling persists for the whole session.
                        m_inferredStride = 0;
                        m_bufBloatLogged = false;
                        m_buf.clear();               // re-sync on next chunk
                    } else if (sinkW > 0 && sinkH > 0) {
                        m_sinkWidth  = sinkW;
                        m_sinkHeight = sinkH;
                        m_sinkStride = sinkW * 4;
                    }

                    // V32.38g tint-defense (corrected). We always NEGOTIATE
                    // BGRx, so the sink should read BGRx and Format_RGB32 (LE
                    // bytes B,G,R,x) is correct with NO swap. This block is the
                    // defensive fallback for when something upstream silently
                    // delivers a different packed-32 order. The R/B-swap fix
                    // (QImage::rgbSwapped) is ONLY correct for formats whose
                    // bytes are R,G,B,(pad) — i.e. R before B with the pad/alpha
                    // byte LAST (RGBx/RGBA). Formats with the pad/alpha byte
                    // FIRST (xRGB/ARGB/xBGR/ABGR) are NOT fixable by a simple R/B
                    // swap — they need a byte rotation we don't do here — so we
                    // must NOT pretend swapRB handles them (the old code set
                    // swapRB for "ARGB", which produced still-wrong colors).
                    if (!sinkFormat.isEmpty()) {
                        const QString u = sinkFormat.toUpper();
                        // No-swap set (already correct as Format_RGB32).
                        const bool noSwap  = u == QStringLiteral("BGRX")
                                          || u == QStringLiteral("BGRA");
                        // R/B-swap set (R before B, pad/alpha last).
                        const bool doSwap  = u == QStringLiteral("RGBX")
                                          || u == QStringLiteral("RGBA");
                        // Pad/alpha-first formats: cheap swap can't fix these.
                        const bool padFirst = u == QStringLiteral("XRGB")
                                           || u == QStringLiteral("ARGB")
                                           || u == QStringLiteral("XBGR")
                                           || u == QStringLiteral("ABGR");
                        qCInfo(lcGstCam) << "Sink format:" << sinkFormat
                                         << "→ noSwap=" << noSwap
                                         << "doSwap=" << doSwap
                                         << "padFirst=" << padFirst
                                         << "(override:" << m_swapRBForced << ")";
                        if (padFirst) {
                            // Don't silently mis-color. Surface it loudly; an
                            // operator can pin camera/policy/swapRB if their
                            // particular device's bytes happen to land right,
                            // but the real fix is forcing BGRx negotiation.
                            qCWarning(lcGstCam)
                                << "Sink negotiated a pad/alpha-FIRST format ("
                                << sinkFormat << ") which a simple R/B swap can't "
                                   "correct — colors may be wrong. Expected BGRx. "
                                   "Check the videoconvert caps / libcamera build.";
                        } else if (!m_swapRBForced && (noSwap || doSwap)) {
                            const bool swapRB = doSwap;   // noSwap → false
                            if (swapRB != m_swapRedBlueOnReceive) {
                                qCInfo(lcGstCam)
                                    << "Auto-setting R/B swap=" << swapRB
                                    << "from sink format" << sinkFormat;
                                m_swapRedBlueOnReceive = swapRB;
                            }
                        }
                    }
                }
            }
        }

        // V32.34+: keep the last N bytes of stderr (policy.stderrBufMaxBytes)
        // so we can include the actual reason in error() messages on early
        // exit. Append-then-truncate-from-front gives O(1) amortized cost.
        m_stderrBuf.append(err);
        const int maxBytes = m_policy.stderrBufMaxBytes;
        if (m_stderrBuf.size() > maxBytes) {
            m_stderrBuf.remove(0, m_stderrBuf.size() - maxBytes);
        }
    });
    connect(m_proc,
            QOverload<int, QProcess::ExitStatus>::of(&QProcess::finished),
            this, [this, node](int code, QProcess::ExitStatus) {
        qCInfo(lcGstCam) << "gst-launch exited:" << code << "node:" << node;
        // V32.34+: drain any stderr the QProcess buffered just before
        // exiting. The readyReadStandardError signal isn't guaranteed
        // to deliver the final bytes if the process exits in the same
        // event-loop tick as it printed its error. Without this, fast
        // failures (pipewiresrc rejected target-object → exit in <50ms)
        // arrive here with an empty m_stderrBuf and the user sees a
        // generic "early exit" instead of the actual GStreamer message.
        if (m_proc) {
            const QByteArray finalErr = m_proc->readAllStandardError();
            if (!finalErr.isEmpty()) {
                m_stderrBuf.append(finalErr);
                const int maxBytes = m_policy.stderrBufMaxBytes;
                if (m_stderrBuf.size() > maxBytes) {
                    m_stderrBuf.remove(0, m_stderrBuf.size() - maxBytes);
                }
            }
        }
        m_active = false;
        emit stopped();

        // V32.34+ GPU fallback (MUST come before the alternate-node retry
        // below). If the failure was the GL colorspace pipeline failing to
        // link (gldownload/glupload/glcolorconvert/glcolorscale "could not
        // link"), the problem is NOT the camera node — it's that this
        // device's GL stack can't negotiate the path. Swapping to a
        // different node just re-fails identically (observed on FP5: it
        // thrashed camera_29 ↔ camera_10 + PIP, never recovering). Instead:
        // latch m_gpuConvertFailed (forces the CPU path in
        // buildConvertScaleSegment for the rest of this backend's life) and
        // retry the SAME node. m_stderrBuf was just drained above so it
        // holds the final error text even on a same-tick exit.
        if (!m_gpuConvertFailed
            && (m_stderrBuf.contains("could not link gldownload")
                || m_stderrBuf.contains("could not link glupload")
                || m_stderrBuf.contains("could not link glcolorconvert")
                || m_stderrBuf.contains("could not link glcolorscale"))) {
            qCWarning(lcGstCam) << "GPU colorspace pipeline failed to link on"
                                << node << "— forcing CPU path and retrying"
                                << "the SAME node (not swapping cameras)";
            m_gpuConvertFailed = true;
            m_lastStartMs = 0;  // don't let the cooldown block the retry
            m_isRecoveryRestart = true;  // our own relaunch, not a fresh session
            QTimer::singleShot(100, this, [this, node]() {
                start(node);
            });
            return;
        }

        // V32.33+ Phase 1 audit: detect early exit within the policy
        // early-exit window — typically indicates the source node didn't
        // exist, had wrong format, or permissions were denied. Try the
        // next available node automatically before giving up, IF policy
        // allows it (autoRetryAlternate=true) and we haven't already.
        const int earlyExitMs = m_probeMode
            ? m_policy.probeEarlyExitWindowMs
            : m_policy.earlyExitWindowMs;

        // V32.36+ (device logs FP5): negotiation SAME-NODE recovery must run
        // even when m_retriedAlternateNode is true. When the camera page is
        // resumed with an EXPLICIT device id (the common case: swipe back to
        // camera, double-tap flip), start() sets m_retriedAlternateNode=true
        // ("user picked this node, don't auto-swap"). That flag also gated the
        // whole early-exit block below — so the negotiation settle-retry, which
        // lived inside it, NEVER ran on resume, and a transient not-negotiated
        // (-4) became a terminal failure (observed: camera_29 fails on every
        // resume, no "device-settle retry" line). The settle/relaxed retries are
        // SAME-NODE recoveries, not node swaps, so they belong outside the
        // alternate-node guard. Only the actual swap (further below) stays gated.
        const bool earlyExit =
            m_startTimer.isValid() && m_startTimer.elapsed() < earlyExitMs;
        if (earlyExit && m_policy.autoRetryAlternate) {
            QByteArray negTail = m_stderrBuf;
            const bool wasNegotiation =
                negTail.contains("not-negotiated")
                || negTail.contains("Could not negotiate")
                || negTail.contains("Format negotiation failed");
            constexpr int kNegotiationSettleMs = 600;
            if (wasNegotiation && !m_negotiationSettleRetried) {
                m_negotiationSettleRetried = true;
                qCInfo(lcGstCam) << "Negotiation failed on" << node
                                 << "→ device-settle retry (SAME node, normal caps) in"
                                 << kNegotiationSettleMs << "ms"
                                    " (transient device-state; runs regardless of"
                                    " explicit-node selection)";
                m_lastStartMs = 0;
                m_isRecoveryRestart = true;
                QTimer::singleShot(kNegotiationSettleMs, this, [this, sameNode = node]() {
                    start(sameNode);
                });
                return;
            }
            if (wasNegotiation && !m_relaxCaps && !m_retriedRelaxed) {
                m_retriedRelaxed = true;
                m_relaxCaps = true;
                m_lastRelaxedNode = node;
                qCInfo(lcGstCam) << "Negotiation still failing on" << node
                                 << "→ retrying SAME node with relaxed caps "
                                    "(no width/height pin) before any camera swap";
                m_lastStartMs = 0;
                m_isRecoveryRestart = true;
                QTimer::singleShot(kNegotiationSettleMs, this, [this, sameNode = node]() {
                    start(sameNode);
                });
                return;
            }
            // V32.37+ device-log fix: settle + relaxed both failed. Before
            // swapping cameras (which on a 2-sensor phone shows the WRONG one),
            // keep retrying the SAME node with a growing backoff — the FP5's
            // node settles within ~10-15s and then works first-try, but three
            // rapid attempts in ~1.5s all fail. Bounded so it can't loop forever.
            if (wasNegotiation && m_negotiationBackoffCount < kNegotiationBackoffMax) {
                ++m_negotiationBackoffCount;
                const int mult  = 1 << (m_negotiationBackoffCount - 1);
                const int delay = std::min(kNegotiationBackoffBaseMs * mult,
                                           kNegotiationBackoffCapMs);
                qCInfo(lcGstCam) << "Negotiation still failing on" << node
                                 << "→ backoff retry" << m_negotiationBackoffCount
                                 << "of" << kNegotiationBackoffMax << "in" << delay
                                 << "ms (SAME node; sensor still settling)";
                m_lastStartMs = 0;
                m_isRecoveryRestart = true;
                m_relaxCaps = true;  // keep relaxed caps through the backoff
                m_lastRelaxedNode = node;
                QTimer::singleShot(delay, this, [this, sameNode = node]() {
                    start(sameNode);
                });
                return;
            }
            // Negotiation recovery exhausted (or not a negotiation failure):
            // fall through to the alternate-node logic below, which IS gated by
            // m_retriedAlternateNode (an explicit user node choice suppresses
            // the auto-swap, as before).
        }

        if (m_startTimer.isValid() && m_startTimer.elapsed() < earlyExitMs
            && !m_retriedAlternateNode
            && m_policy.autoRetryAlternate) {
            // V32.34+: surface the real reason from buffered stderr.
            // Pattern-match common GStreamer error strings to give
            // actionable advice instead of "Resource error".
            //
            // Order matters: MORE-SPECIFIC patterns first, then GENERIC.
            // A missing pipewiresrc node produces stderr containing BOTH
            // "no element named X found" AND "erroneous pipeline", but
            // the actionable fix is "node missing", not "install plugins".
            //
            // We also append the raw stderr tail to whatever hint we
            // surface — the heuristic match might pick the wrong template,
            // but the raw text gives the user (and us, in bug reports)
            // the actual GStreamer failure reason.
            const QString stderrTail = QString::fromUtf8(m_stderrBuf).trimmed();
            if (!stderrTail.isEmpty()) {
                qCWarning(lcGstCam) << "Early exit stderr tail:" << stderrTail;
                QString hint;
                // Most specific first: "no element named ... found" is
                // pipewiresrc's "your target-object doesn't exist". This
                // is the dominant failure mode on FP5 when the bridge
                // hasn't published virtual-cam-* yet.
                if (stderrTail.contains(QStringLiteral("no element named"))
                    || stderrTail.contains(QStringLiteral("No matching node"))
                    || stderrTail.contains(QStringLiteral("Failed to find target"))
                    || stderrTail.contains(QStringLiteral("could not be found"))
                    || stderrTail.contains(QStringLiteral("Target object not found"))
                    || stderrTail.contains(QStringLiteral("Couldn't find target"))
                    || stderrTail.contains(QStringLiteral("no such target"))
                    || stderrTail.contains(QStringLiteral("Cannot find target"))) {
                    hint = tr("PipeWire node '%1' not found — bridge may not "
                              "be running, or the device unplugged. Check "
                              "`pw-dump | grep node.name` for live nodes.").arg(node);
                } else if (stderrTail.contains(QStringLiteral("Could not negotiate"))
                           || stderrTail.contains(QStringLiteral("not-negotiated"))) {
                    hint = tr("camera negotiation failed — the device may not "
                              "support the requested format. Try a different "
                              "Capture width/height in camera Tuning, or "
                              "SNAPCLONE_CAMERA_ROTATION=none to disable rotation.");
                } else if (stderrTail.contains(QStringLiteral("Resource busy"))
                           || stderrTail.contains(QStringLiteral("Device or resource busy"))) {
                    hint = tr("camera is busy — another process is using it. "
                              "Try `fuser -v /dev/video*` to find the user.");
                } else if (stderrTail.contains(QStringLiteral("Permission denied"))) {
                    hint = tr("permission denied opening the camera — check "
                              "/dev/video* group membership (usually 'video').");
                } else if (stderrTail.contains(QStringLiteral("no such element"))
                           || stderrTail.contains(QStringLiteral("No such element"))) {
                    // Reached only if NONE of the more-specific patterns
                    // above hit. Genuine missing-element case (vs the
                    // missing-node case that also includes this string).
                    hint = tr("a required GStreamer element is missing — "
                              "install gst-plugin-pipewire and gst-plugins-good.");
                } else if (stderrTail.contains(QStringLiteral("WARNING: erroneous pipeline"))) {
                    // Last-resort generic — erroneous pipeline could mean
                    // anything. Surface the raw stderr without claiming
                    // we know the cause.
                    hint = tr("camera pipeline failed: %1").arg(stderrTail.left(300));
                }
                if (!hint.isEmpty()) {
                    // Always include a short raw stderr tail so users
                    // reporting bugs have the actual GStreamer message,
                    // not just our translation.
                    const QString rawTail = stderrTail.right(200);
                    emit error(hint + QStringLiteral("\n[gst: %1]").arg(rawTail));
                } else {
                    // No template matched — surface the raw stderr so the
                    // user has SOMETHING to act on (and we get a clean
                    // bug report).
                    emit error(tr("camera failed: %1").arg(stderrTail.left(500)));
                }
            }

            m_retriedAlternateNode = true;  // one retry per session
            // V32.35+ (REVISED per device logs): not-negotiated (-4) on this
            // device is a DEVICE-STATE / TIMING failure, NOT a caps failure.
            // Evidence: the SAME node streams 1280x720 fine moments before/after;
            // it fails transiently right after a stop/start or a PIP session
            // (two concurrent libcamera opens). Crucially, the prior "relax the
            // width/height pins and retry in 100ms" recovery ALSO failed in the
            // logs (16:00:01.880: relaxed pipeline, still -4) — proving caps
            // aren't the cause and 100ms isn't enough for libcamera to release
            // and reset the sensor. So the recovery ladder is now:
            //   1st: wait a real SETTLE window, then retry the SAME node with
            //        NORMAL caps (give libcamera time to free the device).
            //   2nd: if that still fails, THEN try relaxed caps (cheap, in case
            //        some device really is caps-picky).
            //   3rd: only then fall back to the other camera.
            const bool wasNegotiation =
                stderrTail.contains(QStringLiteral("not-negotiated"))
                || stderrTail.contains(QStringLiteral("Could not negotiate"))
                || stderrTail.contains(QStringLiteral("Format negotiation failed"));
            // Settle delay: long enough for libcamera/pipewire to release the
            // media device after the failed pipeline's teardown. 100ms was too
            // short (logs); 600ms comfortably clears the release on the FP5.
            constexpr int kNegotiationSettleMs = 600;
            if (wasNegotiation && !m_negotiationSettleRetried) {
                m_negotiationSettleRetried = true;   // one settle-retry per session
                m_retriedAlternateNode = false;      // this isn't the alt-node retry
                qCInfo(lcGstCam) << "Negotiation failed on" << node
                                 << "→ device-settle retry (SAME node, normal caps) in"
                                 << kNegotiationSettleMs << "ms"
                                    " (not-negotiated is transient device-state here)";
                m_lastStartMs = 0;
                m_isRecoveryRestart = true;  // our own relaunch, not a fresh session
                QTimer::singleShot(kNegotiationSettleMs, this, [this, sameNode = node]() {
                    start(sameNode);
                });
                return;
            }
            if (wasNegotiation && !m_relaxCaps && !m_retriedRelaxed) {
                m_retriedRelaxed = true;
                m_relaxCaps = true;            // next buildPipeline omits w/h pins
                m_lastRelaxedNode = node;      // mark so start() keeps relax for THIS node
                m_retriedAlternateNode = false; // a fresh relaxed attempt, not the alt-node retry
                qCInfo(lcGstCam) << "Negotiation still failing on" << node
                                 << "→ retrying SAME node with relaxed caps "
                                    "(no width/height pin) before any camera swap";
                m_lastStartMs = 0;
                m_isRecoveryRestart = true;  // our own relaunch, not a fresh session
                QTimer::singleShot(kNegotiationSettleMs, this, [this, sameNode = node]() {
                    start(sameNode);
                });
                return;
            }
            // V32.37+ device-log fix (mirror of the primary block): bounded
            // backoff on the SAME node before swapping — bridges the FP5's
            // ~10-15s sensor-settle gap.
            if (wasNegotiation && m_negotiationBackoffCount < kNegotiationBackoffMax) {
                ++m_negotiationBackoffCount;
                const int mult  = 1 << (m_negotiationBackoffCount - 1);
                const int delay = std::min(kNegotiationBackoffBaseMs * mult,
                                           kNegotiationBackoffCapMs);
                m_retriedAlternateNode = false;
                qCInfo(lcGstCam) << "Negotiation still failing on" << node
                                 << "→ backoff retry" << m_negotiationBackoffCount
                                 << "of" << kNegotiationBackoffMax << "in" << delay
                                 << "ms (SAME node; sensor still settling)";
                m_lastStartMs = 0;
                m_isRecoveryRestart = true;
                m_relaxCaps = true;
                m_lastRelaxedNode = node;
                QTimer::singleShot(delay, this, [this, sameNode = node]() {
                    start(sameNode);
                });
                return;
            }
            const auto devs = availableDevices();
            for (const auto &d : devs) {
                if (d.id == node) continue;
                qCInfo(lcGstCam) << "Early exit on" << node
                                 << "→ retrying with" << d.id;
                // V32.34+: tell the UI the active node changed so it
                // can update the dropdown, show a "switched to X"
                // banner, etc. The UI must NOT call setSelection
                // back on the backend in response (would loop) —
                // it should only reflect the new state visually.
                emit activeNodeChanged(
                    node, d.id,
                    tr("Original camera failed to start; falling back."));
                // V32.34+: reset m_lastStartMs so the alternate-node
                // retry doesn't get blocked by the cooldown. The
                // start() that just failed-early shouldn't gate the
                // next genuine attempt — that would make adaptive
                // fallback unreliable. A 100ms delay is enough for
                // the pipewire node tree to settle after the previous
                // pipeline's teardown.
                m_lastStartMs = 0;
                QTimer::singleShot(100, this, [this, alt = d.id]() {
                    start(alt);
                });
                return;
            }
        }
    });

    // V32.34+: log the full command we're about to run, at Info level.
    // Indispensable when the pipeline fails — the user can copy-paste
    // the same gst-launch invocation into a shell and see the exact
    // GStreamer error themselves, plus we get it in bug reports.
    qCInfo(lcGstCam) << "gst-launch command:" << gstBin
                     << args.join(QChar(' '));

    m_proc->start(gstBin, args);
    const int startupMs = m_probeMode
        ? m_policy.probeStartupTimeoutMs
        : m_policy.startupTimeoutMs;
    if (!m_proc->waitForStarted(startupMs)) {
        emit error(tr("gst-launch failed to start within %1 ms").arg(startupMs));
        return false;
    }
    m_active = true;
    m_startTimer.start();
    qCInfo(lcGstCam) << "Started gst-launch on" << node
                     << "(BGRx" << m_frameWidth << "x" << m_frameHeight
                     << ")  probeMode=" << m_probeMode
                     << "first-frame-timeout="
                     << (m_probeMode ? m_policy.probeFirstFrameTimeoutMs
                                     : m_policy.firstFrameTimeoutMs) << "ms";

    // V32.34+: first-frame watchdog.
    //
    // PROBE MODE: a single QTimer::singleShot fires once at the
    // probeFirstFrameTimeoutMs deadline. Probe sessions terminate as
    // soon as the first frame arrives, so this is the only watchdog
    // they need.
    //
    // NORMAL MODE: the m_framedropTimer (built below) handles BOTH
    // the no-first-frame case AND the post-startup stall case via
    // phase-switching. The framedrop timer's interval starts at
    // firstFrameTimeoutMs and switches to framedropTimeoutMs once
    // m_firstFrameSeen flips inside onStdoutData. We don't arm a
    // separate singleShot here for normal mode — it would double-
    // fire with the framedrop timer and emit error() twice.
    if (m_probeMode) {
        const int firstFrameMs = m_policy.probeFirstFrameTimeoutMs;
        QTimer::singleShot(firstFrameMs, this, [this, node, firstFrameMs]() {
            if (!m_active || m_currentNode != node) return;  // already moved on
            if (m_firstFrameSeen) return;                    // got frames, OK
            qCWarning(lcGstCam) << "Probe first-frame watchdog: no frames from"
                                << node << "after" << firstFrameMs << "ms";
            emit error(tr("camera %1 produced no frames within %2 ms")
                       .arg(node).arg(firstFrameMs));
            if (m_proc) m_proc->kill();
        });
    }

    // V32.34+: stall-detection watchdog. Distinct from the first-frame
    // watchdog above — that one catches "never started"; this one
    // catches "started, ran for a while, then went silent" (USB cam
    // unplugged, kernel module crashed, PipeWire reconnect drop).
    //
    // Probe mode skips this — probes already exit after the first
    // frame so a stall watchdog has nothing to watch. Real captures
    // arm it after start() and onStdoutData rearms on every byte
    // arrival from gst-launch.
    //
    // The timer is single-shot and rearmed manually each time data
    // shows up. We don't use an "interval" timer because frame arrival
    // is bursty (queue depth, scheduler jitter), and a periodic check
    // would either miss late frames (interval > gap) or fire spuriously
    // (interval < gap). Manual rearm tracks the actual no-data interval.
    if (!m_probeMode) {
        m_framedropTimer = new QTimer(this);
        m_framedropTimer->setSingleShot(true);
        m_framedropTimer->setInterval(m_policy.framedropTimeoutMs);
        connect(m_framedropTimer, &QTimer::timeout, this, [this, node]() {
            // Bail if we already moved on (different node, or already
            // stopped). m_active check covers the after-stop race.
            if (!m_active || m_currentNode != node) return;
            qCWarning(lcGstCam) << "Framedrop watchdog: no data from"
                                << node << "for" << m_policy.framedropTimeoutMs
                                << "ms — source stalled";
            emit error(tr("camera %1 stopped delivering frames "
                          "(no data for %2 ms). Was the camera disconnected?")
                       .arg(node).arg(m_policy.framedropTimeoutMs));
            if (m_proc) {
                // Tear down the process; the finished handler will
                // emit stopped() and (per autoRetryAlternate) try the
                // next node.
                m_proc->kill();
            }
        });
        // First arm: after start, we have firstFrameTimeoutMs to receive
        // ANY data. Until the first frame arrives, the first-frame
        // watchdog above is in charge. After the first frame, the
        // framedrop timer takes over with the shorter framedropTimeoutMs.
        // We arm it now with firstFrameTimeoutMs as the initial deadline
        // so a stall BEFORE the first frame is also caught (otherwise
        // there's a race where the first-frame watchdog fires AFTER the
        // framedrop one with stale state).
        m_framedropTimer->setInterval(m_policy.firstFrameTimeoutMs);
        m_framedropTimer->start();
    }

    return true;
}

void GstLaunchBackend::stop()
{
    // V32.34+: log stop() with current state so the journal shows the
    // full lifecycle alongside start() logs. Together with the
    // start-cooldown logging this answers "is the camera flapping?"
    // by showing every start/stop pair with timestamps.
    qCInfo(lcGstCam) << "stop() called — was-active:" << m_active
                     << "current-node:" << m_currentNode
                     << "frames-seen:" << m_sessionFrameCount
                     << "(fps-window:" << m_frameTimes.size() << ")";
    // V32.34+: shut down the stall watchdog. Done BEFORE killing the
    // process so a queued timer fire can't race a stop()->stopped()
    // sequence and emit error() against an already-dead session.
    if (m_framedropTimer) {
        m_framedropTimer->stop();
        m_framedropTimer->deleteLater();
        m_framedropTimer = nullptr;
    }
    if (!m_proc) { m_active = false; return; }
    // Disconnect ALL signals from m_proc to us BEFORE killing. This prevents
    // any final readyRead* / finished signal from firing into our slots
    // during the destruction window. Without this, a queued signal could
    // dereference m_proc after we've nulled it (or worse, after the QProcess
    // is partway through deletion).
    QObject *deadProc = m_proc;
    deadProc->disconnect(this);
    m_proc = nullptr;     // null FIRST so any miss matches our nullptr guards
    m_buf.clear();
    // Now kill and let Qt clean up. We don't waitForFinished — that runs a
    // local event loop, which can re-enter our handler stack if the caller
    // (e.g. CameraWidget::hideEvent fired during a swipe) is mid-operation
    // on the same call stack as another dialog or animation. The cost of
    // not waiting: gst-launch's stderr/stdout may briefly continue producing
    // bytes that QProcess buffers harmlessly until deleteLater runs.
    //
    // CRITICAL: reparent BEFORE deleteLater. If stop() is called from our
    // destructor, the QObject parent-child cleanup would synchronously
    // delete the QProcess while gst-launch is still being reaped by the
    // OS — producing "QProcess: Destroyed while process running" warnings.
    // Reparenting orphans the QProcess so it survives until deleteLater
    // runs, by which time the stop request has been processed.
    //
    // V32.34+: the signal to the child differs by path. At shutdown we SIGKILL
    // and reap synchronously (speed + we're tearing down anyway). During normal
    // operation we terminate() gracefully so gst-launch's -e flag can flush a
    // clean EOS, and we delete only once it has actually finished (see below) —
    // this is what removes the "Destroyed while process still running" warning.
    if (QCoreApplication::closingDown()) {
        auto *proc = static_cast<QProcess *>(deadProc);
        proc->kill();
        if (proc->state() != QProcess::NotRunning)
            proc->waitForFinished(200);   // SIGKILL already sent; reaps fast
        delete proc;                       // signals already disconnected above
    } else {
        // Normal operation: orphan so the QProcess survives until it is
        // actually reaped, then delete. The previous version called kill()
        // + bare deleteLater(), but kill() is ASYNCHRONOUS — by the time the
        // queued deleteLater ran on the next loop pass, the OS often hadn't
        // finished reaping gst-launch yet, so ~QProcess fired the cosmetic
        // "Destroyed while process still running" warning seen on every
        // camera stop in the FP5 logs.
        //
        // V32.34+: instead, tie the delete to the process ACTUALLY finishing.
        // We connect the process's own finished() to its own deleteLater()
        // (a self-connection that does NOT involve `this`, so it's safe after
        // the disconnect(this) above), then ask it to stop. deleteLater now
        // runs only after the child is reaped — no warning, and still no
        // blocking waitForFinished() on this call stack (preserving the
        // swipe-triggered-hideEvent re-entrancy avoidance documented above).
        auto *proc = static_cast<QProcess *>(deadProc);
        proc->setParent(nullptr);          // orphan; survives past this scope
        QObject::connect(proc,
                         QOverload<int, QProcess::ExitStatus>::of(&QProcess::finished),
                         proc, &QObject::deleteLater);
        // Belt-and-suspenders: if for some reason finished() never arrives
        // (e.g. the child is wedged), a one-shot timer still deletes it so we
        // don't leak the orphan.
        QTimer::singleShot(3000, proc, [proc]() {
            if (proc->state() != QProcess::NotRunning) proc->kill();
            proc->deleteLater();
        });
        // gst-launch was started with -e (send EOS on SIGINT) so a graceful
        // terminate() lets it finalize; if it ignores that, the timer kills it.
        proc->terminate();
    }
    if (m_active) {
        m_active = false;
        emit stopped();
    }
}

void GstLaunchBackend::onStdoutData()
{
    if (!m_proc) return;
    m_buf.append(m_proc->readAllStandardOutput());

    // V32.34+: rearm the stall watchdog on any data arrival. The
    // interval may have switched from firstFrameTimeoutMs to
    // framedropTimeoutMs once m_firstFrameSeen flipped — the timer
    // is set BELOW after the first-frame block, so the very first
    // call here still uses the longer first-frame interval (correct,
    // since the first frame might not arrive in the very first chunk).
    if (m_framedropTimer && m_framedropTimer->isActive()) {
        m_framedropTimer->start();  // restart with current interval
    }

    // V32.34+: buffer-bloat detection. If m_buf grows much larger than
    // a single frame, it means we're misaligned — most likely because
    // m_frameBytes doesn't match what gst-launch is actually producing
    // (caps negotiation landed on a different resolution or stride).
    // Without this check, we'd silently render garbage in perpetuity
    // because the loop below reads m_frameBytes at a time off a buffer
    // that's not actually frame-aligned.
    //
    // Heuristic: more than 3 frames buffered = something's off.
    // gst-launch with sync=false on fdsink shouldn't queue frames;
    // they get consumed at the pace we read. >3 frames worth means
    // either (a) we read slower than 30fps (unlikely on a modern
    // GUI thread) or (b) we're not consuming the bytes correctly.
    constexpr int kBufBloatFactor = 3;
    if (m_forcedStride > 0) {
        // V32.35+: a manual stride override is pinned — skip inference entirely.
        // The slice/QImage already use m_forcedStride; don't let the bloat
        // heuristic fight the value the user is deliberately testing.
    } else if (m_buf.size() > m_frameBytes * kBufBloatFactor && !m_bufBloatLogged) {
        m_bufBloatLogged = true;
        // V32.34+ TILING/REPEAT FIX: bloat means the REAL frame is LARGER than
        // the tight m_frameBytes — gst is emitting a PADDED row stride. Find the
        // smallest alignment whose padded frame size divides the buffer cleanly,
        // store it as m_inferredStride, and re-slice from the next chunk at the
        // correct stride. This is what stops the diagonal drift / repeat-near-
        // bottom: previously we only LOGGED candidates and dropped the buffer.
        const int w = m_frameWidth, h = m_frameHeight;
        QString inferred;
        int chosenStride = 0;
        qint64 chosenFrame = 0;
        // V32.35+ CORRECTNESS FIX: this inference MUST match the tested authority
        // snapclone-videosrc::infer_stride / best_stride_guess. That module uses
        // EXACT division (observed_total % frame_size == 0) — a buffer that is a
        // whole number of padded frames. The inline copy here had drifted to a
        // FUZZY test (`% padFrame < padFrame/8`), which could adopt a WRONG
        // alignment whenever the buffer happened to land within 12.5% of a
        // multiple — a false positive that itself CAUSES tiling by slicing at a
        // bad stride. Restored to exact division to match the tested logic.
        // (Follow-up: drive this from the crate via FFI so the two can't drift
        // again — same treatment as the menu/request models.)
        for (int alignBytes : {16, 32, 64, 128, 256}) {
            const int tightRow = w * 4;
            const int padRow = ((tightRow + alignBytes - 1) / alignBytes) * alignBytes;
            const qint64 padFrame = static_cast<qint64>(padRow) * h;
            if (padFrame != m_frameBytes
                && (m_buf.size() % padFrame) == 0) {  // EXACT whole frames (matches Rust)
                inferred += QStringLiteral(" [align=%1B → row=%2 frame=%3]")
                                .arg(alignBytes).arg(padRow).arg(padFrame);
                if (chosenStride == 0) { // smallest clean alignment wins
                    chosenStride = padRow;
                    chosenFrame  = padFrame;
                }
            }
        }
        qCWarning(lcGstCam).noquote()
            << "Frame buffer bloat:" << m_buf.size() << "bytes, expected"
            << m_frameBytes << "/frame (" << w << "x" << h << "x4)."
            << "Pipeline output has a PADDED stride."
            << (inferred.isEmpty()
                    ? QStringLiteral("Could not infer a clean stride candidate.")
                    : QStringLiteral("Stride candidates:") + inferred);
        if (chosenStride > 0) {
            // Adopt the inferred stride for ALL subsequent frames; align the
            // buffer to a frame boundary so the next slice starts clean.
            m_inferredStride = chosenStride;
            qCInfo(lcGstCam) << "Adopting inferred row stride" << chosenStride
                             << "(padded frame" << chosenFrame << "bytes) —"
                             << "preview should stop repeating/tiling.";
            const qint64 rem = m_buf.size() % chosenFrame;
            if (rem > 0) m_buf.remove(0, rem); // drop partial leading frame
        } else {
            // No clean candidate — fall back to the old behavior (drop + retry).
            m_buf.clear();
        }
        return;
    }

    // V32.34+ STRIDE FIX (repeat-near-bottom bug): the per-frame SLICE size and
    // the QImage bytesPerLine must agree on the SAME stride, or every frame
    // drifts and earlier rows reappear at the bottom. Previously m_frameBytes
    // was always the TIGHT size (w*h*4) while the real pipe bytes are padded
    // (each row rounded up to an alignment boundary), so the slice was too short
    // → progressive diagonal drift → repeat near the bottom. m_inferredStride
    // (set in the bloat path below, 0 = "use tight") drives BOTH now.
    // V32.35+: a manual SNAPCLONE_CAMERA_STRIDE override wins over everything —
    // it's the on-device diagnosis hatch. Otherwise use the inferred padded
    // stride, then the sink-announced stride, then the tight fallback.
    const int rowStride = (m_forcedStride > 0)
                              ? m_forcedStride
                              : (m_inferredStride > 0)
                                  ? m_inferredStride
                                  : (m_sinkStride > 0 ? m_sinkStride : m_frameWidth * 4);
    const int frameH    = (m_sinkHeight > 0) ? m_sinkHeight : m_frameHeight;
    const qint64 sliceBytes = static_cast<qint64>(rowStride) * frameH;

    // V32.37+ PERF — frame coalescing. fdsink with sync=false writes frames as
    // fast as the camera produces them; if the GUI thread ever falls behind (a
    // heavy paint, a stride re-inference, a GC-like pause), MULTIPLE complete
    // frames pile up in m_buf and a naive "emit every frame" loop would decode +
    // copy (~3MB each) + run the full rotate/mirror/scale/pixmap cascade for
    // ALL of them — even though a LIVE preview only ever wants the newest. Worse,
    // that wasted work on the UI thread makes the backlog deeper: a feedback
    // loop that turns one hitch into sustained lag.
    //
    // So: if more than one whole frame is buffered, FAST-FORWARD past the stale
    // ones (drop their bytes, no decode/copy/emit) and fully process only the
    // last. The dropped frames were never going to be seen. We still advance the
    // frame-time tracker per skipped frame so the framerate health check sees
    // the true capture rate, and the first-frame milestone still fires on the
    // first frame we actually process.
    if (sliceBytes > 0) {
        const qint64 whole = m_buf.size() / sliceBytes;
        if (whole > 1) {
            const qint64 skip = whole - 1;
            m_buf.remove(0, skip * sliceBytes);   // discard stale frames cheaply
            // V32.38h FIX: the skipped frames DID arrive from the camera, so we
            // still want the framerate-health check to see them — but they must
            // be SPACED across the interval since the last recorded frame, not
            // all stamped "now". Stamping N frames at the same instant injects
            // N-1 zero-interval deltas, which drag the MEDIAN inter-frame time
            // toward 0 (apparent ∞ fps) and mask a genuine slowdown. Distribute
            // them evenly between the last sample and now so the recorded cadence
            // reflects the true average production rate.
            const qint64 now  = QDateTime::currentMSecsSinceEpoch();
            const qint64 last = m_frameTimes.isEmpty() ? now : m_frameTimes.last();
            const qint64 span = now - last;
            // Only distribute when we actually have a positive interval to spread
            // across. If span<=0 (first-ever batch, or clock not advanced) we have
            // no cadence information — recording invented timestamps would just be
            // zero-deltas again, so skip the skipped frames for the health metric
            // entirely (the processed frame below still records normally).
            if (span > 0) {
                const qint64 steps = skip + 1;
                for (qint64 i = 1; i <= skip; ++i) {
                    recordFrameTime(last + (span * i) / steps);
                }
            }
            if (!m_coalesceLogged && m_policy.logFirstFrameInfo) {
                m_coalesceLogged = true;
                qCDebug(lcGstCam) << "Frame coalescing active — skipped" << skip
                                  << "stale frame(s) to catch up to live preview"
                                  << "(GUI fell behind the camera rate)";
            }
        }
    }

    while (m_buf.size() >= sliceBytes) {
        const QByteArray frame = m_buf.left(sliceBytes);
        m_buf.remove(0, sliceBytes);

        // V32.34+ TILING FIX: use the NEGOTIATED stride/dims when known.
        // m_sinkStride/m_sinkWidth/m_sinkHeight are set once the fdsink caps
        // line is parsed; before that we fall back to the requested dims
        // (best guess for the very first frames). Passing the real
        // bytesPerLine is what stops the diagonal-walk / tiled rendering.
        // Use the SAME stride the slice was cut at (rowStride), and the sink
        // dims, so bytesPerLine matches the actual buffer layout exactly.
        const int imgStride = rowStride;
        // V32.37+ SAFETY: imgW (from m_sinkWidth) and imgStride (from the
        // inferred/forced/sink stride) are chosen from INDEPENDENT sources. If
        // they disagree such that imgW*4 > imgStride, QImage reads imgW*4 bytes
        // per row out of a buffer that only has imgStride bytes per row — an
        // out-of-bounds read AND the diagonal-tiling artefact. Clamp imgW to what
        // the stride can actually hold so we never over-read; log once if it ever
        // fires (means sink width and stride are inconsistent and needs a look).
        int imgW = (m_sinkWidth  > 0) ? m_sinkWidth  : m_frameWidth;
        if (imgW * 4 > imgStride) {
            const int safeW = imgStride / 4;
            if (!m_strideWidthMismatchLogged) {
                m_strideWidthMismatchLogged = true;
                qCWarning(lcGstCam) << "Frame width" << imgW << "needs" << (imgW * 4)
                                    << "bytes/row but stride is only" << imgStride
                                    << "— clamping width to" << safeW
                                    << "to avoid an out-of-bounds read (sink width/"
                                       "stride inconsistent).";
            }
            imgW = safeW;
        }
        const int imgH      = frameH;
        QImage img(reinterpret_cast<const uchar *>(frame.constData()),
                   imgW, imgH,
                   imgStride,
                   QImage::Format_RGB32);
        // V32.34+: R/B swap fallback. If sink-caps parsing detected RGBx (not
        // our negotiated BGRx) at the fdsink pad, the upstream byte order has R
        // and B reversed and we correct it here. Also exposed via QSettings
        // camera/policy/swapRB so it can be pinned without rebuilding.
        //
        // V32.39+: detach from the pipe buffer FIRST (one copy we need anyway to
        // retain the frame), then swap R/B IN PLACE via the shared SIMD path
        // (NEON on ARM, SSSE3/AVX2 on x86, scalar elsewhere). This drops the
        // extra full-frame allocation QImage::rgbSwapped() did every swapped
        // frame, and the swap itself is vectorized.
        QImage owned = img.copy();
        if (m_swapRedBlueOnReceive) {
            PixelOps::swapRB8888(owned.bits(), imgW, imgH, owned.bytesPerLine());
        }
        m_lastFrame = owned;
        ++m_sessionFrameCount;   // V32.38p: real per-session delivery count

        // V32.34+: first-frame milestone. Triggers the firstFrameReceived
        // signal (consumed by CameraTester), suppresses the watchdog,
        // and optionally logs a one-time confirmation that the node is
        // delivering frames at the expected dimensions. Subsequent
        // frames go straight to framePreview without any per-frame
        // bookkeeping cost — the branch predictor will hoist this
        // out of the hot loop once m_firstFrameSeen is true.
        if (!m_firstFrameSeen) {
            m_firstFrameSeen = true;
            if (m_policy.logFirstFrameInfo) {
                qCInfo(lcGstCam) << "First frame from" << m_currentNode
                                 << "size=" << m_frameWidth << "x" << m_frameHeight
                                 << "bytes-per-frame=" << m_frameBytes;
            }
            // V32.34+: switch the framedrop watchdog from the
            // first-frame-timeout window to the shorter post-startup
            // stall-detection window. Subsequent onStdoutData calls
            // will rearm it with this new interval.
            if (m_framedropTimer && !m_probeMode) {
                m_framedropTimer->setInterval(m_policy.framedropTimeoutMs);
                m_framedropTimer->start();
            }
            emit firstFrameReceived();
            // Probe mode: stop right after the first frame so the node
            // is freed for the real session. CameraTester reads
            // lastFrame() via signal/slot in its onProbe handler.
            if (m_probeMode) {
                // Defer the stop so the queued framePreview signal
                // (emitted below this block) lands before we tear
                // down. stop() is safe to call from this slot but
                // we don't want to invalidate `frame` mid-iteration.
                QTimer::singleShot(0, this, [this]() { stop(); });
            }
        }

        // Don't emit framePreview in probe mode — there's no UI consumer,
        // and the auto-stop fires immediately after this. lastFrame()
        // still returns the captured image for the probe to inspect.
        if (!m_probeMode) {
            emit framePreview(owned);
            // V32.34+: track frame arrival time for the framerate
            // health check. Cheap (couple of QList ops every frame;
            // sort runs once per 30 frames). Probe mode skips this —
            // probe mode only captures one frame anyway.
            recordFrameTime(QDateTime::currentMSecsSinceEpoch());
        }
    }

    // Cap buffer so it doesn't grow unbounded if rendering falls behind.
    // V32.38h FIX: must use the ACTUAL frame size (sliceBytes = padded rowStride
    // × height), NOT the tight m_frameBytes. With a padded stride m_frameBytes is
    // smaller than a real frame, so the old `right(m_frameBytes*2)` both kept too
    // little AND cut at an offset that isn't a frame boundary — re-introducing the
    // diagonal-tiling misalignment the stride logic prevents. Keep whole frames
    // and trim on a frame boundary.
    if (sliceBytes > 0 && m_buf.size() > sliceBytes * 4) {
        const qint64 keepFrames = 2;
        const qint64 keepBytes  = sliceBytes * keepFrames;
        // right(keepBytes) might start mid-frame; re-align to a frame boundary so
        // the next slice begins cleanly.
        m_buf = m_buf.right(keepBytes);
        const qint64 rem = m_buf.size() % sliceBytes;
        if (rem > 0) m_buf.remove(0, rem);
    }
}

void GstLaunchBackend::requestCapture()
{
    if (m_lastFrame.isNull()) {
        emit error(tr("No frame available yet"));
        return;
    }
    emit frameCaptured(m_lastFrame);
}

// ─── V32.34+: defensive quality management ──────────────────────────────────

void GstLaunchBackend::checkQualityAndMaybeRestart()
{
    // Only act if the user asked for a specific target (sourceTargetWidth
    // > 0). Without a target, we don't have a yardstick to measure
    // negotiated quality against, so logging "got 320×240" with no
    // context would be noise.
    if (m_policy.sourceTargetWidth <= 0 || m_policy.sourceTargetHeight <= 0) {
        return;
    }
    // Compute ratio of negotiated to target for each axis; pick the
    // worse of the two as the "quality score". Below lowQualityRatio
    // means we got materially less than asked for.
    const qreal wRatio = static_cast<qreal>(m_negotiatedSourceWidth)
                       / static_cast<qreal>(m_policy.sourceTargetWidth);
    const qreal hRatio = static_cast<qreal>(m_negotiatedSourceHeight)
                       / static_cast<qreal>(m_policy.sourceTargetHeight);
    const qreal score  = qMin(wRatio, hRatio);

    if (score >= m_policy.lowQualityRatio) {
        // Acceptable quality — no action.
        qCDebug(lcGstCam) << "Source quality OK — ratio:" << score
                          << "negotiated:" << m_negotiatedSourceWidth
                          << "x" << m_negotiatedSourceHeight;
        return;
    }

    // Below threshold. Log a clear warning regardless of restart
    // policy — the user / future-debugger needs to know.
    qCWarning(lcGstCam) << "Source negotiated LOW QUALITY:"
                        << m_negotiatedSourceWidth << "x" << m_negotiatedSourceHeight
                        << "(asked for"
                        << m_policy.sourceTargetWidth << "x" << m_policy.sourceTargetHeight
                        << ", ratio" << score << "< threshold"
                        << m_policy.lowQualityRatio << ")";

    // Decide based on autoQualityRestart policy.
    const QString mode = m_policy.autoQualityRestart;
    if (mode == QLatin1String("off")) {
        return;
    }
    if (mode == QLatin1String("first") && m_qualityRestartUsed) {
        // Already tried once in this session — accept the result.
        qCInfo(lcGstCam) << "Quality restart already attempted this session, accepting";
        return;
    }

    // We're going to restart. Capture what we need first; we'll call
    // start() with the same node from a 0ms QTimer (NOT directly from
    // the stderr handler — Qt's QProcess doesn't like being
    // restarted from inside its own signal dispatch).
    m_qualityRestartUsed = true;
    const QString nodeToRestart = m_currentNode;
    qCInfo(lcGstCam) << "Auto-restarting for quality on" << nodeToRestart;
    m_isRecoveryRestart = true;  // our own relaunch, not a fresh session
    QTimer::singleShot(0, this, [this, nodeToRestart]() {
        // Defensive: a stop+start cycle can race against the user
        // pulling out of the camera page. If we're no longer active
        // by the time this fires, skip — the user moved on.
        if (!m_active) return;
        stop();
        // V32.34+: same cooldown-bypass as the early-exit retry path.
        // Without zeroing m_lastStartMs, the start() below would be
        // dropped because it follows the previous accepted start()
        // by less than kSwitchCooldownMs. The quality restart is a
        // deliberate, deduplicated action — the cooldown's purpose
        // (suppress rapid-fire external start calls) doesn't apply.
        m_lastStartMs = 0;
        start(nodeToRestart);
    });
}

void GstLaunchBackend::recordFrameTime(qint64 nowMs)
{
    m_frameTimes.append(nowMs);
    // Keep last 30 samples.
    while (m_frameTimes.size() > 30) {
        m_frameTimes.removeFirst();
    }
    if (++m_framesSinceHealthCheck < kFrameHealthCheckInterval) return;
    m_framesSinceHealthCheck = 0;

    // Need at least 2 samples to compute an interval.
    if (m_frameTimes.size() < 2) return;

    // Median / min / max inter-frame interval — median is robust to an
    // occasional pause; min and max bracket the jitter.
    QList<qint64> deltas;
    deltas.reserve(m_frameTimes.size() - 1);
    for (int i = 1; i < m_frameTimes.size(); ++i) {
        deltas.append(m_frameTimes[i] - m_frameTimes[i - 1]);
    }
    std::sort(deltas.begin(), deltas.end());
    const qint64 medianMs = deltas[deltas.size() / 2];
    const qint64 minMs    = deltas.first();
    const qint64 maxMs    = deltas.last();
    const qreal  effFps   = medianMs > 0 ? 1000.0 / static_cast<qreal>(medianMs) : 0.0;

    // V32.39+ PERF DIAGNOSTIC: unconditional cadence log (does NOT require a
    // sourceTargetFps to be set — that gate is why the FP5 saw nothing before).
    // This single line distinguishes THROUGHPUT lag (low fps, large median)
    // from LATENCY/JITTER (median fine but large max → a periodic stall, the
    // gldownload-readback signature), and names the live conversion path + output
    // size so flipping preferGpuConvert or previewMax* can be compared run to run.
    // Gated only by logFirstFrameInfo so it can be silenced.
    if (m_policy.logFirstFrameInfo) {
        qCInfo(lcGstCam) << "Preview cadence: ~" << qRound(effFps) << "fps"
                         << "(median" << medianMs << "ms, min" << minMs
                         << "ms, max" << maxMs << "ms) at"
                         << m_frameWidth << "x" << m_frameHeight
                         << (m_usingGpuPath ? "[GPU convert]" : "[CPU convert]")
                         << "— delivered" << m_sessionFrameCount << "frames";
    }

    // Existing one-shot degraded WARNING — only meaningful when a target fps is
    // configured to compare against.
    if (m_frameRateDegradedLogged) return;
    if (m_policy.sourceTargetFps <= 0) return;
    const qreal targetMs = 1000.0 / static_cast<qreal>(m_policy.sourceTargetFps);
    if (static_cast<qreal>(medianMs)
        > targetMs * m_policy.frameRateDegradedRatio) {
        m_frameRateDegradedLogged = true;
        qCWarning(lcGstCam) << "Frame rate degraded — median interval"
                            << medianMs << "ms (~" << effFps << "fps),"
                            << "target" << m_policy.sourceTargetFps << "fps."
                            << "If this persists, sensor may be in low-power"
                            << "mode or CPU is overloaded.";
    }
}

} // namespace SnapClone::UI::Camera
