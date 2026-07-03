#pragma once

// GstLaunchBackend — spawns gst-launch-1.0 reading from PipeWire and
// pipes raw BGRx frames over stdout. Used on postmarketOS / Alpine where
// Qt's QCamera doesn't enumerate libcamera nodes through the bridge.
//
// Discovery: lists virtual-cam-{rear,front,tracking} nodes from camera-bridge,
// plus any other Video/Source PipeWire nodes if they exist.
//
// Frame format: pipeline forces BGRx at CameraPolicy::captureWidth ×
// captureHeight (default 480×640) so onStdoutData can split incoming
// bytes into fixed-size frames without parsing per-frame metadata.
//
// V32.34+: timeouts and limits come from CameraPolicy (see CameraPolicy.hpp).
// Construct with probeMode=true to use the much-shorter probe timeouts —
// CameraTester does this to enumerate node health quickly.

#include "ui/camera/CameraBackend.hpp"
#include "ui/camera/CameraPolicy.hpp"
#include <QElapsedTimer>
#include <QList>

#include <QByteArray>

QT_BEGIN_NAMESPACE
class QProcess;
class QTimer;
QT_END_NAMESPACE

namespace SnapClone::UI::Camera {

class GstLaunchBackend : public CameraBackend
{
    Q_OBJECT
public:
    explicit GstLaunchBackend(QObject *parent = nullptr);

    /// Construct in probe mode — timeouts are taken from the probe* fields
    /// of CameraPolicy instead of the regular ones. Frames are NOT emitted
    /// via framePreview() (the probe consumer pulls via lastFrame() once
    /// the first frame arrives). Probe-mode backends auto-stop after the
    /// first frame to free the node for the real session.
    static GstLaunchBackend *makeProbe(QObject *parent = nullptr);

    ~GstLaunchBackend() override;

    bool start(const QString &deviceId = {}) override;
    void stop() override;
    [[nodiscard]] bool isActive() const override { return m_active; }
    void requestCapture() override;
    [[nodiscard]] QImage lastFrame() const override { return m_lastFrame; }
    [[nodiscard]] QList<DeviceInfo> availableDevices() const override;
    [[nodiscard]] QString defaultDeviceId() const override;

    /// Effective policy for the current (or last) start(). Captured at
    /// start time so live policy edits don't change behavior of an
    /// already-running backend. Public so CameraTester can read
    /// "what timeouts is this backend actually using."
    [[nodiscard]] const CameraPolicy &policySnapshot() const { return m_policy; }
    [[nodiscard]] bool isProbeMode() const { return m_probeMode; }

    // V32.34+ note: firstFrameReceived() is declared in the base class
    // CameraBackend so all backends signal a uniform "ready" event.
    // GstLaunchBackend emits it from inside onStdoutData after the
    // first complete frame is parsed.

private slots:
    void onStdoutData();

private:
    /// V32.34+ GPU accel + tint fix: detect once (cached process-wide)
    /// whether a given gst element is installed. Used to prefer GPU-
    /// accelerated colorspace conversion (glupload ! glcolorconvert !
    /// gldownload, or v4l2convert on devices with a hardware converter)
    /// over the CPU `videoconvert` element. Returns true if `gst-inspect-1.0
    /// <element>` exits 0. Result is memoized in a static map so repeated
    /// start() cycles don't re-spawn gst-inspect.
    static bool gstElementAvailable(const QString &element);

    /// V32.34+: build the colorspace-conversion + scale segment of the
    /// pipeline as a fresh QStringList. Chooses GPU path when the GL
    /// elements are present AND m_policy.preferGpuConvert is set; else
    /// falls back to the deterministic dual-videoconvert CPU path. The
    /// returned list is spliced into the main args between the source
    /// queue and the fdsink. Centralizing this keeps the GPU/CPU
    /// decision in one auditable place rather than scattered inline.
    [[nodiscard]] QStringList buildConvertScaleSegment(const QString &flipMethod) const;

    QProcess  *m_proc        = nullptr;
    QByteArray m_buf;
    int        m_frameWidth  = 1280;  // mirror of m_policy.captureWidth at start
    int        m_frameHeight = 720;
    int        m_frameBytes  = 0;
    QImage     m_lastFrame;
    bool       m_active      = false;
    bool       m_probeMode   = false;
    bool       m_firstFrameSeen = false;
    quint64    m_sessionFrameCount = 0;  ///< V32.38p: REAL total frames this session
                                          ///< (m_frameTimes.size() caps at 30 and
                                          ///< was wrongly logged as "frames-seen").
    QString    m_currentNode;
    /// Snapshot of CameraPolicyStore at start() time. Read by the
    /// stderr handler, watchdog, and early-exit handler. NOT updated
    /// for the lifetime of a session — stop+start cycle picks up
    /// the new policy.
    CameraPolicy m_policy;
    // V32.32: speculative-listing grace period. We list baseline
    // cameras even when pw-dump returns empty for the first kGraceMs
    // milliseconds after construction, so the dropdown isn't blank
    // during bridge startup. After grace expires, only confirmed
    // cameras appear — broken cameras stop showing as available,
    // making the failure user-visible.
    mutable QElapsedTimer m_graceTimer;
    // V32.33+ Phase 1 audit: track time since last start() call so we
    // can detect early gst-launch exit (≤2s = source unavailable or
    // format negotiation failed). On early exit we retry once with a
    // different node before giving up. Reset on each user-initiated
    // start() with an explicit deviceId so manual selection doesn't
    // accidentally fall through to a different camera.
    QElapsedTimer m_startTimer;
    bool          m_retriedAlternateNode = false;
    /// V32.35+: when a negotiation failure happens, the FIRST recovery is to
    /// relaunch the SAME node with relaxed caps (drop the pinned width/height
    /// and let libcamera/pipewiresrc negotiate freely) — NOT to jump to the
    /// other camera (which caused the front/rear mixup: a transient
    /// not-negotiated on the rear silently fell back to the front). Only if the
    /// relaxed same-node retry ALSO fails do we swap nodes. This flag, when
    /// true, makes the next buildPipeline() omit the width/height pins.
    bool          m_relaxCaps = false;
    /// V32.35+: one relaxed same-node retry per session (mirrors
    /// m_retriedAlternateNode) so a node that fails even relaxed doesn't loop.
    bool          m_retriedRelaxed = false;
    /// V32.37+ device-log fix: after settle+relaxed both fail, retry the SAME
    /// node with a growing backoff before swapping cameras. The FP5's pipewire
    /// node can take ~10-15s to settle after a cold start (logs show 3 rapid
    /// attempts fail but the same node works first-try ~14s later). Bounded so
    /// it can't loop forever. Mirrors snapclone-gstpipeline RecoverySm backoff.
    int           m_negotiationBackoffCount = 0;
    static constexpr int kNegotiationBackoffMax   = 4;
    static constexpr int kNegotiationBackoffBaseMs = 1200;
    static constexpr int kNegotiationBackoffCapMs  = 4800;
    /// V32.35+ (device-logs revision): one device-SETTLE retry per session — the
    /// FIRST response to a not-negotiated (-4), which on this device is a
    /// transient device-state failure, not a caps failure. Retries the same node
    /// with NORMAL caps after a settle delay; only if THAT fails do we try
    /// relaxed caps, then swap nodes. Reset on node change like the others.
    bool          m_negotiationSettleRetried = false;
    /// V32.36+: true for the brief window between scheduling one of our own
    /// recovery relaunches (settle / relaxed-caps / GPU-fallback / quality
    /// restart) and that relaunch entering start(). Distinguishes "our own
    /// recovery re-entry" from "a genuinely fresh session", so start() resets
    /// the per-session retry guards only on real new sessions and never reopens
    /// a recovery loop. Set right before each recovery singleShot; consumed
    /// (read then cleared) at the top of start().
    bool          m_isRecoveryRestart = false;
    /// V32.35+: the node we last set m_relaxCaps for, so a re-entrant start()
    /// from our own retry timer keeps the relaxed caps while a FRESH start for
    /// any node clears them.
    QString       m_lastRelaxedNode;
    /// V32.34+: stall-detection watchdog. Rearmed on every successful
    /// onStdoutData() (specifically: whenever bytes show up on stdout,
    /// not necessarily a full frame). If it fires, the source stopped
    /// producing — distinct from "never started" which the first-frame
    /// watchdog handles. We use a single-shot QTimer here instead of
    /// QElapsedTimer + periodic check because gst-launch only signals
    /// us on data arrival; a periodic check would need its own timer
    /// anyway, so we just let the timer ITSELF be the trigger.
    ///
    /// Lifetime: built in start(), reset in stop(). Not built in the
    /// constructor because the policy isn't available there.
    QTimer       *m_framedropTimer = nullptr;
    // V32.34+: buffered stderr from gst-launch. Plain stderr-to-debug-log
    // hid the actual reason for early exits (negotiation failure, format
    // mismatch, "No such element", permissions). We now retain the last
    // ~4KB of stderr so the early-exit handler can surface the real
    // cause in the error() signal, AND match well-known GStreamer error
    // patterns to user-actionable messages. Size taken from policy.
    QByteArray    m_stderrBuf;

    // V32.34+: defensive quality management. The pipewiresrc-side
    // negotiated caps are captured ONCE from stderr (via gst-launch -v
    // output, see the stderr-handler comment). We compare against the
    // sourceTarget* fields of the policy and may restart the pipeline
    // if libcamera latched into a low-quality preview mode.
    //
    // m_sourceCapsParsed gates the parser: once we've extracted the
    // negotiated caps we stop looking, so subsequent negotiations
    // (rare but possible if the pipeline renegotiates mid-stream)
    // don't keep firing the check. m_qualityRestartUsed prevents
    // the "first"-mode restart from looping forever — one auto-restart
    // attempt per session, max.
    bool m_sourceCapsParsed       = false;
    int  m_negotiatedSourceWidth  = 0;
    int  m_negotiatedSourceHeight = 0;
    bool m_qualityRestartUsed     = false;

    /// V32.34+ tint defense: parse the SINK side caps (fdsink pad) one
    /// time per session, to log exactly what format we're getting bytes
    /// in. The pipeline negotiates BGRx — but if videoconvert ever passes
    /// through with mismatched caps (observed on Phone Red), the bytes
    /// reaching our QImage::Format_RGB32 buffer are in a different byte
    /// order and the colors look wrong (R/B swap = red tint, etc).
    /// Logged at info so it always lands in the journal.
    bool m_sinkCapsParsed         = false;

    /// V32.34+ TILING FIX: the ACTUAL width/height/stride of the BGRx bytes
    /// fdsink writes to the pipe, parsed from the negotiated sink caps. The
    /// frame-extraction loop and QImage construction MUST use these, not the
    /// requested m_frameWidth/m_frameHeight — if GStreamer negotiated a
    /// different size (videoscale couldn't hit the exact target, the source
    /// imposed a size, or stride rounding changed the row layout), slicing the
    /// pipe at the wrong byte boundary makes one real frame render as several
    /// shrunken tiled copies (the persistent "tiled framebuffer" symptom).
    /// 0 = not yet parsed → fall back to the requested dims as a best guess
    /// until the caps line arrives on stderr.
    int  m_sinkWidth              = 0;
    int  m_sinkHeight             = 0;
    int  m_sinkStride             = 0;   ///< bytesPerLine; 0 = assume width*4
    int  m_inferredStride         = 0;   ///< V32.34+: real padded row stride inferred from buffer bloat; 0 = use tight/sink
    int  m_forcedStride           = 0;   ///< V32.35+: manual stride override (SNAPCLONE_CAMERA_STRIDE); 0 = none. Highest priority — for on-device tiling diagnosis.
    bool m_swapRBForced           = false; ///< V32.34+: true if swapRB pinned via settings (auto-detect won't override)

    /// V32.34+ tint defense: when set true, m_lastFrame is computed via
    /// QImage::rgbSwapped() instead of using the raw bytes as-is. Set
    /// automatically by the sink-caps parser when the negotiated format
    /// starts with "RGB" rather than "BGR" — we always asked for BGRx
    /// so a sink-side RGB* means an upstream element swapped channels
    /// silently. Also exposed via QSettings camera/policy/swapRB so a
    /// user can force it on while we debug a specific phone's pipeline.
    /// Default false (no swap) because the pipeline target is BGRx.
    bool m_swapRedBlueOnReceive   = false;

    /// V32.34+ GPU fallback: set true when the GL colorspace pipeline fails
    /// to link at start (stderr shows a "could not link gl... " error).
    /// Once set, buildConvertScaleSegment forces the CPU path for the rest
    /// of this backend's life regardless of policy.preferGpuConvert, so a
    /// device whose GL stack can't negotiate the path recovers to a working
    /// camera on the automatic restart instead of failing forever. Reset
    /// only by destroying the backend (a fresh process can re-probe GL).
    /// This is process-lifetime, NOT persisted — the persisted policy stays
    /// whatever the user set; we just refuse to honor GPU after it proved
    /// broken this run.
    bool m_gpuConvertFailed       = false;

    /// V32.39+ PERF: which conversion path is live this session, set by
    /// buildConvertScaleSegment() (const, hence mutable). Read by the periodic
    /// cadence log so the journal shows whether GPU or CPU conversion is in use
    /// alongside the measured fps — letting a flip of preferGpuConvert be
    /// compared empirically.
    mutable bool m_usingGpuPath   = false;

    // V32.34+: frame-interval tracking for the framerate-degraded
    // health check. Each successful frame appends its arrival time
    // to m_frameTimes; we keep the last 30 so the median responds
    // to ~1 second of frames. checkFrameRateHealth runs from
    // onStdoutData every 30 frames.
    QList<qint64> m_frameTimes;
    int           m_framesSinceHealthCheck = 0;
    static constexpr int kFrameHealthCheckInterval = 30;
    bool          m_frameRateDegradedLogged = false;

    /// Compare m_negotiatedSourceWidth/Height against
    /// m_policy.sourceTarget{Width,Height}, and depending on
    /// policy.autoQualityRestart take action: log only ("off"), or
    /// schedule a one-shot restart ("first"/"always") via a 0ms
    /// QTimer (we can't reentrantly call start() from a stderr
    /// handler — Qt complains about restarting QProcess from inside
    /// a signal callback for the same QProcess).
    void checkQualityAndMaybeRestart();

    /// Append a frame-arrival timestamp and, every
    /// kFrameHealthCheckInterval frames, compute median interval and
    /// emit a warning if it's worse than policy.frameRateDegradedRatio
    /// × the target. Logged once per session (m_frameRateDegradedLogged
    /// suppresses repeat warnings — the user only needs to know once).
    void recordFrameTime(qint64 nowMs);

    // V32.34+: camera switch cooldown. User reported "the camera can
    // be red green or purple sometimes and the mode is switching a
    // lot." Some upstream signal (CameraHealthService re-probe, UI
    // race on focus change, repeated user taps on a slow phone) is
    // firing start() rapidly. Each start() reconfigures pipewire +
    // libcamera, and if the underlying sensor doesn't have time to
    // settle into a stable mode between starts, the negotiated format
    // can vary run-to-run — including landing on a different color
    // mode than expected, which the renderer interprets as the
    // wrong colorspace (the red/green/purple flapping).
    //
    // m_lastStartMs gates: if a start() call arrives within
    // kSwitchCooldownMs of the previous one for ANY node (including
    // same-node restarts), it's dropped. The dropped count is logged
    // every time so we can see in the journal how aggressively
    // upstream is hammering us. The first call after startup or a
    // long quiescent period passes through immediately.
    qint64 m_lastStartMs = 0;
    int    m_droppedStarts = 0;
    QString m_lastStartNode;  // for diagnostic log
    static constexpr qint64 kSwitchCooldownMs = 500;

    // V32.34+: stacked-retry guard. When start() is called before
    // camera-bridge has published its nodes, we schedule a 1500ms
    // singleShot timer that re-emits deviceListChanged so the UI
    // retries. WITHOUT this guard, each repeated start() call
    // (which CAN happen if the UI fires on deviceListChanged AND
    // on becoming-visible AND on whatever else) STACKS a new 1500ms
    // timer. After a few seconds we'd have many timers firing in
    // succession, each emitting deviceListChanged, each calling
    // start() — burning CPU and spamming the log.
    //
    // m_bridgeWaitScheduled flips to true when we set the timer
    // and back to false in the timer's lambda. Subsequent failing
    // start() calls see the flag set and skip scheduling a duplicate.
    bool m_bridgeWaitScheduled = false;
    static constexpr int kMaxStartRetries = 5;
    int  m_consecutiveStartRetries = 0;

    /// V32.34+: one-shot log gate for the buffer-bloat warning. The
    /// check happens every onStdoutData() call (potentially many times
    /// per second when the bloat is happening); without this flag we'd
    /// spam the journal. Reset in start() so subsequent sessions can
    /// re-warn if the issue recurs.
    bool m_bufBloatLogged = false;
    /// V32.37+ one-shot guard for the sink-width vs stride mismatch warning.
    bool m_strideWidthMismatchLogged = false;
    /// V32.37+ one-shot log guard for frame coalescing (stale-frame skip).
    bool m_coalesceLogged = false;
};

} // namespace SnapClone::UI::Camera
