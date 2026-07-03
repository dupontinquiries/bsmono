#pragma once

// ─── UI/CameraWidget.hpp ──────────────────────────────────────────────────────
//
// Camera viewfinder + snap capture.
//
// Architecture:
//   CameraWidget is a pure UI shell. All actual capture happens through a
//   CameraBackend interface (see ui/camera/CameraBackend.hpp). Two backends:
//     • QtCameraBackend     — desktop / Qt's QMediaDevices-visible cameras
//     • GstLaunchBackend    — postmarketOS / PipeWire virtual cams
//   CameraWidget picks the right one at startup based on isPostmarketOS()
//   and falls back if the preferred backend reports no devices.
//
//   This means the widget itself contains no QCamera/QProcess plumbing —
//   adding new backends or features (video recording, focus controls) is
//   done in the backend, not by adding more if-branches here.
//
// External API (used by MainWindow):
//   startCamera() / stopCamera()  — lifecycle (idempotent, safe to call repeatedly)
//   setRecipient(name)             — updates "To: alice" indicator
//   setScanMode(active)            — emits scanFrameAvailable at ~8fps for QR
//
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QImage>
#include <QPointF>    // V32.34+: m_lastFocusTapPoint
#include <QHash>      // V32.34+: m_landmarkHintCooldown
#include <QMutex>     // V32.32: guards m_latestFrame across worker-thread CDAF
#include <atomic>     // V32.32: m_focusInProgress
#include <memory>

QT_BEGIN_NAMESPACE
class QPushButton;
class QComboBox;
class QLabel;
class QTimer;
QT_END_NAMESPACE

namespace SnapClone::UI {

namespace Camera { class CameraBackend; struct DeviceInfo; class VcmController; class FocusReticle; class GlassScrim; }
namespace Video  { class ShutterButton; class VideoRecorder; class VideoTranscoder; }

} // namespace SnapClone::UI

// V32.32: forward decl for the AppController setController() pattern.
namespace SnapClone::Core { class AppController; }

namespace SnapClone::UI {

class CameraWidget : public QWidget
{
    Q_OBJECT

public:
    explicit CameraWidget(QWidget *parent = nullptr);
    ~CameraWidget() override;

    void startCamera();
    void stopCamera();
    /// Update the "To: <name>" indicator shown in the top bar.
    void setRecipient(const QString &displayName);

    /// V32.37+ fullscreen redesign: toggle full-bleed/ambient mode. When true,
    /// the viewfinder fills the entire widget and the control bars float over it
    /// as translucent overlays (camera-roll / ambient preview look). When false,
    /// the classic stacked layout (topBar / viewfinder / bottomBar). Safe to call
    /// repeatedly; no-op if already in the requested mode.
    void setFullBleed(bool on);
    [[nodiscard]] bool isFullBleed() const { return m_fullBleed; }
    void setScanMode(bool active); ///< throttled frame emission for QR scanner

    /// V32.32: attach the AppController so capture path can reach the
    /// process-wide face library + detector for opportunistic selfie
    /// face-crop archiving (Phase 1 of the face-bandwidth project).
    /// Non-owning. Optional — when null, captures behave as before
    /// (no face library interaction).
    ///
    /// V32.34+ battery: now also subscribes to the controller's
    /// PowerManager screenActiveChanged signal so the camera pipeline
    /// can be fully stopped when the device screen turns off. Out-of-line
    /// because the connect needs the full PowerManager type.
    void setController(SnapClone::Core::AppController *c);

private slots:
    /// V32.34+ battery: respond to device screen on/off. On OFF, stop
    /// the backend pipeline + bridge (sensor power-down). On ON, restart
    /// if WE were the one who stopped it and the page is still showing.
    void onScreenActiveChanged(bool active);

protected:
    void showEvent(QShowEvent *e) override;
    void hideEvent(QHideEvent *e) override;
    /// V32.32: tap-to-focus. Filters MouseButtonPress events on the
    /// viewfinder label and triggers a fast CDAF sweep at the tap.
    /// Returns false (don't consume) so other consumers still see the
    /// event if they ever need to.
    bool eventFilter(QObject *obj, QEvent *ev) override;

private slots:
    /// V32.32: kick off a fast CDAF sweep on the cached preview frames.
    /// Idempotent against re-entry — second tap during a sweep is a
    /// no-op. Connected from the viewfinder tap event filter.
    void runFastFocusSweep();

    /// V32.35+: unified tap entry for BOTH mouse and touch. Implements the
    /// single-tap-focus / double-tap-flip discrimination. `point` is in
    /// m_viewfinderLabel local coordinates.
    void handleViewfinderTap(const QPointF &point);
    /// Run a focus sweep at `point` WITHOUT the double-tap-to-flip arming. Used
    /// for taps on the chrome strip (bar gaps / non-interactive bar labels),
    /// where a double-tap should not flip the camera.
    void triggerFocusOnly(const QPointF &point);
    /// V32.35+: flip between the current facing and the opposite (front<->rear)
    /// by driving m_cameraSelector to the next device of the other facing —
    /// the SAME path the menu uses, so no parallel switch logic.
    void flipCameraFacing();

    /// V32.32: periodic sharpness sampler. Triggers a re-focus when the
    /// scene's sharpness drops substantially below the post-focus
    /// baseline, indicating the subject likely moved. Runs at 1.5s
    /// intervals after the first successful focus. Skipped during
    /// recording and when focus is locked.
    void onSceneChangeTick();

signals:
    /// Emitted at ~8fps when QR scan mode is active.
    void scanFrameAvailable(const QImage &frame);
    void snapCaptured(QImage image, int durationSecs);
    /// Emitted after a video is recorded + transcoded successfully.
    /// `webmPath` is the file path on disk; `durationMs` is recording length.
    /// MainWindow handler reads the file and routes through sendSnap with
    /// MediaType::Video. The file lives in temp until the snap is encoded
    /// for transport (then it can be deleted).
    void videoSnapCaptured(QString webmPath, int durationMs);
    void backRequested();
    /// User asked to start a voice chat — via the shutter swipe-down or the
    /// bottom-right mode button. The host opens the multi-select people picker.
    void voiceChatRequested();

    /// V32.30: re-emits the backend's preview frames for any consumer
    /// that wants live frames (e.g. AmbientCameraDriver). Fires at the
    /// backend's native rate (~30fps). Consumers should throttle —
    /// don't subscribe if you can't handle 30Hz.
    void previewFrameAvailable(const QImage &frame);

private slots:
    void onCaptureClicked();          ///< Photo (quick tap on shutter)
    void onRecordStarted();           ///< Hold detected → begin recording
    void onRecordLocked();            ///< Swipe-up after hold → hands-free
    void onRecordStopped(int durMs);  ///< Release/tap → stop + transcode
    void onTranscodeFinished(const QString &outputPath);
    void onTranscodeFailed(const QString &reason);
    void onRecorderTick(int elapsedMs);
    void onTextOnlyClicked();
    void onSelectorChanged(int index);
    void onRotateClicked();   ///< Cycle user rotation 0→90→180→270→0

    // Backend signal handlers
    void onPreviewFrame(const QImage &frame);
    void onFrameCaptured(const QImage &frame);
    void onBackendError(const QString &msg);
    void onBackendDeviceListChanged();

    /// V32.33: PIP backend's preview frame handler. Scales the frame into
    /// the bottom-right overlay. Mirrors if the PIP camera is front-facing
    /// (selfie convention — your motions match the overlay).
    void onPipFramePreview(const QImage &frame);

private:
    void buildUi();
    void selectBackend();
    void refreshDeviceSelector();
    /// V32.35+: resume capture on the SELECTED device (not the default).
    /// Returns the backend start() result. See impl for the bug rationale.
    bool resumeSelectedCamera();
    void showStartingPlaceholder();
    void showNoCameraPlaceholder();

    /// V32.33 / V32.38+: create m_pipBackend (matching m_backend's class),
    /// start it on `pipDeviceId`, and show m_pipOverlayLabel. `mirror` controls
    /// whether the overlay is horizontally flipped — true for a user-facing /
    /// selfie PIP, false for an external scene camera. No-op if the PIP backend
    /// is already running. Called from the PIP dropdown branches in
    /// onSelectorChanged ("Rear + Selfie PIP" and "Camera + USB PIP").
    void startPip(const QString &pipDeviceId, bool mirror);
    /// V32.33: stop and destroy m_pipBackend, hide m_pipOverlayLabel.
    /// Idempotent. Called when switching away from PIP mode, on
    /// stopCamera(), and on widget destruction.
    void stopPip();
    /// V32.33: reposition m_pipOverlayLabel into the bottom-right corner
    /// of m_viewfinderLabel with kPipOverlayMargin spacing. Called from
    /// resize events and after startPip() shows the overlay.
    void repositionPipOverlay();
    /// V32.37+ position the floating top/bottom bars over the viewfinder when in
    /// full-bleed mode. Called from the viewfinder resize handler.
    void layoutFloatingBars();
    /// Position top-anchored overlays (REC indicator) clear of the floating top
    /// bar in full-bleed; restore to the top inset in classic mode. Called from
    /// layoutFloatingBars and at each REC-indicator show site.
    void positionTopOverlays();
    /// Lazily create the full-bleed frosted-glass scrim layers (mouse-transparent,
    /// parented to the viewfinder, sitting behind the interactive bars).
    Camera::GlassScrim *ensureTopScrim();
    Camera::GlassScrim *ensureBottomScrim();
    /// Feed each visible scrim the strip of the current viewfinder pixmap under
    /// it so the frosted glass tracks the live scene. Called per preview frame.
    /// Scale + (full-bleed) centre-crop + dpr-tag `displayImg` (already rotated
    /// and mirrored as needed) and blit it to the viewfinder. Shared by the live
    /// frame path and the resize handler (which re-renders the cached frame so
    /// the preview tracks size changes even while paused/stalled).
    void renderToViewfinder(const QImage &displayImg, bool live = false);
    // Compose a full-viewfinder image: the aspect-fit frame centred over a
    // blurred, expanded "bleed" of itself, so a frame whose aspect ratio
    // doesn't match the viewfinder fills the dead space with a soft glow
    // instead of hard bars. Used for the non-full-bleed (fit) path.
    QImage makeGlowBleed(const QImage &src, const QImage &fit, const QSize &dstSize) const;
    void updateScrimBackdrops(const QImage &viewfinderSized);
    /// V32.33: returns true iff the backend reports BOTH a Facing::Rear
    /// and a Facing::Front device. Used by refreshDeviceSelector to
    /// decide whether to append the special PIP dropdown entry.
    [[nodiscard]] bool backendHasFrontAndRear() const;
    /// V32.33: returns the first Facing::Rear device id from the current
    /// backend's available devices (empty if none).
    [[nodiscard]] QString firstRearDeviceId() const;
    /// V32.33: returns the first Facing::Front device id (empty if none).
    [[nodiscard]] QString firstFrontDeviceId() const;
    /// V32.38+: first "external" camera id — a device the registry does NOT
    /// classify as an internal sensor and whose name isn't a known user-facing
    /// webcam (i.e. a plugged-in USB camera). Empty if none. Used as the PIP
    /// source for the "Camera + USB PIP" mode.
    [[nodiscard]] QString firstExternalDeviceId() const;
    /// V32.38+: first "internal" / user-facing camera id — registry Front/Rear,
    /// or a name matching a built-in webcam (FaceTime/iSight/Integrated/HD
    /// Webcam). Empty if none. Used as the MAIN source for "Camera + USB PIP".
    [[nodiscard]] QString firstInternalDeviceId() const;
    /// V32.38+: true iff the backend reports BOTH an internal/user-facing
    /// camera and an external USB camera (and simultaneous capture is allowed).
    /// Gates the "Camera + USB PIP" dropdown entry.
    [[nodiscard]] bool backendCanInternalPlusExternalPip() const;
    /// V32.38+: classify a device (registry facing, else built-in-webcam name)
    /// as internal/user-facing vs external USB. Shared by the three helpers
    /// above. Static — consults only the global registry, no instance state.
    [[nodiscard]] static bool isInternalCamera(const QString &id,
                                               const QString &displayName);

    /// V32.32: opportunistic selfie face crop archive. Called from
    /// onFrameCaptured when the front camera is active and an
    /// AppController is attached. Dispatches detection + crop + encode
    /// + library insert onto a worker thread; never blocks. Soft-fails
    /// on any sub-step (no face, library disabled, encode error) — no
    /// user-facing indicator either way. See implementation for full
    /// rationale.
    void archiveSelfieFaceIfAny(const QImage &frame);
    [[nodiscard]] bool shouldProcessFrames() const;
    void updateProcessingState();
    /// Apply m_userRotation to a frame. Returns the rotated copy.
    /// Returns the original (no copy) if rotation is 0.
    [[nodiscard]] QImage applyUserRotation(const QImage &src) const;

    /// Start polling the recorder's preview file (~20 Hz) and painting
    /// each new JPEG frame to the viewfinder. Safe to call when no
    /// recorder is active — it'll find no file and quietly do nothing.
    void startPreviewPolling();
    /// Stop the preview poll timer and revert the viewfinder to the
    /// "RECORDING" placeholder (in case the camera backend hasn't
    /// resumed yet). Called from recordingStopped / recordingFailed.
    void stopPreviewPolling();

    // The active backend (QtCameraBackend or GstLaunchBackend).
    // Replaceable at runtime for fallback.
    std::unique_ptr<Camera::CameraBackend> m_backend;

    // ─── V32.33: Picture-in-Picture (basic, v1) ──────────────────────────────
    //
    // When the user picks the special "Rear + Selfie PIP" dropdown entry,
    // we run a SECOND camera backend concurrently. The main backend drives
    // the full viewfinder (typically the rear camera); the PIP backend
    // drives a small overlay in the bottom-right corner (typically the
    // front camera).
    //
    // Limitations (v1, deliberate):
    //   - Capture path is unchanged: the SAVED snap is the main camera only.
    //     PIP is preview-only. Capturing both as separate streams is queued
    //     for the protocol-extension turn where the wire format learns to
    //     carry composite/multi-asset snaps. See PIP design discussion.
    //   - Tap on the PIP overlay does nothing (no swap, no hide).
    //   - PIP position is fixed bottom-right; drag-to-reposition is queued.
    //   - The "Rear + Selfie PIP" dropdown entry only appears when the
    //     backend reports both a rear-facing and a front-facing device.
    //     On phones where only the gst-launch backend is used and
    //     availableDevices() returns empty, the entry simply won't show.
    //
    // Lifecycle: m_pipBackend is non-null IFF PIP mode is active. It is
    // created on selector change (when itemData == kPipSentinel) and torn
    // down when switching away or on stopCamera(). It uses the same backend
    // CLASS as m_backend (so both QtCameraBackend or both GstLaunchBackend);
    // mixing classes isn't supported because the backends typically own
    // process-wide resources (Qt's QMediaDevices, gst-launch instances).
    std::unique_ptr<Camera::CameraBackend> m_pipBackend;
    QLabel  *m_pipOverlayLabel = nullptr;
    /// V32.38+: the device id the PIP backend is currently running (generalised
    /// from the old hardwired front camera). Used by the retry path + teardown.
    QString m_pipDeviceId;
    /// V32.38+: whether to horizontally mirror the PIP overlay. True for a
    /// user-facing PIP (selfie / front), false for an external scene camera.
    bool    m_pipMirror = false;
    /// Sentinel `itemData()` value for the PIP dropdown entry. Distinguishes
    /// the special PIP mode from real device IDs (which are PipeWire node
    /// names or Qt device byte arrays — never start with "__").
    static constexpr const char *kPipSentinel = "__pip__";
    /// V32.38+: sentinel for the second PIP mode — built-in/internal webcam as
    /// MAIN, external USB camera as the PIP overlay. Distinct from kPipSentinel
    /// (rear+front) so onSelectorChanged dispatches the right camera pairing.
    /// Like kPipSentinel it's a non-device value (real ids never start "__").
    /// Shown only when both an internal and an external camera are present
    /// (see backendCanInternalPlusExternalPip()).
    static constexpr const char *kPipExtSentinel = "__pip_ext__";
    /// V32.34+: PIP retry-once gate. The first start() can race with
    /// camera-bridge startup and return false; we schedule a 1.5s
    /// retry that flips this flag. If retry also fails, we give up
    /// for the session. Reset on successful start.
    bool    m_pipRetried = false;
    /// Bottom-right PIP overlay dimensions (logical px). Chosen to be
    /// recognisable but not dominant; roughly matches BeReal's PIP size.
    static constexpr int kPipOverlayW = 120;
    static constexpr int kPipOverlayH = 160;
    static constexpr int kPipOverlayMargin = 16;

    // UI elements
    QLabel       *m_viewfinderLabel  = nullptr;
    QLabel       *m_recipientLabel   = nullptr;
    QLabel       *m_recipientBanner  = nullptr;
    // V32.37+ fullscreen redesign: the top/bottom control bars are wrapped in
    // container widgets so they can EITHER sit in the QVBoxLayout (classic
    // stacked layout) OR float as overlay children of the viewfinder
    // (full-bleed/ambient mode — viewfinder fills the whole widget, bars hover
    // over it with a translucent scrim). setFullBleed() toggles between the two;
    // layoutFloatingBars() positions them when floating.
    QWidget      *m_topBarWidget     = nullptr;
    QWidget      *m_bottomBarWidget  = nullptr;
    // V32.38+ full-bleed gradient scrims. SEPARATE from the bars: the scrim
    // paints a frosted-glass blur of the viewfinder strip behind it with a
    // subtle chromatic shift, fading edge→transparent, and is
    // WA_TransparentForMouseEvents (so empty regions pass taps through to the
    // viewfinder for tap-to-focus) while the bar above stays fully interactive.
    // Created lazily on first full-bleed, parented to m_viewfinderLabel.
    Camera::GlassScrim *m_topScrim    = nullptr;
    Camera::GlassScrim *m_bottomScrim = nullptr;
    bool          m_fullBleed        = false;
    Video::ShutterButton  *m_shutter         = nullptr;
    Video::VideoRecorder  *m_recorder        = nullptr;
    Video::VideoTranscoder *m_transcoder     = nullptr;
    /// V32.32: AppController for opportunistic face-library writes on
    /// capture. Non-owning, may be null on test instances. Capture
    /// path branches on `m_controller == nullptr` to skip face work.
    SnapClone::Core::AppController *m_controller = nullptr;    QLabel                *m_recordIndicator = nullptr;  // red dot + elapsed
    QString                m_recordingPath;               // current .mkv being written
    /// True if we stopped the camera backend so the recorder could open
    /// the v4l2 device exclusively. We restart it on completion.
    bool                   m_viewfinderPausedForRecording = false;
    /// Polls the recorder's tee-branch JPEG preview file at ~20 Hz so the
    /// viewfinder shows live frames while the recorder owns /dev/video0.
    /// See VideoRecorder::previewFilePath() for the file contract.
    QTimer                *m_previewPollTimer = nullptr;
    qint64                 m_previewLastMtime = 0;        // skip re-read if unchanged
    QPushButton  *m_backButton       = nullptr;
    QPushButton  *m_textOnlyButton   = nullptr;
    QComboBox    *m_cameraSelector   = nullptr;

    // Scan throttling for QR mode
    bool m_scanMode      = false;
    int  m_scanThrottle  = 0;

    // V32.32: tap-to-focus state + most-recent-frame cache.
    //
    // THREADING MODEL (revised V32.32):
    //   - onPreviewFrame runs on the GUI thread (frames delivered via
    //     QueuedConnection from the backend).
    //   - The CDAF sweep runs on a WORKER thread via QtConcurrent::run.
    //     Why: the sweep does 5-9 lens moves with 100-150ms settle
    //     delays each (~750ms total for fast, ~3s for full CDAF). On
    //     the GUI thread that blocks input handling for the duration,
    //     causing tap events to buffer up and double-fire when the
    //     thread unblocks.
    //   - m_latestFrame is READ by the worker and WRITTEN by the GUI
    //     thread. Concurrent access of a non-atomic QImage handle is
    //     UB, so we guard with m_frameMutex. The QImage itself is
    //     implicit-shared so copy-under-lock is cheap (refcount bump).
    //   - m_focusInProgress is std::atomic<bool> so the GUI thread's
    //     re-entry check and the worker's set/clear don't race.
    //   - VcmController is owned by CameraWidget; the worker reads its
    //     state while the worker is the only user. We tear it down on
    //     camera switch — that path waits for any in-flight worker.
    //
    // m_latestFrame holds the most recent post-rotation preview frame.
    // Updated in onPreviewFrame after rotation. Worker reads a snapshot
    // under m_frameMutex.
    //
    // m_vcm is the dw9719 controller (rear cam only — front has no AF).
    // Lazily opened after the camera streams for 1.5s so the AFVDD_2P8
    // rail has stabilised.
    //
    // m_focusInProgress prevents overlapping CDAF sweeps. Atomic
    // because it's set on the worker and checked on the GUI thread.
    QImage                                m_latestFrame;
    mutable QMutex                        m_frameMutex;
    std::unique_ptr<Camera::VcmController> m_vcm;
    QTimer                               *m_vcmOpenTimer  = nullptr;
    std::atomic<bool>                     m_focusInProgress { false };
    /// V32.35+: set true to abort an in-flight focus sweep (double-tap flip).
    /// Read by the FocusSweep worker via the isCancelled predicate before each
    /// lens move; cleared when a new sweep is armed. Atomic: worker reads it,
    /// GUI thread writes it.
    std::atomic<bool>                     m_focusAbort { false };

    // V32.32: tap registration debounce.
    //
    // Even with the focus sweep off the GUI thread, certain operations
    // (camera bridge spawn, pw-dump query, file I/O on startup) can
    // briefly block the main thread. During a block, touch events
    // buffer in Qt's event queue. When the block ends, all buffered
    // taps deliver back-to-back, and a single physical tap can look
    // like two taps from Qt's perspective. The single-finger lock
    // catches the simultaneous-fingers case but doesn't catch
    // sequentially-delivered-buffered taps.
    //
    // Debounce: track the timestamp of the last accepted focus tap.
    // Reject subsequent taps within kFocusTapDebounceMs. Real fingers
    // can't tap that fast and a buffered ghost tap typically follows
    // within tens of ms, so this is a clean discriminator.
    qint64                                m_lastFocusTapMs = 0;

    // V32.35+: double-tap-to-flip + touch tap-to-focus.
    //
    // A single viewfinder tap = focus; a double tap = flip front/rear. Because
    // we can't know a tap is "single" until the double-tap window passes, a tap
    // arms m_pendingFocusTimer (~kDoubleTapMs). If a second tap lands before it
    // fires, we cancel the pending focus and flip instead. If it expires, we run
    // the focus sweep for the stored point. This adds ~kDoubleTapMs of latency
    // to tap-to-focus — the conventional camera-app tradeoff (most phone cameras
    // delay single-tap focus for exactly this reason).
    //
    // Touch handling: Plasma Mobile may deliver taps as QTouchEvent that Qt does
    // NOT synthesize into the QMouseEvent this filter historically required —
    // which is why mouse-click focused but finger-tap didn't. We now also accept
    // TouchBegin and extract the first touch point, funneling both into one
    // handleViewfinderTap(point) path.
    QTimer  *m_pendingFocusTimer = nullptr;  ///< fires the deferred single-tap focus
    QPointF  m_pendingFocusPoint;            ///< viewfinder-coord point of the armed tap
    qint64   m_firstTapMs = 0;               ///< timestamp of the first tap in a potential double

    // V32.32: scene-change-triggered re-focus.
    //
    // After a successful CDAF sweep, we record the sharpness baseline
    // (m_focusBaseline). A slow timer (~1Hz) samples current sharpness.
    // If it drops below baseline * kReFocusThreshold (e.g. 0.6×), we
    // assume the subject has moved or the user has reframed, and
    // trigger another fast sweep automatically.
    //
    // This is "continuous AF" without continuously moving the lens —
    // we only sample sharpness (no lens motion), and only ACT when
    // sharpness genuinely drops. Visible lens drift is avoided.
    //
    // m_focusLocked: user-controlled override. When true, the timer
    // still samples but never triggers a sweep. Useful for video
    // recording where focus pulls are disruptive.
    double                                m_focusBaseline   = 0.0;
    QTimer                               *m_sceneChangeTimer = nullptr;
    bool                                  m_focusLocked     = false;

    /// V32.34+ AF UI: visual feedback for tap-to-focus. The reticle is
    /// a child of m_viewfinderLabel and gets repositioned to the tap
    /// point on each press. CameraWidget owns the state-transition: we
    /// call triggerAt() in the eventFilter when a tap is accepted, and
    /// setState(Locked|Failed) from the QFutureWatcher::finished slot
    /// of runFastFocusSweep().
    Camera::FocusReticle *m_focusReticle = nullptr;
    /// Last accepted tap point in viewfinder-label coordinates. Stored
    /// so that the focus-result handler can re-show the reticle in the
    /// same place (it's possible the user has tapped again in between,
    /// but the in-flight sweep was for the older tap — and the result
    /// belongs to the older tap visually).
    QPointF m_lastFocusTapPoint;
    /// V32.34+ AF UI: small persistent pill ("AF" / "AF —") in the
    /// viewfinder corner that communicates whether tap-to-focus is
    /// available for the current camera. Updated when the selector
    /// changes, when CameraRegistry resolves a device's supportsFocus,
    /// and after the async VCM open finishes.
    QLabel *m_afBadge = nullptr;
    void updateAfBadge();

    /// V32.32 (V32.34+ wiring): landmark-candidate hint. A subtle pill
    /// "+ Save as landmark" that fades into the bottom of the viewfinder
    /// when the live frame scores above kLandmarkHintThreshold via the
    /// snapclone-saliency `sc_landmark_score` heuristic. Tap = open
    /// AddLandmarkDialog with the frame attached. Dismiss = swipe down
    /// or wait for the auto-hide timer to expire (also adds the current
    /// blurHash to a cooldown table for an hour to avoid pestering the
    /// user about the same scene).
    QLabel *m_landmarkHint = nullptr;
    /// Timer driving the scoring cadence. We score the latest preview
    /// frame every kLandmarkScoreIntervalMs while the camera is active
    /// AND the viewfinder is visible AND the app is in the foreground.
    /// Off during recording (focus on the active shot, not new affordances).
    QTimer *m_landmarkScoreTimer = nullptr;
    /// Guard so the scorer doesn't pile up workers on a slow device.
    /// True from when a QtConcurrent::run is dispatched until its
    /// QFutureWatcher's finished slot lands. New ticks during this
    /// window are dropped.
    bool m_landmarkScoreInFlight = false;
    /// Auto-hide timer for the hint pill. Started when the pill is
    /// shown; on timeout, hide. Reset whenever the score crosses the
    /// threshold again (so a sustained good scene keeps the pill up).
    QTimer *m_landmarkHintAutoHide = nullptr;
    /// Cooldown set keyed by blurHash hex string. Entries expire after
    /// kLandmarkHintCooldownMs. Cheap linear walk on insert; we'd
    /// upgrade to a deque-with-timestamps if this ever held more than
    /// a handful of entries (unlikely — the user dismisses ~one scene
    /// per session at most).
    QHash<QString, qint64> m_landmarkHintCooldown;
    /// Snapshot of the frame that scored above threshold, captured
    /// when we SHOW the hint. Used by the tap handler to hand the
    /// composer dialog the exact frame the user reacted to (rather
    /// than whatever frame happens to be current at tap time).
    QImage m_landmarkHintFrame;
    /// Compute + apply landmark score for the latest preview frame.
    void runLandmarkScoreTick();
    /// User tapped the hint pill → open AddLandmarkDialog with the captured frame.
    void onLandmarkHintTapped();
    static constexpr int    kLandmarkScoreIntervalMs = 1000;
    static constexpr float  kLandmarkHintThreshold   = 0.7f;
    static constexpr int    kLandmarkHintCooldownMs  = 60 * 60 * 1000; // 1 hour
    static constexpr int    kLandmarkHintAutoHideMs  = 4500;

    // User rotation override — cycles 0/90/180/270 via the rotate button.
    // Applied to preview, captured frames, and scan frames so all paths
    // see the same orientation. Persisted to QSettings so the user setting
    // survives between launches.
    int          m_userRotation = 0;          ///< 0, 90, 180, or 270
    QPushButton *m_rotateButton = nullptr;

    // V32.32: front-camera preview mirror. When the selected device is a
    // front-facing camera, we horizontally flip the LIVE preview only
    // (not captured photos or recorded videos that get sent to others).
    // This matches every phone camera UX convention: the user sees their
    // motions naturally in the viewfinder (right hand on right side),
    // but the recipient sees you as another person would — un-mirrored.
    //
    // Set automatically based on device id containing "front" or
    // "tracking" (both are user-facing on the FP5). Could be made into
    // a user toggle later, but the auto-detect handles the common case.
    bool m_mirrorPreview = false;

    // Battery-aware lifecycle tracking
    bool m_widgetVisible       = false;
    bool m_appActive           = true;
    // V32.38r: timestamp (ms since epoch) when the app last went INACTIVE, or 0
    // while active. Used to ride the preview through a brief focus-loss blip
    // (desktop/Plasma flips ApplicationActive for notifications/focus) instead of
    // freezing the viewfinder the instant focus is lost. See shouldProcessFrames.
    qint64 m_inactiveSinceMs   = 0;
    QTimer *m_inactiveGraceTimer = nullptr;   ///< re-checks the gate at grace expiry
    int  m_frameProcessedCount = 0;
    int  m_frameDropCount      = 0;
    bool m_previewFrameValidated = false; ///< V32.34+: log preview frame layout sanity once per session

    /// V32.34+ battery: screen-off state from PowerManager. When the
    /// device screen turns off (logind IdleHint / KDE screensaver), we
    /// fully STOP the backend pipeline + camera-bridge — not just drop
    /// frames. A running GStreamer pipeline keeps the sensor, ISP, and
    /// preview encoder powered, which is one of the largest idle drains
    /// while the camera page happens to be foreground at lock time.
    /// m_stoppedForScreenOff records that WE stopped it (vs the user
    /// navigating away) so we know to auto-restart on screen-on.
    bool m_screenOff             = false;
    bool m_stoppedForScreenOff   = false;
    /// Connect this once setController()/setPowerManager() runs. Held so
    /// we can disconnect cleanly if the controller is replaced.
    QMetaObject::Connection m_screenConn;

    // V32.34+: id we last marked as in-use with CameraHealthService.
    // On stop() / firstFrameReceived() we use this to release the
    // correct id rather than walking availableDevices() (which would
    // wrongly unset other widgets' in-use ids — PIP backend is a
    // separate session against a different device). Empty when no
    // device is marked in-use by this widget.
    QString m_inUseDeviceId;
    /// Same idea for the PIP backend (front camera, when active).
    /// Tracked separately because PIP is a distinct CameraBackend
    /// instance against a different device id; one widget's stop
    /// shouldn't release the other's in-use status.
    QString m_pipInUseDeviceId;

    // V32.26: debounce show/hide → camera start/stop. A fast swipe past
    // the camera page would otherwise tear down + rebuild the backend
    // (~1.5s kernel release wait on some devices) on every transition,
    // causing visible stutters. With a 250ms debounce, a show followed
    // by hide within the window cancels the planned start; same for
    // hide followed by show. Only "real" visibility transitions reach
    // startCamera/stopCamera.
    //
    // The timer is allocated lazily on first showEvent (not in the
    // constructor) because allocating QTimer requires an event loop
    // and CameraWidget can be constructed before the loop is running.
    QTimer *m_lifecycleTimer  = nullptr;
    bool    m_pendingStart    = false;  // a debounced start is queued
    bool    m_pendingStop     = false;  // a debounced stop is queued
};

} // namespace SnapClone::UI
