#include "ui/CameraWidget.hpp"
#include "ui/SnapComposerDialog.hpp"
#include "ui/AddLandmarkDialog.hpp"      // V32.34+: viewfinder landmark hint
#include "ui/Theme.hpp"
#include "ui/camera/CameraBackend.hpp"
#include "ui/camera/QtCameraBackend.hpp"
#include "ui/camera/GstLaunchBackend.hpp"
#ifdef SNAPCLONE_LIBCAMERA
#include "ui/camera/LibcameraBackend.hpp"
#endif
#include "ui/camera/VcmController.hpp"   // V32.32: VCM focus control
#include "ui/camera/FocusSweep.hpp"      // V32.32: CDAF sweep + sharpness
#include "ui/camera/FocusReticle.hpp"    // V32.34+: tap-to-focus visual feedback
#include "ui/camera/GlassScrim.hpp"      // V32.38+: frosted-glass full-bleed scrim
#include "ui/camera/GlassIconButton.hpp" // V32.38e: mottled glass icon button + ripple
#include "ui/camera/CameraRegistry.hpp"  // V32.32: typed camera metadata
#include "ui/camera/CameraPolicy.hpp"    // V32.34+: simultaneousCapture gate for PiP
#include "core/LandmarkHash.hpp"         // V32.34+: blurHash for cooldown table
#include "storage/StorageManager.hpp"    // V32.34+: deref for saveLandmark
#ifdef SNAPCLONE_RUST_SALIENCY
#  include "saliency/snapclone_saliency_ffi.h"   // V32.34+: sc_landmark_score
#endif
#include "ui/camera/CameraHealthService.hpp"  // V32.34+: cached probe results
#include "ui/video/ShutterButton.hpp"
#include "ui/video/VideoRecorder.hpp"
#include "ui/video/VideoTranscoder.hpp"
#include <QStandardPaths>
#include <QDateTime>
#include <QDir>
#include <QFile>
#include <QBuffer>             // V32.32: JPEG encode for face library
#include <QCryptographicHash>  // V32.32: SHA-256 for face crop dedup
#include "core/AppController.hpp"   // V32.32: access face library + detector
#include "core/PowerManager.hpp"    // V32.34+ battery: screenActiveChanged
#include "face/FaceLibrary.hpp"     // V32.32: Library + Detector types
#include <cstring>             // V32.32: std::memcpy on the tight RGB buffer

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QMenu>
#include <QPushButton>
#include <QComboBox>
#include <QLabel>
#include <QPixmap>
#include <QPainter>
#include <QMouseEvent>          // V32.34+: tap-to-focus position lookup
#include <QTouchEvent>          // V32.35+: touch tap-to-focus (Plasma Mobile)
#include <QTransform>
#include <QTimer>
#include <QFileInfo>
#include <QAbstractAnimation>
#include <QPropertyAnimation>
#include <QGraphicsOpacityEffect>
#include <QSettings>
#include <QPointer>
#include <QTextStream>
// V32.32: async CDAF sweep — keep blocking lens-settle delays off the
// GUI thread so input handling stays responsive.
#include <QtConcurrent>
#include <QFutureWatcher>
#include <QMutexLocker>
#include <QElapsedTimer>
#include <QEventLoop>
#include <QApplication>
#include <QGuiApplication>
#include <QLoggingCategory>

namespace SnapClone::UI {

Q_LOGGING_CATEGORY(lcCamera, "snapclone.camera")

namespace {
// V32.38r: how long the preview keeps processing after the app loses ACTIVE
// state while still visible + screen-on, before treating it as a real
// background and pausing. Rides through transient desktop/Plasma focus blips
// (notifications, focus changes) without freezing the viewfinder; short enough
// that a genuine background pauses promptly (battery). Mirrors the grace_ms arg
// of snapclone_videosrc::frame_gate_debounced.
constexpr qint64 kInactiveGraceMs = 400;
/// True if this Linux system is postmarketOS / Alpine, where Qt's QCamera
/// usually fails to enumerate libcamera nodes through the bridge.
bool isPostmarketOS()
{
    static int cached = -1;
    if (cached != -1) return cached == 1;
    QFile f(QStringLiteral("/etc/os-release"));
    if (!f.open(QIODevice::ReadOnly | QIODevice::Text)) { cached = 0; return false; }
    const QString text = QTextStream(&f).readAll();
    cached = (text.contains(QLatin1String("postmarketOS"), Qt::CaseInsensitive)
           || text.contains(QLatin1String("ID=postmarketos"), Qt::CaseInsensitive)) ? 1 : 0;
    return cached == 1;
}
} // anon

// ─── Construction ────────────────────────────────────────────────────────────

CameraWidget::CameraWidget(QWidget *parent)
    : QWidget(parent)
{
    buildUi();

    // V32.37+ fullscreen redesign: enable the full-bleed viewfinder (edge-to-edge
    // preview, floating control bars) by default. Gated by a QSettings flag so it
    // can be rolled back on a device without a rebuild: set
    // [ui] fullBleedPreview=false in SnapClone.conf to return to the classic
    // stacked layout. Read AFTER buildUi so the bar containers exist.
    {
        QSettings s;
        const bool fullBleed = s.value(QStringLiteral("ui/fullBleedPreview"), true).toBool();
        if (fullBleed) setFullBleed(true);
    }

    // Track app-level visibility for battery-aware frame processing.
    connect(qApp, &QGuiApplication::applicationStateChanged,
            this, [this](Qt::ApplicationState s) {
        const bool active = (s == Qt::ApplicationActive);
        m_appActive = active;
        if (active) {
            m_inactiveSinceMs = 0;   // clear the blip timer
        } else {
            m_inactiveSinceMs = QDateTime::currentMSecsSinceEpoch();
            // V32.38r: don't freeze the preview instantly on focus loss. We keep
            // processing for a short grace window (kInactiveGraceMs) so a
            // transient ApplicationActive flip (notification, focus change on
            // desktop/Plasma) doesn't stutter the viewfinder. Arm a one-shot to
            // re-evaluate (and log/pause) once the grace expires — frames keep
            // arriving so the gate would re-check anyway, but the timer
            // guarantees the PAUSED transition fires even if delivery hiccups.
            if (!m_inactiveGraceTimer) {
                m_inactiveGraceTimer = new QTimer(this);
                m_inactiveGraceTimer->setSingleShot(true);
                connect(m_inactiveGraceTimer, &QTimer::timeout, this,
                        [this]() { updateProcessingState(); });
            }
            m_inactiveGraceTimer->start(kInactiveGraceMs + 20);
        }
        updateProcessingState();
    });
    m_widgetVisible = isVisible();
    m_appActive     = (qApp->applicationState() == Qt::ApplicationActive);
}

void CameraWidget::setController(SnapClone::Core::AppController *c)
{
    m_controller = c;
    // V32.34+ battery: subscribe to screen on/off. Disconnect any prior
    // connection first (controller could in principle be re-set). The
    // PowerManager may be null on platforms where it failed to construct
    // — guard accordingly; absence just means we never get screen-off
    // events and behave as before (camera stays on with the page).
    if (m_screenConn) {
        QObject::disconnect(m_screenConn);
        m_screenConn = QMetaObject::Connection();
    }
    if (m_controller) {
        if (auto *pm = m_controller->powerManager()) {
            m_screenConn = connect(
                pm, &SnapClone::Core::PowerManager::screenActiveChanged,
                this, &CameraWidget::onScreenActiveChanged);
            // Seed the current state so we don't miss a screen-off that
            // happened before we subscribed.
            m_screenOff = !pm->isScreenActive();
            qCInfo(lcCamera) << "Subscribed to screen-state; screen currently"
                             << (m_screenOff ? "OFF" : "on");
        }
    }
}

void CameraWidget::onScreenActiveChanged(bool active)
{
    const bool screenOff = !active;
    if (screenOff == m_screenOff) return;  // de-dupe
    m_screenOff = screenOff;
    qCInfo(lcCamera) << "Screen" << (active ? "ON" : "OFF")
                     << "— camera" << (active ? "may resume" : "stopping for battery");

    if (screenOff) {
        // Stop the pipeline entirely (sensor + ISP + preview encoder
        // power-down), not just drop frames. Only if we're currently
        // running — and remember that WE stopped it so screen-on can
        // auto-resume without fighting the user's own navigation.
        if (m_backend && m_backend->isActive()) {
            m_stoppedForScreenOff = true;
            qCInfo(lcCamera) << "Stopping backend pipeline for screen-off";
            m_backend->stop();
        }
        // Also stop any PIP backend — it has its own bridge consumer.
        if (m_pipBackend && m_pipBackend->isActive()) {
            m_pipBackend->stop();
        }
        // Stop the landmark scorer + focus scene-tick — both are 1Hz
        // timers that have nothing to do while the screen is dark.
        if (m_landmarkScoreTimer && m_landmarkScoreTimer->isActive())
            m_landmarkScoreTimer->stop();
        if (m_sceneChangeTimer && m_sceneChangeTimer->isActive())
            m_sceneChangeTimer->stop();
        // Reflect the new gate (m_screenOff now true) in the processing-state
        // diagnostic so logs show frame processing PAUSED for screen-off.
        updateProcessingState();
    } else {
        // Screen back on. Only auto-restart if WE stopped it for screen-off
        // AND the page is still the visible one (the user might have
        // locked the phone on the camera page, then unlocked having
        // swiped to another page in between — don't fight that).
        if (m_stoppedForScreenOff) {
            m_stoppedForScreenOff = false;
            // Resume if the camera page is visible OR we're in scan mode (the
            // scanner can be open over another page, so m_widgetVisible is false
            // but we still need frames). Without the scan-mode clause, a QR scan
            // that survived a screen-off would stay dead after unlock.
            if ((m_widgetVisible || m_scanMode) && m_appActive) {
                qCInfo(lcCamera) << "Restarting backend pipeline after screen-on";
                // Defer via the lifecycle timer path used by showEvent so
                // we get the same debounce + node-selection logic rather
                // than calling start() raw.
                m_pendingStart = true;
                m_pendingStop  = false;
                if (m_lifecycleTimer) m_lifecycleTimer->start();
                if (m_landmarkScoreTimer && !m_landmarkScoreTimer->isActive())
                    m_landmarkScoreTimer->start();
            }
        }
        // Reflect the cleared screen-off gate in the processing-state diagnostic.
        updateProcessingState();
    }
}

CameraWidget::~CameraWidget()
{
    // V32.32: the async CDAF sweep captures a raw pointer to m_vcm.
    // If the widget is destroyed mid-sweep, the worker would hold a
    // dangling pointer to a destroyed VcmController. Wait for any
    // in-flight work to finish before letting destruction proceed.
    //
    // Bounded wait: 2.5s upper bound. The full CDAF sweep is ~3s in
    // the worst case (the rarely-used long sweep path); fast focus is
    // ~750ms. 2.5s covers both. If a sweep hasn't finished by then
    // we proceed anyway and accept the risk — leaving the destructor
    // hung forever on a stuck VCM is worse.
    if (m_focusInProgress.load()) {
        QElapsedTimer wait;
        wait.start();
        while (m_focusInProgress.load() && wait.elapsed() < 2500) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
        }
    }
}

void CameraWidget::buildUi()
{
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);

    // ── Top bar ───────────────────────────────────────────────────────────
    // V32.37+: wrapped in a container widget so full-bleed mode can reparent it
    // onto the viewfinder as a floating overlay. In classic mode it sits in the
    // root QVBoxLayout as before.
    m_topBarWidget = new QWidget(this);
    auto *topBar = new QHBoxLayout(m_topBarWidget);
    topBar->setContentsMargins(Theme::SPACE_SM, Theme::SPACE_SM,
                               Theme::SPACE_SM, Theme::SPACE_XS);

    m_backButton = new Camera::GlassIconButton(QStringLiteral("←"), this);
    m_backButton->setToolTip(tr("Back to Inbox"));
    m_backButton->setFixedSize(Theme::TOUCH_ICON, Theme::TOUCH_ICON);
    topBar->addWidget(m_backButton);
    topBar->addStretch();

    // V32.15: no longer a "selected friend indicator" — that sticky-recipient
    // UX was removed because users forgot who was active in multi-conversation
    // sessions. The send flow now always shows a contact picker after capture,
    // so this label just hints at that. We keep the setRecipient() method
    // because other paths (wayfinder map → contact tap) may want to surface
    // a hint; in the default flow it stays as the prompt below.
    m_recipientLabel = new QLabel(tr("Shoot then choose recipient"), this);
    m_recipientLabel->setObjectName(QStringLiteral("subtitleLabel"));
    m_recipientLabel->setAlignment(Qt::AlignCenter);
    topBar->addWidget(m_recipientLabel);
    topBar->addStretch();

    m_cameraSelector = new QComboBox(this);
    m_cameraSelector->setMaximumWidth(200);
    topBar->addWidget(m_cameraSelector);

    // Rotation override — cycles 0/90/180/270. Useful when sensor orientation
    // is wrong (per-device DTS issues, opposite-mounted front/rear sensors,
    // future regressions). Persists across launches via QSettings.
    m_rotateButton = new Camera::GlassIconButton(QStringLiteral("⟳"), this);
    m_rotateButton->setToolTip(tr("Rotate viewfinder 90°"));
    m_rotateButton->setFixedSize(Theme::TOUCH_ICON, Theme::TOUCH_ICON);
    topBar->addWidget(m_rotateButton);
    // Restore saved rotation
    {
        QSettings s;
        m_userRotation = s.value(QStringLiteral("camera/userRotation"), 0).toInt();
        if (m_userRotation != 0 && m_userRotation != 90
         && m_userRotation != 180 && m_userRotation != 270)
            m_userRotation = 0;
    }
    root->addWidget(m_topBarWidget);

    // ── Viewfinder ────────────────────────────────────────────────────────
    m_viewfinderLabel = new QLabel(this);
    m_viewfinderLabel->setAlignment(Qt::AlignCenter);
    m_viewfinderLabel->setStyleSheet(QStringLiteral("background: #000000;"));
    m_viewfinderLabel->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Expanding);
    // V32.35+: accept touch events directly. Without this, Qt only delivers
    // synthesized mouse events (unreliable on Plasma Mobile), which is why a
    // mouse click focused but a finger tap didn't. With it, TouchBegin reaches
    // our event filter and handleViewfinderTap drives focus/flip.
    m_viewfinderLabel->setAttribute(Qt::WA_AcceptTouchEvents, true);
    // V32.32: tap-to-focus. The event filter catches MousePress on the
    // viewfinder and triggers a fast CDAF sweep. We don't act on a per-
    // region basis (no spot focus yet) — the sweep evaluates whole-frame
    // sharpness. Refining to a tap-region ROI is a future enhancement.
    m_viewfinderLabel->installEventFilter(this);
    root->addWidget(m_viewfinderLabel, 1);

    // ── Bottom bar ────────────────────────────────────────────────────────
    // V32.37+: wrapped in a container widget (see top bar note).
    m_bottomBarWidget = new QWidget(this);
    auto *bottomBar = new QHBoxLayout(m_bottomBarWidget);
    bottomBar->setContentsMargins(0, Theme::SPACE_SM, 0, Theme::SPACE_LG);
    // V32.38+: left counterweight matching the trailing text-snap button so the
    // shutter sits at the true horizontal centre. The bar was
    // [stretch][shutter][stretch][text button] — the text button on the right
    // had no left-side equivalent, pulling the shutter visibly left of centre.
    auto *shutterLeftSpacer = new QWidget(m_bottomBarWidget);
    shutterLeftSpacer->setFixedSize(Theme::TOUCH_ICON, Theme::TOUCH_ICON);
    shutterLeftSpacer->setAttribute(Qt::WA_TransparentForMouseEvents, true);
    bottomBar->addWidget(shutterLeftSpacer);
    bottomBar->addStretch();

    // Shutter button: tap = photo, hold = record video, hold+swipe-up = lock recording.
    // Custom widget (ShutterButton) handles all three gestures from one touch.
    m_shutter = new Video::ShutterButton(this);
    m_shutter->setToolTip(tr("Tap: photo · Hold: video · Hold+swipe up: lock"));
    bottomBar->addWidget(m_shutter, 0, Qt::AlignCenter);

    // Recording indicator overlay (red dot + elapsed) — shown over viewfinder
    // during recording. Click-through so it doesn't block other UI.
    m_recordIndicator = new QLabel(m_viewfinderLabel);
    m_recordIndicator->setAttribute(Qt::WA_TransparentForMouseEvents, true);
    m_recordIndicator->setStyleSheet(QStringLiteral(
        "background: rgba(232,60,60,0.85); color: white;"
        "padding: 4px 12px; border-radius: 12px;"
        "font-weight: 600; font-size: 13px;"));
    m_recordIndicator->setText(QStringLiteral("● REC 0:00"));
    m_recordIndicator->adjustSize();
    m_recordIndicator->move(12, 12);
    m_recordIndicator->hide();

    // V32.34+ AF UI: tap-to-focus reticle. Child of the viewfinder so it
    // composites on top of the live image and resizes-relative is handled
    // by the eventFilter that already repositions PIP children. Hidden by
    // default; shown via triggerAt() from the tap handler.
    m_focusReticle = new Camera::FocusReticle(m_viewfinderLabel);

    // V32.34+ AF UI: persistent "AF available" badge. Tells the user at a
    // glance whether tapping the viewfinder will actually do anything.
    // Hidden by default — shown when we know the AF state for the
    // current camera. Updated by updateAfBadge() called from camera
    // selection change + post-VCM-open.
    m_afBadge = new QLabel(m_viewfinderLabel);
    m_afBadge->setAttribute(Qt::WA_TransparentForMouseEvents, true);
    m_afBadge->setStyleSheet(QStringLiteral(
        "background: rgba(0,0,0,0.55); color: rgba(255,255,255,0.92);"
        "padding: 3px 8px; border-radius: 8px;"
        "font-weight: 600; font-size: 11px; letter-spacing: 0.5px;"));
    m_afBadge->hide();

    // V32.32 / V32.34+ wiring: landmark candidate hint pill. Hidden until
    // the periodic scorer (m_landmarkScoreTimer) sees a frame score above
    // the threshold. Clickable (not transparent-for-mouse) so a tap opens
    // the AddLandmarkDialog with the frame attached. Positioned at the
    // bottom-center of the viewfinder so it's visible without obscuring
    // the subject — repositioned on resize via eventFilter.
    m_landmarkHint = new QLabel(m_viewfinderLabel);
    m_landmarkHint->setText(tr("+ Save as landmark"));
    m_landmarkHint->setAlignment(Qt::AlignCenter);
    m_landmarkHint->setStyleSheet(QStringLiteral(
        "background: rgba(0,0,0,0.6); color: rgba(255,255,255,0.95);"
        "padding: 7px 16px; border-radius: 14px;"
        "font-weight: 500; font-size: 13px;"));
    m_landmarkHint->setCursor(Qt::PointingHandCursor);
    m_landmarkHint->installEventFilter(this);
    m_landmarkHint->hide();

    // Auto-hide timer: if the user doesn't react within ~4.5s we fade
    // the pill out. The pill re-appears next time the scorer crosses
    // threshold (and the scene isn't in the cooldown table).
    m_landmarkHintAutoHide = new QTimer(this);
    m_landmarkHintAutoHide->setSingleShot(true);
    m_landmarkHintAutoHide->setInterval(kLandmarkHintAutoHideMs);
    connect(m_landmarkHintAutoHide, &QTimer::timeout, this, [this]() {
        if (m_landmarkHint) m_landmarkHint->hide();
    });

    // Periodic scorer: runs while camera is live + viewfinder visible.
    // Started in showEvent, stopped in hideEvent. 1Hz cadence balances
    // responsiveness (the user perceives "the app noticed this scene"
    // within a second) against battery/CPU cost (the score is cheap but
    // not free, and the cooldown logic already filters re-shows).
    m_landmarkScoreTimer = new QTimer(this);
    m_landmarkScoreTimer->setInterval(kLandmarkScoreIntervalMs);
    connect(m_landmarkScoreTimer, &QTimer::timeout,
            this, &CameraWidget::runLandmarkScoreTick);

    bottomBar->addStretch();

    m_textOnlyButton = new QPushButton(QStringLiteral("T"), this);
    m_textOnlyButton->setObjectName(QStringLiteral("textSnapBtn"));
    m_textOnlyButton->setFixedSize(Theme::TOUCH_ICON, Theme::TOUCH_ICON);
    m_textOnlyButton->setToolTip(tr("New message — voice chat or text snap"));
    m_textOnlyButton->setStyleSheet(QStringLiteral(
        "QPushButton { background: rgba(0,0,0,0.55); color: white;"
        "  border: 2px solid rgba(255,255,255,0.5); border-radius: %1px;"
        "  font-size: 20px; font-weight: bold; }"
        "QPushButton:pressed { background: rgba(255,255,255,0.15); }")
        .arg(Theme::TOUCH_ICON / 2));
    bottomBar->addWidget(m_textOnlyButton);
    root->addWidget(m_bottomBarWidget);

    connect(m_shutter, &Video::ShutterButton::clicked,
            this, &CameraWidget::onCaptureClicked);
    connect(m_shutter, &Video::ShutterButton::recordStarted,
            this, &CameraWidget::onRecordStarted);
    connect(m_shutter, &Video::ShutterButton::recordLocked,
            this, &CameraWidget::onRecordLocked);
    connect(m_shutter, &Video::ShutterButton::recordStopped,
            this, &CameraWidget::onRecordStopped);
    connect(m_shutter, &Video::ShutterButton::voiceModeRequested,
            this, &CameraWidget::voiceChatRequested);
    connect(m_textOnlyButton, &QPushButton::clicked, this, [this]() {
        QMenu menu(this);
        menu.addAction(tr("Voice chat"), this, &CameraWidget::voiceChatRequested);
        menu.addAction(tr("Text snap"),  this, &CameraWidget::onTextOnlyClicked);
        menu.exec(m_textOnlyButton->mapToGlobal(
            QPoint(0, -menu.sizeHint().height())));
    });
    connect(m_backButton, &QPushButton::clicked,
            this, &CameraWidget::backRequested);
    connect(m_cameraSelector, &QComboBox::currentIndexChanged,
            this, &CameraWidget::onSelectorChanged);
    connect(m_rotateButton, &QPushButton::clicked,
            this, &CameraWidget::onRotateClicked);
}

// ─── Backend selection ───────────────────────────────────────────────────────

void CameraWidget::selectBackend()
{
    if (m_backend) return;  // already chosen

    // postmarketOS: prefer GstLaunchBackend (works through camera-bridge).
    // Fall back to Qt if no PipeWire virtual cams exist.
    // Other platforms: prefer Qt; fall back to gst-launch only on demand.
    auto tryGst = [this]() {
        auto gst = std::make_unique<Camera::GstLaunchBackend>(this);
        if (!gst->availableDevices().isEmpty()) {
            m_backend = std::move(gst);
            return true;
        }
        return false;
    };
    auto tryQt = [this]() {
        auto qt = std::make_unique<Camera::QtCameraBackend>(this);
        if (!qt->availableDevices().isEmpty()) {
            m_backend = std::move(qt);
            return true;
        }
        return false;
    };

#ifdef SNAPCLONE_LIBCAMERA
    auto tryLibcamera = [this]() {
        auto lc = std::make_unique<Camera::LibcameraBackend>(this);
        if (!lc->availableDevices().isEmpty()) {
            m_backend = std::move(lc);
            return true;
        }
        return false;
    };
#endif

    // V32.39+: honor the user's rear-camera backend preference (the rear is the
    // startup camera). "libcamera" is the default; "auto" falls through to the
    // platform ordering below. If the chosen backend reports no devices we fall
    // through too, so a bad pick can never leave the camera dead.
    {
        const QString pref = QSettings(QStringLiteral("SnapCloneProject"),
                                       QStringLiteral("SnapClone"))
            .value(QStringLiteral("camera/rearBackend"),
                   QStringLiteral("libcamera")).toString();
        if (pref == QLatin1String("libcamera")) {
#ifdef SNAPCLONE_LIBCAMERA
            if (tryLibcamera())
                qCInfo(lcCamera) << "Backend: LibcameraBackend (preference rear=libcamera)";
#endif
        } else if (pref == QLatin1String("gstreamer")) {
            if (tryGst())
                qCInfo(lcCamera) << "Backend: GstLaunchBackend (preference rear=gstreamer)";
        } else if (pref == QLatin1String("qt")) {
            if (tryQt())
                qCInfo(lcCamera) << "Backend: QtCameraBackend (preference rear=qt)";
        }
        // "auto", or the preferred backend had no devices → platform order below.
    }

    // V32.39+: libcamera is the native camera stack on mobile Linux, so it is
    // now PREFERRED over GstLaunchBackend there; gst-launch (through the
    // camera-bridge) is demoted to the fallback, with Qt last. On desktop,
    // Qt's QCamera stays primary (well-tested), libcamera comes next (it can
    // also drive UVC/V4L2), gst-launch last. Guarded by SNAPCLONE_LIBCAMERA so
    // builds without libcamera-dev keep the original gst/Qt ordering.
    if (!m_backend && isPostmarketOS()) {
        bool ok = false;
#ifdef SNAPCLONE_LIBCAMERA
        if (tryLibcamera()) {
            qCInfo(lcCamera) << "Backend: LibcameraBackend (postmarketOS, direct libcamera)";
            ok = true;
        }
#endif
        if (!ok && tryGst()) {
            qCInfo(lcCamera) << "Backend: GstLaunchBackend (pmOS, libcamera unavailable)";
            ok = true;
        }
        if (!ok && tryQt()) {
            qCInfo(lcCamera) << "Backend: QtCameraBackend (pmOS last-resort)";
            ok = true;
        }
        if (!ok) qCWarning(lcCamera) << "No backend has available devices yet";
    } else if (!m_backend) {
        bool ok = tryQt();
        if (ok) qCInfo(lcCamera) << "Backend: QtCameraBackend (desktop)";
#ifdef SNAPCLONE_LIBCAMERA
        if (!ok && tryLibcamera()) {
            qCInfo(lcCamera) << "Backend: LibcameraBackend (desktop, direct libcamera)";
            ok = true;
        }
#endif
        if (!ok && tryGst()) {
            qCInfo(lcCamera) << "Backend: GstLaunchBackend (desktop fallback)";
            ok = true;
        }
        if (!ok) qCWarning(lcCamera) << "No backend has available devices yet";
    }

    if (m_backend) {
        connect(m_backend.get(), &Camera::CameraBackend::framePreview,
                this, &CameraWidget::onPreviewFrame);
        connect(m_backend.get(), &Camera::CameraBackend::frameCaptured,
                this, &CameraWidget::onFrameCaptured);
        connect(m_backend.get(), &Camera::CameraBackend::error,
                this, &CameraWidget::onBackendError);
        connect(m_backend.get(), &Camera::CameraBackend::deviceListChanged,
                this, &CameraWidget::onBackendDeviceListChanged);
        // V32.34+: adaptive fallback transparency. When the backend
        // silently swapped to a different node than we asked for
        // (the alternate-node retry path), update the selector so
        // the user can SEE that the active camera isn't what they
        // picked. Without this, the UI label says "Rear Camera" but
        // the frames are from the front sensor (or whichever node
        // the backend fell over to) — which is exactly the Phone B
        // bug report.
        //
        // We BLOCK the selector's signal during the change so the
        // selector update doesn't fire onDeviceSelectorChanged
        // → m_backend->start() again (which would loop or thrash).
        connect(m_backend.get(), &Camera::CameraBackend::activeNodeChanged,
                this, [this](const QString &requested,
                             const QString &actual,
                             const QString &reason) {
            qCInfo(lcCamera) << "Backend swapped node:" << requested
                             << "→" << actual << "reason:" << reason;
            if (!m_cameraSelector) return;
            // Find the selector entry for the actual node and select
            // it without firing currentIndexChanged. If the actual
            // node isn't in the list (rare — it should be since the
            // backend pulled it from availableDevices), do nothing.
            const int idx = m_cameraSelector->findData(actual);
            if (idx < 0) {
                qCWarning(lcCamera) << "Node" << actual
                                    << "not in selector — list out of date?";
                return;
            }
            QSignalBlocker blocker(m_cameraSelector);
            m_cameraSelector->setCurrentIndex(idx);
            // Surface a brief notice so the user knows the swap
            // happened. Reuse the existing camera-banner mechanism
            // if any, otherwise log + skip — better than silently
            // misleading the user.
            // TODO(banner): if a banner widget exists in this widget,
            // call show(text) on it here. For now just log.
        });
        // V32.34+: tell CameraHealthService which device is in use so
        // probes don't compete with the live capture. We TRACK the id
        // we marked (m_inUseDeviceId) so the stopped() handler releases
        // only ours — important because the PIP backend is a separate
        // CameraWidget session with its own m_inUseDeviceId, and we
        // must not unset the other widget's in-use device on our stop.
        connect(m_backend.get(), &Camera::CameraBackend::firstFrameReceived,
                this, [this]() {
            // The backend doesn't tell us which device it bound to,
            // so we infer from the device selector. This is correct
            // as long as the selector is single source of truth for
            // "what we asked for" — which it is in this widget.
            const QString id = m_cameraSelector
                ? m_cameraSelector->currentData().toString() : QString();
            if (id.isEmpty() || id == QString::fromLatin1(kPipSentinel)
                || id == QString::fromLatin1(kPipExtSentinel)) return;

            // If we were already holding a DIFFERENT id (camera switch
            // before previous stop fired), release the old one first.
            if (!m_inUseDeviceId.isEmpty() && m_inUseDeviceId != id) {
                Camera::CameraHealthService::instance()
                    .setDeviceInUse(m_inUseDeviceId, false);
            }
            m_inUseDeviceId = id;
            Camera::CameraHealthService::instance().setDeviceInUse(id, true);
        });
        connect(m_backend.get(), &Camera::CameraBackend::stopped,
                this, [this]() {
            // Release exactly the id we held — don't walk available
            // devices (PIP widget may have OTHER ids marked in-use,
            // and clearing those would tell the health service the
            // PIP camera is no longer running when it actually is).
            if (m_inUseDeviceId.isEmpty()) return;
            Camera::CameraHealthService::instance()
                .setDeviceInUse(m_inUseDeviceId, false);
            m_inUseDeviceId.clear();
        });
    }
}

// ─── Device selector population ──────────────────────────────────────────────

void CameraWidget::refreshDeviceSelector()
{
    if (!m_backend) return;
    m_cameraSelector->blockSignals(true);
    const QString currentId = m_cameraSelector->currentData().toString();
    m_cameraSelector->clear();
    int restoreIndex = -1;
    int i = 0;
    for (const auto &d : m_backend->availableDevices()) {
        m_cameraSelector->addItem(d.displayName, d.id);
        if (d.id == currentId) restoreIndex = i;
        ++i;
    }
    // V32.33: append PIP entry if both a rear and a front camera are
    // available. The entry's itemData is the kPipSentinel string, which
    // onSelectorChanged distinguishes from real device ids.
    if (backendHasFrontAndRear()) {
        m_cameraSelector->addItem(tr("Rear + Selfie PIP"),
                                  QString::fromLatin1(kPipSentinel));
        if (currentId == QString::fromLatin1(kPipSentinel)) {
            restoreIndex = i;  // restore PIP selection if it was active
        }
        ++i;
    }
    // V32.38+: internal webcam + external-USB PIP entry. Shown when the backend
    // exposes both an internal/user-facing camera and an external USB one
    // (desktop with a plugged-in webcam). onSelectorChanged distinguishes this
    // sentinel from the rear+front one and from real device ids.
    if (backendCanInternalPlusExternalPip()) {
        m_cameraSelector->addItem(tr("Camera + USB PIP"),
                                  QString::fromLatin1(kPipExtSentinel));
        if (currentId == QString::fromLatin1(kPipExtSentinel)) {
            restoreIndex = i;
        }
        ++i;
    }
    if (restoreIndex >= 0) m_cameraSelector->setCurrentIndex(restoreIndex);
    m_cameraSelector->blockSignals(false);
}

void CameraWidget::onSelectorChanged(int index)
{
    if (!m_backend || index < 0) return;
    // Refuse mid-recording switches — they cause device-busy chaos and
    // leave the recording tied to an old device that's no longer in the
    // viewfinder. The selector is also disabled visually while recording.
    if (m_recorder || m_transcoder) {
        qCInfo(lcCamera) << "Camera switch refused — recording in progress";
        return;
    }
    const QString id = m_cameraSelector->currentData().toString();
    if (id.isEmpty()) return;

    // V32.33: PIP mode — selected entry is the special "Rear + Selfie PIP".
    // Start MAIN backend on the first rear, start PIP backend on the first
    // front. Bypass the single-device path below entirely.
    if (id == QString::fromLatin1(kPipSentinel)) {
        // V32.34+ PiP FIX: hard guard. Even if a stale PiP selection survives
        // (saved setting, device list changed), do NOT attempt two simultaneous
        // streams on hardware that can't do it — that's the silent-black-tile
        // failure. Surface a clear reason and fall back to the rear camera.
        if (!Camera::CameraPolicyStore::instance().snapshot().simultaneousCaptureSupported) {
            qCInfo(lcCamera) << "PIP requested but simultaneous capture is not "
                                "enabled for this device — falling back to rear";
            const QString rearOnly = firstRearDeviceId();
            refreshDeviceSelector();
            if (!rearOnly.isEmpty()) {
                stopPip();
                m_backend->stop();
                m_backend->start(rearOnly);
            }
            return;
        }
        const QString rearId  = firstRearDeviceId();
        const QString frontId = firstFrontDeviceId();
        if (rearId.isEmpty() || frontId.isEmpty()) {
            // backendHasFrontAndRear was true at populate-time but devices
            // have changed since. Refresh selector and quietly bail.
            qCInfo(lcCamera) << "PIP mode unavailable — missing rear or front device";
            refreshDeviceSelector();
            return;
        }
        qCInfo(lcCamera) << "PIP mode: main=" << rearId << " pip=" << frontId;
        // Main camera = rear, not mirrored.
        m_mirrorPreview = false;
        // NOTE: the focus-teardown sequence below (focusInProgress wait,
        // VCM close, scene-change timer stop, focus baseline reset) is
        // intentionally duplicated from the single-device switch path
        // below. If you change the focus lifecycle, change BOTH branches
        // — they need to agree on what state survives a camera switch.
        // Factoring this into a helper is filed (low priority — the dance
        // is mostly one-liners and the duplication is locally obvious).
        if (m_focusInProgress.load()) {
            QElapsedTimer wait;
            wait.start();
            while (m_focusInProgress.load() && wait.elapsed() < 2000) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
            }
        }
        if (m_vcm) m_vcm->close();
        m_vcm.reset();
        if (m_vcmOpenTimer) {
            m_vcmOpenTimer->stop();
            m_vcmOpenTimer->deleteLater();
            m_vcmOpenTimer = nullptr;
        }
        if (m_sceneChangeTimer) m_sceneChangeTimer->stop();
        m_focusBaseline = 0.0;

        m_backend->stop();
        m_backend->start(rearId);
        // Front camera is user-facing → mirror the PIP overlay (selfie).
        startPip(frontId, /*mirror=*/true);
        return;
    }

    // V32.38+: internal webcam + external-USB PIP. MAIN = the internal /
    // built-in webcam, PIP overlay = the external USB camera. Parallel to the
    // rear+front branch above; the focus-teardown dance below is duplicated for
    // the same reason — it must agree with the single-device switch path.
    if (id == QString::fromLatin1(kPipExtSentinel)) {
        if (!Camera::CameraPolicyStore::instance().snapshot().simultaneousCaptureSupported) {
            qCInfo(lcCamera) << "USB-PIP requested but simultaneous capture is "
                                "not enabled for this device — falling back";
            const QString internalOnly = firstInternalDeviceId();
            refreshDeviceSelector();
            if (!internalOnly.isEmpty()) {
                stopPip();
                m_backend->stop();
                m_backend->start(internalOnly);
            }
            return;
        }
        const QString mainId = firstInternalDeviceId();
        const QString extId  = firstExternalDeviceId();
        if (mainId.isEmpty() || extId.isEmpty()) {
            qCInfo(lcCamera) << "USB-PIP unavailable — missing internal or "
                                "external device";
            refreshDeviceSelector();
            return;
        }
        qCInfo(lcCamera) << "USB-PIP mode: main=" << mainId << " pip=" << extId;
        // Main = internal/user-facing webcam → mirror the main preview (selfie).
        m_mirrorPreview = true;
        if (m_focusInProgress.load()) {
            QElapsedTimer wait;
            wait.start();
            while (m_focusInProgress.load() && wait.elapsed() < 2000) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
            }
        }
        if (m_vcm) m_vcm->close();
        m_vcm.reset();
        if (m_vcmOpenTimer) {
            m_vcmOpenTimer->stop();
            m_vcmOpenTimer->deleteLater();
            m_vcmOpenTimer = nullptr;
        }
        if (m_sceneChangeTimer) m_sceneChangeTimer->stop();
        m_focusBaseline = 0.0;

        m_backend->stop();
        m_backend->start(mainId);
        // External scene camera → do NOT mirror the PIP overlay.
        startPip(extId, /*mirror=*/false);
        return;
    }

    // Not PIP mode — make sure any existing PIP is torn down.
    stopPip();

    qCInfo(lcCamera) << "Switching to device:" << id;
    m_previewFrameValidated = false;  // re-validate the new stream's frame layout
    // V32.32 (revision): mirror state from CameraRegistry, not substring
    // matching. The registry has typed Facing + mirrorPreview metadata
    // per known virtual camera, so adding sensors doesn't require
    // patching this file. Unknown camera ids (custom user-supplied
    // nodes) fall back to "no mirror" — safe default for rear cams,
    // mildly wrong for a custom front. Users can override by setting
    // their custom node's metadata in CameraRegistry's kTable.
    // V32.42+: mirror via CameraRegistry, with a name-rule fallback. Virtual-node
    // cameras carry typed mirror metadata; a USB/UVC webcam opened through
    // QtCameraBackend (e.g. "FaceTime HD Camera") has a Qt device id that matches
    // no table entry, so byId() alone left it un-mirrored — mirrorForDevice falls
    // back to the front-facing rule list on the display name.
    m_mirrorPreview = Camera::CameraRegistry::instance()
                          .mirrorForDevice(id, m_cameraSelector->currentText());
    qCInfo(lcCamera) << "Mirror preview:" << m_mirrorPreview;

    // V32.32: tear down VCM on switch. The new camera may not have AF
    // (front, ultrawide), AND even if it does, the AFVDD_2P8 rail
    // power-cycles when the sensor changes — the VCM has to be re-
    // opened after the new stream stabilises. Clearing m_vcmOpenTimer
    // means onPreviewFrame will re-check supportsFocus and schedule
    // a fresh open if appropriate.
    //
    // V32.32 (revision): if a CDAF sweep is currently running on the
    // worker thread, we MUST wait for it to finish before destroying
    // VcmController — the worker holds a raw pointer to it. The wait
    // is bounded (~750ms max for fast sweep) and the user expects a
    // camera switch to take a moment. Spin-polling here is acceptable
    // because the work happens on another thread and we're just
    // observing a flag.
    if (m_focusInProgress.load()) {
        // Pump the event loop while waiting so the QFutureWatcher's
        // finished signal can deliver back to us. processEvents with a
        // budget prevents accidental reentry from runaway delivery.
        QElapsedTimer wait;
        wait.start();
        while (m_focusInProgress.load() && wait.elapsed() < 2000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
        }
    }
    if (m_vcm) m_vcm->close();
    m_vcm.reset();
    if (m_vcmOpenTimer) {
        m_vcmOpenTimer->stop();
        m_vcmOpenTimer->deleteLater();
        m_vcmOpenTimer = nullptr;
    }
    // V32.32: stop scene-change monitoring on camera switch too. The
    // baseline is camera-specific (different sensor = different
    // sharpness distribution); we re-establish it after the next
    // successful focus on the new camera.
    if (m_sceneChangeTimer) {
        m_sceneChangeTimer->stop();
    }
    m_focusBaseline = 0.0;

    // V32.34+ AF UI: refresh AF badge for the new camera. Even before
    // the new stream starts, we know whether AF is supported (CameraRegistry
    // has it indexed), so the badge can communicate the state of the
    // upcoming camera immediately rather than waiting for first-frame.
    updateAfBadge();

    m_backend->stop();
    // V32.34+: clear the viewfinder during the switch so the user
    // doesn't see a stale frame from the previous camera. The first
    // frame from the new camera (typically <500ms later, longer on
    // FP5's libcamera) will overwrite this. Without clearing, the
    // user perceives a "frozen camera" for that interval — confusing
    // when they just tapped to switch cameras.
    if (m_viewfinderLabel) {
        m_viewfinderLabel->setPixmap(QPixmap());
        m_viewfinderLabel->setText(tr("Switching camera…"));
        m_viewfinderLabel->setStyleSheet(QStringLiteral(
            "background: #1a1a1a; color: #888;"
            "font-size: %1px; padding: 20px;").arg(Theme::FONT_SM));
    }
    // V32.38n: also clear the CACHED frame, not just the visible pixmap. The
    // resize/resume re-render (eventFilter Resize) and the CDAF focus sweep both
    // read m_latestFrame; leaving the previous camera's frame there means a
    // resize during the switch re-blits the OLD camera's image, and a focus
    // sweep could evaluate sharpness on the stale frame and drive the NEW
    // camera's VCM to a wrong position. Clear under the mutex (a focus worker
    // may read it concurrently). m_firstFrameSeen-style consumers already null-
    // check via isNull().
    {
        QMutexLocker lock(&m_frameMutex);
        m_latestFrame = QImage();
    }
    m_backend->start(id);
}

// ─── V32.33: Picture-in-Picture ──────────────────────────────────────────────
//
// See header for the design rationale. Short version: a second camera
// backend runs concurrently when the user picks the "Rear + Selfie PIP"
// dropdown entry. The second backend drives a small overlay; the main
// backend continues to drive the full viewfinder. Capture path is
// unchanged for v1 (saved snap is main camera only).

bool CameraWidget::backendHasFrontAndRear() const
{
    if (!m_backend) return false;
    // V32.34+ PiP FIX: PiP needs TWO cameras LIVE AT ONCE. On single-ISP mobile
    // hardware (FP5) the front and rear share one pipeline and can't both run —
    // opening the second fails negotiation, which is why PiP appeared to do
    // nothing. Only offer PiP when the device is known to support simultaneous
    // capture (desktop / independent sensors / USB webcams). Default false; set
    // camera/policy/simultaneousCapture=true to enable. The richer per-source
    // concurrency model lives in the Rust snapclone-videosrc crate; this is the
    // conservative gate until that's wired through the registry.
    if (!Camera::CameraPolicyStore::instance().snapshot().simultaneousCaptureSupported) {
        return false;
    }
    bool haveRear = false, haveFront = false;
    for (const auto &d : m_backend->availableDevices()) {
        const auto info = Camera::CameraRegistry::instance().byId(d.id);
        if (!info) continue;
        if (info->facing == Camera::Facing::Rear)  haveRear  = true;
        if (info->facing == Camera::Facing::Front) haveFront = true;
        if (haveRear && haveFront) return true;
    }
    return false;
}

QString CameraWidget::firstRearDeviceId() const
{
    if (!m_backend) return {};
    // V32.34+: prefer a health-confirmed Rear device when the health
    // service has cached results. Falls back to the first registry
    // Rear if no probes have run yet (cold start) or all rears are
    // marked failed.
    //
    // We deliberately don't kick off a probe scan from here — picking
    // the default device is on the hot path (called every time the
    // camera tab is opened), and a 1-5s blocking wait would hurt UX.
    // The first defaultDeviceId() call returns the registry pick;
    // subsequent calls after the user visits Settings (which calls
    // ensureProbed()) get the optimized pick.
    const auto &health = Camera::CameraHealthService::instance();
    if (health.hasResults()) {
        const QString hi = health.bestWorkingDeviceId(Camera::Facing::Rear);
        if (!hi.isEmpty()) {
            // Cross-check with backend's view of available devices —
            // the health cache might mention a node that has since
            // disappeared (bridge restarted, USB unplugged).
            for (const auto &d : m_backend->availableDevices()) {
                if (d.id == hi) return hi;
            }
        }
    }
    for (const auto &d : m_backend->availableDevices()) {
        const auto info = Camera::CameraRegistry::instance().byId(d.id);
        if (info && info->facing == Camera::Facing::Rear) return d.id;
    }
    return {};
}

QString CameraWidget::firstFrontDeviceId() const
{
    if (!m_backend) return {};
    // V32.34+: same health-preference pattern as firstRearDeviceId.
    const auto &health = Camera::CameraHealthService::instance();
    if (health.hasResults()) {
        const QString hi = health.bestWorkingDeviceId(Camera::Facing::Front);
        if (!hi.isEmpty()) {
            for (const auto &d : m_backend->availableDevices()) {
                if (d.id == hi) return hi;
            }
        }
    }
    for (const auto &d : m_backend->availableDevices()) {
        const auto info = Camera::CameraRegistry::instance().byId(d.id);
        if (info && info->facing == Camera::Facing::Front) return d.id;
    }
    return {};
}

bool CameraWidget::isInternalCamera(const QString &id, const QString &displayName)
{
    const auto info = Camera::CameraRegistry::instance().byId(id);
    if (info && (info->facing == Camera::Facing::Front
                 || info->facing == Camera::Facing::Rear
                 || info->facing == Camera::Facing::Tracking)) {
        return true;
    }
    // Desktop: a built-in webcam isn't a virtual-cam-* node, so byId() comes
    // back empty — fall back to the shared user-facing-webcam name heuristic.
    return Camera::CameraRegistry::isUserFacingWebcamModel(displayName);
}

QString CameraWidget::firstInternalDeviceId() const
{
    if (!m_backend) return {};
    for (const auto &d : m_backend->availableDevices()) {
        if (isInternalCamera(d.id, d.displayName)) return d.id;
    }
    return {};
}

QString CameraWidget::firstExternalDeviceId() const
{
    if (!m_backend) return {};
    // "External" = a real camera that ISN'T one of our internal sensors and
    // whose name isn't a recognised built-in webcam — i.e. a plugged-in USB
    // camera. PIP sentinels never appear in availableDevices(), so there's
    // nothing to guard against here.
    for (const auto &d : m_backend->availableDevices()) {
        if (!isInternalCamera(d.id, d.displayName)) return d.id;
    }
    return {};
}

bool CameraWidget::backendCanInternalPlusExternalPip() const
{
    if (!m_backend) return false;
    // Same single-pipeline guard as backendHasFrontAndRear(): two live streams
    // require hardware that can actually do simultaneous capture.
    if (!Camera::CameraPolicyStore::instance().snapshot().simultaneousCaptureSupported)
        return false;
    bool haveInternal = false, haveExternal = false;
    for (const auto &d : m_backend->availableDevices()) {
        if (isInternalCamera(d.id, d.displayName)) haveInternal = true;
        else                                       haveExternal = true;
        if (haveInternal && haveExternal) return true;
    }
    return false;
}

void CameraWidget::startPip(const QString &pipDeviceId, bool mirror)
{
    if (!m_backend) return;
    // V32.38+: generalised from the old hardwired front camera. The caller
    // chooses the PIP device (front for "Rear + Selfie PIP", external USB for
    // "Camera + USB PIP") and whether the overlay is mirrored.
    m_pipDeviceId = pipDeviceId;
    m_pipMirror   = mirror;
    const QString frontId = pipDeviceId;  // local alias keeps the body below intact
    qCInfo(lcCamera) << "startPip: device =" << frontId << "mirror =" << mirror;
    if (frontId.isEmpty()) {
        qCInfo(lcCamera) << "PIP: no PIP device — skipping";
        return;
    }

    // Lazily create the overlay label on first PIP activation. Parented
    // to the viewfinder label so it composites on top automatically and
    // moves with viewfinder resizes (we reposition in eventFilter).
    if (!m_pipOverlayLabel) {
        m_pipOverlayLabel = new QLabel(m_viewfinderLabel);
        m_pipOverlayLabel->setAlignment(Qt::AlignCenter);
        // Visual styling: dark background while waiting for frames,
        // rounded corners, thin border so it reads as a distinct surface
        // over the main viewfinder. WA_TransparentForMouseEvents so tap-
        // to-focus on the main viewfinder still works when the user taps
        // through the PIP area (PIP tap behavior is "nothing" per v1).
        m_pipOverlayLabel->setAttribute(Qt::WA_TransparentForMouseEvents, true);
        m_pipOverlayLabel->setStyleSheet(QStringLiteral(
            "background: #111111;"
            "border: 2px solid #ffffff;"
            "border-radius: 8px;"
        ));
        m_pipOverlayLabel->setFixedSize(kPipOverlayW, kPipOverlayH);
    }

    // Reuse same backend CLASS as main. The two known backends both
    // construct without args and figure out devices themselves via
    // start(deviceId).
    if (!m_pipBackend) {
        // Determine which backend class is active and mirror it.
        // Both classes are forward-declared in the header; we include
        // their concrete headers at the top of this file so dynamic_cast
        // works.
        if (dynamic_cast<Camera::QtCameraBackend *>(m_backend.get())) {
            m_pipBackend = std::make_unique<Camera::QtCameraBackend>(this);
        } else if (dynamic_cast<Camera::GstLaunchBackend *>(m_backend.get())) {
            m_pipBackend = std::make_unique<Camera::GstLaunchBackend>(this);
        } else {
            qCWarning(lcCamera) << "PIP: unknown backend class, can't mirror";
            return;
        }
        // We only wire framePreview — capture and device-list signals
        // from the PIP backend are intentionally ignored for v1.
        connect(m_pipBackend.get(), &Camera::CameraBackend::framePreview,
                this, &CameraWidget::onPipFramePreview);
        // V32.34+: track PIP's in-use device separately from the main
        // backend's. See CameraWidget.hpp for the per-id tracking
        // rationale.
        connect(m_pipBackend.get(), &Camera::CameraBackend::firstFrameReceived,
                this, [this, frontId]() {
            if (frontId.isEmpty()) return;
            if (!m_pipInUseDeviceId.isEmpty() && m_pipInUseDeviceId != frontId) {
                Camera::CameraHealthService::instance()
                    .setDeviceInUse(m_pipInUseDeviceId, false);
            }
            m_pipInUseDeviceId = frontId;
            Camera::CameraHealthService::instance()
                .setDeviceInUse(frontId, true);
        });
        connect(m_pipBackend.get(), &Camera::CameraBackend::stopped,
                this, [this]() {
            if (m_pipInUseDeviceId.isEmpty()) return;
            Camera::CameraHealthService::instance()
                .setDeviceInUse(m_pipInUseDeviceId, false);
            m_pipInUseDeviceId.clear();
        });
        // V32.34+: PIP backend was previously silent on errors. If
        // negotiation failed mid-session the user just saw a frozen
        // black overlay. Now we log + hide the overlay so the
        // failure is visible.
        connect(m_pipBackend.get(), &Camera::CameraBackend::error,
                this, [this](const QString &msg) {
            qCWarning(lcCamera) << "PIP backend error:" << msg
                                << "— hiding overlay";
            if (m_pipOverlayLabel) m_pipOverlayLabel->hide();
        });
        // V32.34+: if PIP backend silently swapped to a different
        // node (the alt-retry path), log it. Less critical for PIP
        // than the main camera since PIP is a "show the other side"
        // affordance, but knowing the swap happened helps diagnose
        // "why does my PIP show the rear camera?".
        connect(m_pipBackend.get(), &Camera::CameraBackend::activeNodeChanged,
                this, [](const QString &req, const QString &act,
                         const QString &reason) {
            qCInfo(lcCamera) << "PIP backend swapped:" << req << "→" << act
                             << "reason:" << reason;
        });
    }

    if (!m_pipBackend->start(frontId)) {
        qCWarning(lcCamera) << "PIP: backend start failed for" << frontId;
        // V32.34+: distinguish transient failure (bridge not yet
        // publishing nodes) from hard failure (no front camera at
        // all). The backend's start() returning false during bridge
        // startup is common; we don't want to permanently disable
        // PIP for what's really a 1.5s delay.
        //
        // We retry ONCE after 1500ms. If that also fails, we treat
        // it as hard failure and hide the overlay. m_pipRetried
        // gates so we don't loop.
        if (!m_pipRetried) {
            m_pipRetried = true;
            qCInfo(lcCamera) << "PIP: scheduling retry in 1500ms";
            QPointer<CameraWidget> self(this);
            QTimer::singleShot(1500, this, [self]() {
                if (!self || !self->m_pipBackend) return;
                // Only retry if we still WANT PIP active — user
                // may have switched modes since we scheduled this.
                if (self->m_pipOverlayLabel && self->m_pipOverlayLabel->isVisible()) {
                    return;  // already visible — earlier success path took over
                }
                const QString frontId2 = self->m_pipDeviceId;
                if (frontId2.isEmpty()) {
                    qCWarning(lcCamera) << "PIP retry: no front cam available";
                    return;
                }
                if (!self->m_pipBackend->start(frontId2)) {
                    qCWarning(lcCamera) << "PIP retry: still failed — giving up";
                    if (self->m_pipOverlayLabel)
                        self->m_pipOverlayLabel->hide();
                } else {
                    qCInfo(lcCamera) << "PIP retry: success on" << frontId2;
                    self->repositionPipOverlay();
                    if (self->m_pipOverlayLabel) {
                        self->m_pipOverlayLabel->show();
                        self->m_pipOverlayLabel->raise();
                    }
                }
            });
        }
        // Hide overlay during retry window so user doesn't see a stale frame.
        if (m_pipOverlayLabel) m_pipOverlayLabel->hide();
        return;
    }
    // Reset retry flag on successful start so future PIP attempts
    // (e.g. user toggled PIP off and back on) get a fresh retry chance.
    m_pipRetried = false;
    qCInfo(lcCamera) << "PIP: backend started for" << frontId
                     << "— overlay size:" << m_pipOverlayLabel->size()
                     << "viewfinder size:" << (m_viewfinderLabel ? m_viewfinderLabel->size() : QSize());
    repositionPipOverlay();
    m_pipOverlayLabel->show();
    m_pipOverlayLabel->raise();
}

void CameraWidget::stopPip()
{
    if (m_pipBackend) {
        m_pipBackend->stop();
        m_pipBackend.reset();
    }
    if (m_pipOverlayLabel) m_pipOverlayLabel->hide();
    // V32.34+: ensure the PIP in-use marker is released even if the
    // backend's stopped() signal didn't fire (e.g. start() never reached
    // the active state). The signal handler above also clears this; it's
    // safe to clear twice.
    if (!m_pipInUseDeviceId.isEmpty()) {
        Camera::CameraHealthService::instance()
            .setDeviceInUse(m_pipInUseDeviceId, false);
        m_pipInUseDeviceId.clear();
    }
    // V32.38+: clear the generalised PIP device/mirror state so a later
    // onPipFramePreview can't apply a stale mirror before startPip re-sets it.
    m_pipDeviceId.clear();
    m_pipMirror = false;
}

void CameraWidget::repositionPipOverlay()
{
    if (!m_pipOverlayLabel || !m_viewfinderLabel) return;
    const int x = m_viewfinderLabel->width()  - kPipOverlayW - kPipOverlayMargin;
    const int y = m_viewfinderLabel->height() - kPipOverlayH - kPipOverlayMargin;
    m_pipOverlayLabel->move(qMax(0, x), qMax(0, y));
}

void CameraWidget::setFullBleed(bool on)
{
    if (on == m_fullBleed) return;          // idempotent
    m_fullBleed = on;
    if (!m_topBarWidget || !m_bottomBarWidget || !m_viewfinderLabel) return;

    auto *root = qobject_cast<QVBoxLayout *>(layout());

    if (on) {
        // ── Go full-bleed ──────────────────────────────────────────────────
        // Pull the bars out of the stacked layout and float them over the
        // viewfinder. The viewfinder already has Expanding size policy and is
        // the only remaining stretch item, so it fills the whole widget.
        if (root) {
            root->removeWidget(m_topBarWidget);
            root->removeWidget(m_bottomBarWidget);
        }
        // V32.38+ FIX (taps were dead in full-bleed): the previous version made
        // the whole BAR WA_TransparentForMouseEvents and relied on the (false)
        // assumption that the attribute "does not propagate to children." For a
        // child widget it effectively does: Qt hit-tests the mouse-transparent
        // bar as see-through across its WHOLE rect — including where its buttons
        // sit — and forwards the press to the viewfinder behind it, so the
        // buttons never get the event. Result: visible but dead controls.
        //
        // Correct split: a separate SCRIM layer carries the gradient and is the
        // only mouse-transparent piece; the BAR stays fully interactive (its
        // buttons receive taps) with a transparent background. Empty regions of
        // the frame are covered by the scrim (taps pass through to the
        // viewfinder for tap-to-focus); the bar only occupies its controls.
        // V32.38c FIX (taps STILL dead after the mouse-transparency fix): the
        // bars were parented to m_viewfinderLabel (a QLabel). A QLabel clips
        // child MOUSE-EVENT delivery to its content rect — children paint, and
        // Tab focus traversal still reaches them (it ignores geometry), but a
        // click is not delivered to a child Qt considers outside the parent's
        // hit region. Runtime childAt() at each button center returned nullptr.
        // Symptom matches exactly: Tab works, mouse/touch doesn't.
        //
        // Fix: parent the bars (and the scrims) to `this` (the CameraWidget),
        // not the label. In full-bleed the viewfinder fills the whole widget, so
        // this-coords == viewfinder-coords and the geometry math is unchanged —
        // but `this` is a plain QWidget that delivers events to its children
        // normally. The scrims feed off viewfinder-sized images; same coords.
        for (QWidget *scrim : { ensureTopScrim(), ensureBottomScrim() }) {
            scrim->setParent(this);
            scrim->setAttribute(Qt::WA_TransparentForMouseEvents, true);
            scrim->show();
            scrim->lower();   // behind the bars (but above the viewfinder)
        }
        for (QWidget *bar : { m_topBarWidget, m_bottomBarWidget }) {
            bar->setParent(this);                // float over the viewfinder
            // Bar itself is interactive (default). Transparent background so the
            // scrim's frosted glass shows through; the bar contributes no scrim
            // of its own and no dead padding (it is sized to its controls only).
            bar->setAttribute(Qt::WA_TransparentForMouseEvents, false);
            bar->setStyleSheet(QStringLiteral("background: transparent;"));
            // V32.38d: route gap taps (presses not on a child control) to
            // tap-to-focus so the full-width bar's empty areas aren't dead.
            bar->setAttribute(Qt::WA_AcceptTouchEvents, true);
            bar->removeEventFilter(this);        // avoid double-install
            bar->installEventFilter(this);
            bar->raise();
            bar->show();
        }
        // The frosted glass + edge→transparent gradient + faint tint are all
        // painted by GlassScrim itself (see GlassScrim::paintEvent); no flat
        // stylesheet gradient here, which would sit ON TOP of the glass and
        // re-introduce the hard-band look.
        // Other viewfinder-child overlays must sit ABOVE the bars where they
        // overlap (reticle/AF badge/record dot are conceptually "on the scene").
        if (m_focusReticle)   m_focusReticle->raise();
        if (m_recordIndicator) m_recordIndicator->raise();
        if (m_afBadge)        m_afBadge->raise();
        if (m_landmarkHint)   m_landmarkHint->raise();
        layoutFloatingBars();
    } else {
        // ── Back to classic stacked layout ─────────────────────────────────
        if (m_topScrim)    m_topScrim->hide();
        if (m_bottomScrim) m_bottomScrim->hide();
        for (QWidget *bar : { m_topBarWidget, m_bottomBarWidget }) {
            bar->removeEventFilter(this);       // gap-forwarding is full-bleed only
            bar->setStyleSheet(QString());      // drop the transparent bg
            bar->setAttribute(Qt::WA_TransparentForMouseEvents, false);
            bar->setParent(this);
        }
        if (root) {
            // Re-insert in the original order: top, viewfinder(already in),
            // bottom. insertWidget keeps the viewfinder's stretch slot.
            root->insertWidget(0, m_topBarWidget);
            root->addWidget(m_bottomBarWidget);
        }
        m_topBarWidget->show();
        m_bottomBarWidget->show();
    }
}

void CameraWidget::positionTopOverlays()
{
    // The top-anchored overlays (REC indicator, AF badge) are children of the
    // viewfinder, which stacks BELOW the floating bars (children of `this`). In
    // full-bleed the top bar covers y=0..barH, so an overlay at y=12 is hidden
    // behind the chrome and raise() can't help (different parent). Push the REC
    // indicator below the bar in full-bleed; restore it to the top inset in
    // classic mode (bars are in the normal layout there, nothing floats over it).
    const int topInset = Theme::SPACE_SM;   // classic resting Y (~12)
    int recY = topInset;
    if (m_fullBleed && m_topBarWidget) {
        recY = m_topBarWidget->height() + Theme::SPACE_SM;
    }
    if (m_recordIndicator) m_recordIndicator->move(topInset, recY);
}

void CameraWidget::layoutFloatingBars()
{
    if (!m_fullBleed || !m_viewfinderLabel) return;
    const int w = m_viewfinderLabel->width();
    const int h = m_viewfinderLabel->height();
    // The gradient SCRIM needs vertical room to fade from opaque (at the edge)
    // to transparent; the BAR is sized to exactly its controls and anchored to
    // the edge. The scrim is taller than the bar (bar height + gradient pad) so
    // the fade reads smoothly past the controls. Scrim is mouse-transparent, so
    // its extra height never creates a dead zone — taps there hit the viewfinder.
    const int kGradientPad = Theme::SPACE_LG;
    if (m_topBarWidget) {
        const int bh = m_topBarWidget->sizeHint().height();
        m_topBarWidget->setGeometry(0, 0, w, bh);
        if (m_topScrim) {
            m_topScrim->setGeometry(0, 0, w, bh + kGradientPad);
            m_topScrim->lower();
        }
        m_topBarWidget->raise();
    }
    if (m_bottomBarWidget) {
        const int bh = m_bottomBarWidget->sizeHint().height();
        m_bottomBarWidget->setGeometry(0, h - bh, w, bh);
        if (m_bottomScrim) {
            const int sh = bh + kGradientPad;
            m_bottomScrim->setGeometry(0, h - sh, w, sh);
            m_bottomScrim->lower();
        }
        m_bottomBarWidget->raise();
    }
    // keep scene overlays above the bars (within the viewfinder's child stack)
    if (m_focusReticle)    m_focusReticle->raise();
    if (m_recordIndicator) m_recordIndicator->raise();
    if (m_afBadge)         m_afBadge->raise();
    if (m_landmarkHint)    m_landmarkHint->raise();

    // V32.38l: push top-anchored overlays below the floating top bar (see
    // positionTopOverlays). Must also be called at each REC-indicator show site
    // because recording can start after the last layout pass.
    positionTopOverlays();
}

Camera::GlassScrim *CameraWidget::ensureTopScrim()
{
    if (!m_topScrim) {
        m_topScrim = new Camera::GlassScrim(
            Camera::GlassScrim::Edge::Top, this);
        m_topScrim->setChromaticShift(1);   // start subtle
    }
    return m_topScrim;
}

Camera::GlassScrim *CameraWidget::ensureBottomScrim()
{
    if (!m_bottomScrim) {
        m_bottomScrim = new Camera::GlassScrim(
            Camera::GlassScrim::Edge::Bottom, this);
        m_bottomScrim->setChromaticShift(1);
    }
    return m_bottomScrim;
}

void CameraWidget::renderToViewfinder(const QImage &displayImg, bool live)
{
    if (!m_viewfinderLabel || displayImg.isNull()) return;
    // V32.34+ perf: FastTransformation for substantial downscales (the source
    // is already supersampled vs the small phone viewfinder); SmoothTransform
    // only when not downscaling hard. Threshold 1.7× catches 1280×720 → 720-wide.
    const QSize srcSize = displayImg.size();
    // V32.38k HiDPI: label->size() is LOGICAL px; on a scaled display a logical-
    // size pixmap is painted soft + compositor-upscaled. Scale/crop in DEVICE px
    // (logical × dpr) and tag the pixmap dpr so it paints crisp. dpr==1 → no-op.
    const qreal dpr = m_viewfinderLabel->devicePixelRatioF();
    const QSize dstLogical = m_viewfinderLabel->size();
    const QSize dstSize(qRound(dstLogical.width()  * dpr),
                        qRound(dstLogical.height() * dpr));
    if (dstSize.isEmpty()) return;
    const bool bigDownscale =
        (srcSize.width()  >= dstSize.width()  * 17 / 10) ||
        (srcSize.height() >= dstSize.height() * 17 / 10);
    // V32.39+: a LIVE preview frame is always FastTransformation. On a HiDPI
    // phone the device-pixel destination (logical × dpr) is often as large as —
    // or larger than — the 1280-wide source, so the old "only Fast when
    // downscaling ≥1.7×" rule fell through to SmoothTransformation and ran an
    // expensive area-resample on every frame, starving the GUI thread (the
    // libcamera "slow / double-tap choke" path; gst hides it in a subprocess).
    // Smooth is imperceptible on 20–30 fps video, so live frames take the cheap
    // bilinear path; still/cached redraws keep the quality-adaptive choice.
    const Qt::TransformationMode mode = (live || bigDownscale)
        ? Qt::FastTransformation : Qt::SmoothTransformation;
    // Full-bleed fills edge-to-edge (expand + centre-crop). Otherwise we fit
    // the whole frame and, when its aspect ratio leaves dead space, fill that
    // space with a soft blurred bleed of the frame instead of hard bars — the
    // result is always a full-viewfinder pixmap, so the label background (and
    // any stale content) never shows through.
    QImage out;
    if (m_fullBleed) {
        QImage scaled = displayImg.scaled(dstSize, Qt::KeepAspectRatioByExpanding, mode);
        if (scaled.size() != dstSize) {
            const int x = qMax(0, (scaled.width()  - dstSize.width())  / 2);
            const int y = qMax(0, (scaled.height() - dstSize.height()) / 2);
            scaled = scaled.copy(x, y, qMin(dstSize.width(),  scaled.width()),
                                       qMin(dstSize.height(), scaled.height()));
        }
        out = scaled;
    } else {
        QImage fit = displayImg.scaled(dstSize, Qt::KeepAspectRatio, mode);
        out = (fit.size() == dstSize) ? fit
                                      : makeGlowBleed(displayImg, fit, dstSize);
    }
    QPixmap pm = QPixmap::fromImage(out);
    pm.setDevicePixelRatio(dpr);
    // V32.38l: setPixmap doesn't clear leftover placeholder text/stylesheet
    // (e.g. "Switching camera…" + 20px padding); reset before showing a frame.
    if (!m_viewfinderLabel->text().isEmpty()) {
        m_viewfinderLabel->setText(QString());
        m_viewfinderLabel->setStyleSheet(QStringLiteral("background: #000000;"));
    }
    m_viewfinderLabel->setPixmap(pm);
    updateScrimBackdrops(out);
}

QImage CameraWidget::makeGlowBleed(const QImage &src, const QImage &fit,
                                   const QSize &dstSize) const
{
    QImage canvas(dstSize, QImage::Format_RGB32);
    // 1) Blurred bleed backdrop. Same cheap recipe as GlassScrim: scale the
    //    source down hard (most of the "blur" is the downscale), then back up
    //    with bilinear smoothing. Expand-cover + centre-crop so the bleed has
    //    no bars of its own. ~1/12 keeps the per-frame cost tiny on the FP5.
    const int dw = qMax(1, dstSize.width()  / 12);
    const int dh = qMax(1, dstSize.height() / 12);
    QImage tiny = src.scaled(QSize(dw, dh), Qt::KeepAspectRatioByExpanding,
                             Qt::FastTransformation);
    if (tiny.size() != QSize(dw, dh)) {
        const int cx = qMax(0, (tiny.width()  - dw) / 2);
        const int cy = qMax(0, (tiny.height() - dh) / 2);
        tiny = tiny.copy(cx, cy, qMin(dw, tiny.width()), qMin(dh, tiny.height()));
    }
    QImage bleed = tiny.scaled(dstSize, Qt::IgnoreAspectRatio, Qt::SmoothTransformation);

    QPainter p(&canvas);
    p.drawImage(0, 0, bleed);
    // Dim the bleed a touch so the crisp, centred frame reads as the subject.
    p.fillRect(canvas.rect(), QColor(0, 0, 0, 72));
    // 2) Centre the aspect-fit frame on top.
    const int x = (dstSize.width()  - fit.width())  / 2;
    const int y = (dstSize.height() - fit.height()) / 2;
    p.drawImage(x, y, fit);
    p.end();
    return canvas;
}

void CameraWidget::updateScrimBackdrops(const QImage &viewfinderSized)
{
    if (!m_fullBleed || viewfinderSized.isNull()) return;
    // V32.38k: `viewfinderSized` is now in DEVICE pixels (logical size × dpr),
    // but scrim geometry() is LOGICAL pixels. Map each scrim's logical rect into
    // device-pixel coordinates before slicing. The GlassScrim downscales its
    // backdrop internally and scales it to its own widget size on paint, so a
    // device-pixel strip is fine (and actually higher quality) — only the crop
    // RECT needs the dpr scale to land on the right region.
    const qreal dpr = m_viewfinderLabel
                          ? m_viewfinderLabel->devicePixelRatioF() : 1.0;
    for (Camera::GlassScrim *scrim : { m_topScrim, m_bottomScrim }) {
        if (!scrim || !scrim->isVisible() || scrim->size().isEmpty()) continue;
        const QRect gl = scrim->geometry();           // logical
        const QRect g(qRound(gl.x() * dpr), qRound(gl.y() * dpr),
                      qRound(gl.width() * dpr), qRound(gl.height() * dpr)); // device
        const QRect clip = g.intersected(QRect(QPoint(0, 0), viewfinderSized.size()));
        if (clip.isEmpty()) continue;
        QImage strip = viewfinderSized.copy(clip);
        if (strip.size() != g.size()) {
            QImage full(g.size(), QImage::Format_ARGB32);
            full.fill(Qt::transparent);
            QPainter pp(&full);
            pp.drawImage(clip.topLeft() - g.topLeft(), strip);
            pp.end();
            strip = full;
        }
        scrim->setBackdrop(strip);
    }
}

void CameraWidget::onPipFramePreview(const QImage &frame)
{
    // V32.34+: log the first PIP frame arrival so we can distinguish
    // "PIP backend isn't producing frames" from "PIP backend works but
    // overlay paint is broken." Logged exactly once per session via
    // a static guard.
    static bool s_pipFirstFrameLogged = false;
    if (!s_pipFirstFrameLogged) {
        qCInfo(lcCamera) << "PIP first frame:" << frame.size()
                         << "format:" << static_cast<int>(frame.format())
                         << "overlay-visible:" << (m_pipOverlayLabel && m_pipOverlayLabel->isVisible())
                         << "overlay-size:" << (m_pipOverlayLabel ? m_pipOverlayLabel->size() : QSize());
        s_pipFirstFrameLogged = true;
    }
    if (!m_pipOverlayLabel || !m_pipOverlayLabel->isVisible()) return;
    // V32.38+: PIP is no longer always the front camera — "Camera + USB PIP"
    // runs an external scene camera in the overlay, which must NOT be mirrored
    // (only user-facing / selfie sources are). startPip() records the correct
    // choice in m_pipMirror; honour it here. The two functions stay coupled:
    // whoever chooses the PIP device also sets m_pipMirror.
    //
    // Apply user rotation FIRST so PIP orientation matches the main
    // viewfinder's; only then (maybe) mirror. Order matters — mirror-then-
    // rotate differs from rotate-then-mirror for 90/270°.
    QImage img = applyUserRotation(frame);
    if (m_pipMirror)
        img = img.mirrored(true, false);
    // KeepAspectRatioByExpanding (vs the main viewfinder's KeepAspectRatio)
    // fills the PIP rectangle completely — we'd rather crop the edges
    // than leave letterbox bars in a 120×160px tile where every pixel
    // counts visually.
    // V32.38k HiDPI: scale to DEVICE px and tag the pixmap dpr, same reasoning
    // as the main viewfinder — a logical-size pixmap is soft on scaled displays,
    // and in a tiny PIP tile every pixel of sharpness counts.
    const qreal pipDpr = m_pipOverlayLabel->devicePixelRatioF();
    const QSize pipDev(qRound(m_pipOverlayLabel->width()  * pipDpr),
                       qRound(m_pipOverlayLabel->height() * pipDpr));
    const QImage scaled = img.scaled(pipDev,
                                      Qt::KeepAspectRatioByExpanding,
                                      Qt::SmoothTransformation);
    QPixmap pipPm = QPixmap::fromImage(scaled);
    pipPm.setDevicePixelRatio(pipDpr);
    m_pipOverlayLabel->setPixmap(pipPm);
}

// ─── Lifecycle ───────────────────────────────────────────────────────────────

void CameraWidget::startCamera()
{
    selectBackend();
    if (!m_backend) {
        showStartingPlaceholder();
        // Retry shortly — virtual cams may appear after camera-bridge spawns
        QTimer::singleShot(800, this, [this]() {
            selectBackend();
            if (m_backend) startCamera();
        });
        return;
    }

    refreshDeviceSelector();

    // V32.32 (revision): set initial mirror state via CameraRegistry.
    // onSelectorChanged usually handles this, but on first startup the
    // selector starts already-populated without firing changed.
    // V32.42+: same name-rule fallback as onSelectorChanged (mirrorForDevice),
    // so a USB webcam selected at startup is mirrored too.
    const QString id0 = m_cameraSelector->currentData().toString();
    m_mirrorPreview = Camera::CameraRegistry::instance()
                          .mirrorForDevice(id0, m_cameraSelector->currentText());

    // Resume if already active
    if (m_backend->isActive()) return;

    // V32.35+ LOG-ORACLE FIX: resume the SELECTED device, not the default.
    // See resumeSelectedCamera() for the full rationale — this was reset-to-rear
    // on every page return / post-recording resume.
    if (!resumeSelectedCamera()) {
        showNoCameraPlaceholder();
    }
}

// V32.35+: resume capture on the device the SELECTOR currently points at,
// falling back to the backend default only when the selector holds no real
// device (PIP sentinel or empty). Centralizes the fix for a class of
// "resume loses the user's camera choice" bugs: startCamera (page return) and
// every post-recording / devices-appeared resume path used the no-arg start(),
// which resolves to defaultDeviceId()=primaryRear — so a user on the FRONT
// camera who navigated away, or who recorded a video, came back to the REAR
// camera. The selector is this widget's source of truth for the chosen device.
// Returns the backend start() result.
bool CameraWidget::resumeSelectedCamera()
{
    if (!m_backend) return false;
    const QString id = m_cameraSelector
        ? m_cameraSelector->currentData().toString() : QString();
    const bool realDevice =
        !id.isEmpty() && id != QString::fromLatin1(kPipSentinel)
                      && id != QString::fromLatin1(kPipExtSentinel);
    return realDevice ? m_backend->start(id) : m_backend->start();
}

void CameraWidget::stopCamera()
{
    if (m_backend) m_backend->stop();
    stopPip();  // V32.33: ensure PIP teardown if active
    // V32.34+: same defensive clear as stopPip — see comment there.
    // Covers the case where the main backend never reached active state
    // and so didn't fire stopped().
    if (!m_inUseDeviceId.isEmpty()) {
        Camera::CameraHealthService::instance()
            .setDeviceInUse(m_inUseDeviceId, false);
        m_inUseDeviceId.clear();
    }
}

void CameraWidget::showEvent(QShowEvent *e)
{
    QWidget::showEvent(e);
    m_widgetVisible = true;
    updateProcessingState();

    // V32.37+ fullscreen redesign: once we're shown the viewfinder has real
    // geometry, so (re)position the floating control bars. Construction-time
    // setFullBleed couldn't place them (zero size then); this is the first
    // correct placement, and the viewfinder-resize eventFilter keeps them pinned
    // afterwards.
    if (m_fullBleed) layoutFloatingBars();

    // V32.26: debounce — see header for rationale. Cancel any pending
    // stop (we're now visible, contradicting the queued stop), and
    // queue a start. If a hide arrives within 250ms it'll cancel this
    // start instead. The first call lazy-creates the timer.
    if (!m_lifecycleTimer) {
        m_lifecycleTimer = new QTimer(this);
        m_lifecycleTimer->setSingleShot(true);
        m_lifecycleTimer->setInterval(250); // matches our typical
                                              // swipe-animation duration; the
                                              // user can't visit-and-leave the
                                              // camera page in less than this
                                              // unless they're swiping past
        connect(m_lifecycleTimer, &QTimer::timeout, this, [this]() {
            if (m_pendingStart) {
                m_pendingStart = false;
                startCamera(); // idempotent
            }
            if (m_pendingStop) {
                m_pendingStop = false;
                stopCamera();
            }
        });
    }
    m_pendingStop  = false;  // cancel any queued stop
    m_pendingStart = true;
    m_lifecycleTimer->start();

    // V32.34+ landmark scoring: kick off the periodic scorer now that
    // the camera page is visible. Tied to lifecycle, NOT to the backend
    // active state — the backend takes ~250ms after showEvent to start
    // delivering frames, and runLandmarkScoreTick's own guards (no frame
    // yet → bail) handle that gracefully. Stopping on hide saves the
    // 1Hz wakeup + worker dispatch when the user is on another page.
    if (m_landmarkScoreTimer && !m_landmarkScoreTimer->isActive()) {
        m_landmarkScoreTimer->start();
    }
}

void CameraWidget::hideEvent(QHideEvent *e)
{
    QWidget::hideEvent(e);
    m_widgetVisible = false;
    updateProcessingState();

    // V32.26: debounce — cancel any pending start, queue a stop.
    if (!m_lifecycleTimer) {
        // Defensive: hideEvent shouldn't fire before any showEvent in
        // practice, but if it does we still need a timer to schedule
        // the stop properly.
        m_lifecycleTimer = new QTimer(this);
        m_lifecycleTimer->setSingleShot(true);
        m_lifecycleTimer->setInterval(250);
        connect(m_lifecycleTimer, &QTimer::timeout, this, [this]() {
            if (m_pendingStart) { m_pendingStart = false; startCamera(); }
            if (m_pendingStop)  { m_pendingStop  = false; stopCamera();  }
        });
    }
    m_pendingStart = false;  // cancel any queued start
    m_pendingStop  = true;
    m_lifecycleTimer->start();

    // V32.34+ landmark scoring: stop the scorer + hide any visible pill
    // when the user leaves the camera page. The pill is parented to the
    // viewfinder so it auto-hides with us, but we also tear down its
    // captured-frame snapshot to free the QImage's backing memory.
    if (m_landmarkScoreTimer && m_landmarkScoreTimer->isActive()) {
        m_landmarkScoreTimer->stop();
    }
    if (m_landmarkHintAutoHide && m_landmarkHintAutoHide->isActive()) {
        m_landmarkHintAutoHide->stop();
    }
    if (m_landmarkHint) m_landmarkHint->hide();
    m_landmarkHintFrame = QImage();
}

// ─── Frame handling ──────────────────────────────────────────────────────────

void CameraWidget::onPreviewFrame(const QImage &frame)
{
    if (!shouldProcessFrames()) {
        ++m_frameDropCount;
        return;
    }
    ++m_frameProcessedCount;

    // V32.32: lazy VCM bring-up. The DW9719 voice-coil motor only
    // responds on i2c after the AFVDD_2P8 rail has powered up, which
    // happens when the sensor starts streaming. We schedule the open
    // 1.5s after the first frame arrives — that's enough for the rail
    // to stabilise across all FP5 builds we've tested. The first
    // open() attempt may succeed directly (driver was bound at boot)
    // or need a rebind (driver probed too early). Failures are
    // silent — the camera works without AF.
    //
    // V32.32 (revision): gate on CameraRegistry.supportsFocus. Front
    // and ultrawide have no VCM — opening the subdev there is wasted
    // ioctls. We check on the FIRST frame and re-check every time the
    // camera changes (selector handler resets m_vcmOpenTimer to null).
    if (!m_vcmOpenTimer) {
        const QString currentId = m_cameraSelector
                                      ? m_cameraSelector->currentData().toString()
                                      : QString();
        const auto info = Camera::CameraRegistry::instance().byId(currentId);
        const bool hasFocus = info && info->supportsFocus;
        if (hasFocus) {
            m_vcmOpenTimer = new QTimer(this);
            m_vcmOpenTimer->setSingleShot(true);
            m_vcmOpenTimer->setInterval(1500);
            connect(m_vcmOpenTimer, &QTimer::timeout, this, [this]() {
                // V32.32 (revision): VCM open + potential rebind moved
                // off the GUI thread. The open path does ~40 sysfs file
                // reads (subdev discovery scan); the rebind path does
                // two sysfs writes + 600ms of explicit sleep. None of
                // that belongs on the input-handling thread.
                //
                // Construct the VcmController on the GUI thread so the
                // unique_ptr ownership is clear, then hand the raw
                // pointer to the worker for the slow ioctl/syscall
                // dance. The pointer is safe because:
                //   - Teardown (onSelectorChanged) waits for in-flight
                //     focus work via the m_focusInProgress flag (see
                //     comments in onSelectorChanged).
                //   - The open task does NOT mutate the GUI thread's
                //     view of m_vcm; it only manipulates the fd
                //     inside the VcmController (which we created in
                //     advance and the GUI thread won't read during
                //     the worker's run because we set
                //     m_focusInProgress=true around the open task).
                if (!m_vcm) {
                    m_vcm = std::make_unique<Camera::VcmController>();
                }
                Camera::VcmController *vcm = m_vcm.get();
                // V32.38j RACE FIX: acquire the focus flag ATOMICALLY, don't just
                // store true. A focus sweep (runFastFocus) drives the same VCM
                // fd from its own worker thread and owns m_focusInProgress via
                // compare_exchange. If a sweep is already in flight, a plain
                // `m_focusInProgress = true` here would (a) be a no-op on the
                // flag yet (b) still launch THIS open worker on the SAME fd
                // concurrently — two threads ioctl-ing one VCM — and then
                // whichever finished first would clear the flag while the other
                // worker was still running. Only launch the open if we win the
                // flag; if a sweep already holds it, the VCM will be opened
                // lazily on first use anyway, so skipping is safe.
                bool expected = false;
                if (m_focusInProgress.compare_exchange_strong(expected, true)) {
                    auto future = QtConcurrent::run([vcm]() {
                        if (!vcm->open()) {
                            vcm->rebindAfterStreamStart();
                        }
                    });
                    auto *watcher = new QFutureWatcher<void>(this);
                    connect(watcher, &QFutureWatcher<void>::finished,
                            this, [this, watcher]() {
                        watcher->deleteLater();
                        m_focusInProgress = false;
                        // V32.34+ AF UI: VCM open finished — badge transitions
                        // from "AF …" (probing) to "AF" (ready) if open
                        // succeeded, or stays greyed if open failed.
                        updateAfBadge();
                    });
                    watcher->setFuture(future);
                } else {
                    // A sweep already owns the VCM; it will open lazily. Just
                    // refresh the badge to reflect current state.
                    updateAfBadge();
                }
            });
            m_vcmOpenTimer->start();
        }
    }

    // Apply user rotation override BEFORE display + scan emission so all
    // downstream consumers see the user-corrected orientation.
    const QImage oriented = applyUserRotation(frame);

    // V32.34+ DEFENSIVE: sanity-check the incoming frame's byte layout at the UI
    // boundary. If the backend ever hands us a QImage whose stride is below the
    // tight minimum or whose buffer is truncated, painting it yields exactly the
    // tiling/repeat/tint artifacts — so we LOG it (once) rather than silently
    // rendering garbage. This is the diagnostic that distinguishes "the UI is
    // wrong" (it isn't — this path is stride-safe) from "the backend handed us a
    // malformed frame" (the real cause, in the fdsink→QImage stride math).
    // Mirrors the tested snapclone-videosrc::validate_preview_frame logic.
    if (!m_previewFrameValidated && !frame.isNull()) {
        const int w   = frame.width();
        const int h   = frame.height();
        const int bpl = static_cast<int>(frame.bytesPerLine());
        const int bpp = frame.depth() / 8;
        const qint64 total = static_cast<qint64>(frame.sizeInBytes());
        const int minStride = w * bpp;
        if (w <= 0 || h <= 0 || bpp <= 0) {
            qCWarning(lcCamera) << "Preview frame BAD GEOMETRY:" << w << "x" << h
                                << "depth" << frame.depth();
        } else if (bpl < minStride) {
            qCWarning(lcCamera) << "Preview frame STRIDE TOO SMALL: bytesPerLine"
                                << bpl << "< tight" << minStride
                                << "(" << w << "x" << h << ") — preview will skew;"
                                << "backend QImage stride is wrong, not the UI.";
        } else if (total < static_cast<qint64>(bpl) * h) {
            qCWarning(lcCamera) << "Preview frame TRUNCATED: sizeInBytes" << total
                                << "< stride×h" << (static_cast<qint64>(bpl) * h);
        } else {
            qCInfo(lcCamera) << "Preview frame layout OK:" << w << "x" << h
                             << "bpl" << bpl << "bytes" << total
                             << "— UI display path is clean.";
        }
        m_previewFrameValidated = true;  // log once per session, not per frame
    }

    if (m_viewfinderLabel) {
        QImage displayImg = oriented;
        // V32.32: mirror the LIVE preview when using the front-facing
        // camera. Selfies feel natural when your motions match (right
        // hand on right side of frame). This applies to display ONLY —
        // the captured photo (onFrameCaptured below) and the scan
        // frame and the previewFrameAvailable emission all use the
        // UN-mirrored `oriented`.
        if (m_mirrorPreview) {
            displayImg = displayImg.mirrored(true, false); // horizontal only
        }
        renderToViewfinder(displayImg, /*live=*/true);   // V32.39+: cheap FastTransform for moving preview
    }

    // QR scan mode emits at ~8fps (every 4th frame from a 30fps stream)
    if (m_scanMode) {
        ++m_scanThrottle;
        if (m_scanThrottle % 4 == 0) emit scanFrameAvailable(oriented);
    }

    // V32.30: re-emit the oriented frame for ambient-camera-mode and
    // any other future consumer. ~30fps; consumers are responsible for
    // their own throttle. We pass the oriented variant (post user-
    // rotation) so listeners see what the user sees.
    emit previewFrameAvailable(oriented);

    // V32.32 (revision): cache the latest frame for the CDAF sweep,
    // which now runs on a WORKER thread. Concurrent access to a non-
    // atomic QImage handle is UB, so writes are mutex-guarded. The
    // QImage copy under lock is O(1) (implicit-shared refcount bump).
    {
        QMutexLocker lock(&m_frameMutex);
        m_latestFrame = oriented;
    }
}

void CameraWidget::onFrameCaptured(const QImage &frame)
{
    // Apply user rotation BEFORE handing to composer — snap recipient sees
    // the same orientation the sender saw in the viewfinder.
    const QImage oriented = applyUserRotation(frame);
    qCDebug(lcCamera) << "Image captured:" << oriented.size()
                      << "(rotation applied:" << m_userRotation << "deg)";

    // V32.32 (Phase 1): opportunistically archive a face crop into the
    // local Face::Library so future selfies can use this as a delta
    // compression reference. Only kicks in when:
    //   - AppController was attached (m_controller != nullptr)
    //   - Library is enabled (user opted in via Settings)
    //   - Detector is loaded (model file exists)
    //   - This is a selfie (front camera — m_mirrorPreview signals that)
    //
    // Runs on a worker thread so the composer dialog opens immediately;
    // archiving is best-effort and never blocks the user-visible flow.
    // The composer opens first (line below) regardless of archive status.
    if (m_mirrorPreview && m_controller) {
        archiveSelfieFaceIfAny(oriented);
    }

    auto *composer = new SnapComposerDialog(oriented, this);
    composer->setAttribute(Qt::WA_DeleteOnClose);
    connect(composer, &SnapComposerDialog::snapReady,
            this, [this](const QImage &img, int dur) {
                emit snapCaptured(img, dur);
            });
    composer->open();
}

// V32.32 (Phase 1): opportunistic selfie face crop archiving.
//
// Detection + crop + encode + hash + library insert all happen on a
// worker thread via QtConcurrent. The capture path returns immediately;
// the composer dialog opens with no perceptible delay. If detection
// fails, finds no face, or the library is disabled / unavailable, the
// whole operation is silently skipped — there's no user-visible
// success indicator and no error path. The settings page counter
// ticks up over time as crops accumulate.
//
// Why the largest detection rather than highest confidence: in a
// selfie the user is almost always the closest/biggest face. Group
// selfies are an edge case where this might pick a friend instead of
// the user; that's OK for Phase 1 (the friend's contact label isn't
// known anyway, and a "self" labeled crop of someone else mostly
// hurts the library's matching quality, not safety).
void CameraWidget::archiveSelfieFaceIfAny(const QImage &frame)
{
    if (!m_controller) return;
    Face::Library  *lib = m_controller->faceLibrary();
    Face::Detector *det = m_controller->faceDetector();
    if (!lib || !det) return;
    if (!lib->isAvailable() || !det->isAvailable()) return;
    if (!lib->isEnabled()) return;

    // Convert to a tightly-packed RGB888 buffer for the detector.
    // QImage::convertToFormat returns a deep copy with the requested
    // format; the result has bytesPerLine == width*3 for Format_RGB888
    // on common platforms but we shouldn't rely on that — we copy
    // line-by-line into a tight buffer below.
    const QImage rgb = frame.convertToFormat(QImage::Format_RGB888);
    if (rgb.isNull() || rgb.width() < 32 || rgb.height() < 32) return;

    const int w = rgb.width();
    const int h = rgb.height();
    QByteArray tightBytes;
    tightBytes.resize(w * h * 3);
    for (int y = 0; y < h; ++y) {
        std::memcpy(tightBytes.data() + y * w * 3,
                    rgb.constScanLine(y), w * 3);
    }

    // Run detection + storage on a worker thread. Capture detector
    // and library pointers by raw value — safe because they're owned
    // by AppController, which outlives CameraWidget (AppController
    // lives in MainWindow's parent scope). We don't capture
    // m_controller itself because the worker never needs to follow
    // back through it.
    auto *detectorPtr = det;
    auto *libraryPtr  = lib;

    // We move both the QImage and the tight RGB bytes into the worker
    // so the GUI thread is done immediately. QImage uses implicit
    // sharing — the move is essentially a refcount bump.
    QImage frameCopy = rgb;

    QtConcurrent::run([detectorPtr, libraryPtr, frameCopy = std::move(frameCopy),
                       tightBytes = std::move(tightBytes), w, h]() {
        // 1. Detect faces.
        const auto detections = detectorPtr->detect(
            tightBytes, static_cast<std::uint32_t>(w),
            static_cast<std::uint32_t>(h), /*maxResults=*/8);
        if (detections.empty()) return;

        // 2. Pick the largest face (most likely the user in a selfie).
        std::size_t bestIdx = 0;
        float bestArea = 0.0f;
        for (std::size_t i = 0; i < detections.size(); ++i) {
            const float a = detections[i].area();
            if (a > bestArea) { bestArea = a; bestIdx = i; }
        }
        const auto &best = detections[bestIdx];

        // 3. Crop the QImage to the bbox plus a small margin so the
        //    stored crop has context around the face (helps future
        //    similarity matching on slightly different framings).
        constexpr float kMarginFactor = 0.15f;
        const float mx = best.width() * kMarginFactor;
        const float my = best.height() * kMarginFactor;
        QRect cropRect(
            static_cast<int>(std::max(0.0f, best.xMin - mx)),
            static_cast<int>(std::max(0.0f, best.yMin - my)),
            static_cast<int>(std::min(static_cast<float>(w),
                                       best.xMax + mx) -
                              std::max(0.0f, best.xMin - mx)),
            static_cast<int>(std::min(static_cast<float>(h),
                                       best.yMax + my) -
                              std::max(0.0f, best.yMin - my))
        );
        cropRect = cropRect.intersected(QRect(0, 0, w, h));
        if (cropRect.width() < 32 || cropRect.height() < 32) return;
        QImage cropImg = frameCopy.copy(cropRect);

        // 4. Encode as JPEG q85. Standard quality/size tradeoff for
        //    library storage; gives ~5-15KB for typical face crops.
        QByteArray jpegBytes;
        QBuffer buf(&jpegBytes);
        buf.open(QIODevice::WriteOnly);
        if (!cropImg.save(&buf, "JPEG", 85)) {
            return;
        }
        buf.close();
        if (jpegBytes.isEmpty()) return;

        // 5. Compute SHA-256 of the JPEG bytes for de-duplication and
        //    future reference-negotiation use.
        QCryptographicHash hasher(QCryptographicHash::Sha256);
        hasher.addData(jpegBytes);
        const QByteArray digest = hasher.result();
        std::array<std::uint8_t, 32> hashArr;
        std::memcpy(hashArr.data(), digest.constData(),
                    std::min<int>(32, digest.size()));

        // 6. Hand to the library. The library is internally
        //    thread-safe (Mutex-guarded), so direct call from the
        //    worker is fine — no need to bounce back to the GUI thread.
        const auto now = QDateTime::currentMSecsSinceEpoch();
        const float quality = best.confidence;  // pass detection confidence
                                                  // through as quality hint
        const auto status = libraryPtr->add(
            Face::LabelKind::SelfFace,
            QString(),
            hashArr,
            jpegBytes,
            static_cast<std::uint32_t>(cropImg.width()),
            static_cast<std::uint32_t>(cropImg.height()),
            now,
            quality);
        if (status != Face::Status::Ok) {
            // Soft-fail: log at debug level, don't surface to user.
            // ConsentDisabled is the most common case (race between
            // the toggle being flipped and this worker completing) —
            // treat as expected.
            qCDebug(lcCamera)
                << "Face library add returned non-Ok status:"
                << static_cast<int>(status);
        }
    });
}

void CameraWidget::onBackendError(const QString &msg)
{
    qCWarning(lcCamera) << "Backend error:" << msg;
    // V32.34+: release the in-use marker on health service so probes
    // and other consumers know the device is no longer being held.
    // Without this, m_inUseDeviceId stays set forever after an error,
    // and CameraHealthService::isDeviceInUse returns true permanently,
    // suppressing legitimate probes for that device.
    if (!m_inUseDeviceId.isEmpty()) {
        Camera::CameraHealthService::instance()
            .setDeviceInUse(m_inUseDeviceId, false);
        m_inUseDeviceId.clear();
    }
    // V32.32: surface the error in the viewfinder so the user knows
    // something failed instead of seeing "Starting camera…" forever.
    // We don't auto-clear — the next successful frame will overwrite
    // this label via setPixmap. If the camera permanently can't start,
    // the user sees the actual error message rather than a placeholder.
    if (m_viewfinderLabel) {
        m_viewfinderLabel->setPixmap(QPixmap());  // clear stale frame
        m_viewfinderLabel->setText(tr("⚠ Camera error\n\n%1\n\n"
                                       "Try switching cameras or check logs.")
                                       .arg(msg));
        m_viewfinderLabel->setStyleSheet(QStringLiteral(
            "background: #1a0e0e; color: #ff9a9a;"
            "font-size: 13px; padding: 20px;"));
    }
}

void CameraWidget::onBackendDeviceListChanged()
{
    refreshDeviceSelector();
    // If we're visible but the backend never managed to start (e.g. virtual
    // cams just appeared after camera-bridge spawned), try start now.
    if (m_backend && !m_backend->isActive() && m_widgetVisible) {
        qCInfo(lcCamera) << "Devices appeared — attempting backend start";
        resumeSelectedCamera();   // V32.35+: selected device, not default rear
    }
}

// ─── Capture ─────────────────────────────────────────────────────────────────

void CameraWidget::onCaptureClicked()
{
    if (!m_backend || !m_backend->isActive()) {
        // Two distinct cases here:
        //   1. No backend at all (camera never started)
        //   2. Backend paused because the recorder is using v4l2 exclusively
        // The second is much more common in normal use (user taps photo
        // during/right-after a video recording). Distinguish in the log
        // so future investigations don't conflate the two.
        if (m_viewfinderPausedForRecording) {
            qCWarning(lcCamera) << "Capture ignored — camera is held by the "
                                   "recorder. Wait for recording to finish.";
        } else {
            qCWarning(lcCamera) << "Capture requested but no active backend";
        }
        return;
    }

    // ── Animated shutter feedback ─────────────────────────────────────────
    // Three concurrent animations give it a physical "click" feel:
    //   1. White flash overlay across the viewfinder (fade in then out)
    //   2. Subtle scale-down on the shutter button (1.0 → 0.92 → 1.0)
    //   3. The actual capture is requested at the same moment as the flash
    //      starts, so the photo includes the moment the user pressed.

    // 1. Flash overlay: a transparent QWidget positioned over the viewfinder
    //    with an opacity effect we animate. WA_TransparentForMouseEvents so
    //    the flash never blocks subsequent input.
    if (m_viewfinderLabel) {
        auto *flash = new QWidget(m_viewfinderLabel);
        flash->setStyleSheet(QStringLiteral("background: white;"));
        flash->setAttribute(Qt::WA_TransparentForMouseEvents, true);
        flash->setGeometry(m_viewfinderLabel->rect());
        flash->show();

        auto *fx = new QGraphicsOpacityEffect(flash);
        fx->setOpacity(0.0);
        flash->setGraphicsEffect(fx);

        // Fade in
        auto *fadeIn = new QPropertyAnimation(fx, "opacity", flash);
        fadeIn->setDuration(80);
        fadeIn->setStartValue(0.0);
        fadeIn->setEndValue(0.65);
        fadeIn->setEasingCurve(QEasingCurve::OutQuad);

        // Hold then fade out
        auto *fadeOut = new QPropertyAnimation(fx, "opacity", flash);
        fadeOut->setDuration(200);
        fadeOut->setStartValue(0.65);
        fadeOut->setEndValue(0.0);
        fadeOut->setEasingCurve(QEasingCurve::OutCubic);

        // Sequence: fadeIn → 60ms hold → fadeOut → delete the overlay
        connect(fadeIn, &QPropertyAnimation::finished, flash, [fadeOut]() {
            QTimer::singleShot(60, fadeOut, [fadeOut]() {
                fadeOut->start(QAbstractAnimation::DeleteWhenStopped);
            });
        });
        connect(fadeOut, &QPropertyAnimation::finished, flash, &QWidget::deleteLater);
        fadeIn->start(QAbstractAnimation::DeleteWhenStopped);
    }

    // 2. Shutter button squish — scale via QPropertyAnimation on a custom
    //    "scale" property would require Q_PROPERTY plumbing on ShutterButton.
    //    Simpler: animate its geometry inward then back. The button is a
    //    fixed 78×78 widget, we shrink it by 6px on each side then restore.
    if (m_shutter) {
        const QRect orig = m_shutter->geometry();
        const int   delta = 4;
        const QRect shrunk(orig.x() + delta, orig.y() + delta,
                           orig.width() - 2*delta, orig.height() - 2*delta);
        auto *anim = new QPropertyAnimation(m_shutter, "geometry", m_shutter);
        anim->setDuration(180);
        anim->setKeyValueAt(0.0, orig);
        anim->setKeyValueAt(0.5, shrunk);
        anim->setKeyValueAt(1.0, orig);
        anim->setEasingCurve(QEasingCurve::OutQuad);
        anim->start(QAbstractAnimation::DeleteWhenStopped);
    }

    // Trigger the actual capture concurrent with the visual feedback.
    m_backend->requestCapture();
}

void CameraWidget::onTextOnlyClicked()
{
    auto *composer = new SnapComposerDialog(this);
    composer->setAttribute(Qt::WA_DeleteOnClose);
    connect(composer, &SnapComposerDialog::snapReady,
            this, [this](const QImage &img, int dur) {
                emit snapCaptured(img, dur);
            });
    composer->open();
}

// ─── Recipient indicator ─────────────────────────────────────────────────────

void CameraWidget::setRecipient(const QString &displayName)
{
    if (!m_recipientLabel) return;
    if (displayName.isEmpty()) {
        m_recipientLabel->setText(tr("Shoot then choose recipient"));
        m_recipientLabel->setStyleSheet(
            QStringLiteral("color: %1;").arg(QLatin1String(Theme::TEXT_SECONDARY())));
    } else {
        m_recipientLabel->setText(tr("To: %1").arg(displayName));
        m_recipientLabel->setStyleSheet(
            QStringLiteral("color: %1; font-weight: 600;")
                .arg(QLatin1String(Theme::ACCENT())));
    }

    if (m_recipientBanner) {
        if (displayName.isEmpty()) {
            m_recipientBanner->setText(tr("Shoot then choose recipient"));
            m_recipientBanner->setStyleSheet(QStringLiteral(
                "background: rgba(0,0,0,0.55); color: rgba(255,255,255,0.85);"
                "font-size: 13px; font-weight: 500; padding: 6px 14px; border-radius: 0;"));
        } else {
            m_recipientBanner->setText(tr("To: ") + displayName);
            m_recipientBanner->setStyleSheet(QStringLiteral(
                "background: rgba(0,0,0,0.55); color: rgba(255,255,255,0.90);"
                "font-size: 13px; font-weight: 500; padding: 6px 14px; border-radius: 0;"));
        }
        m_recipientBanner->setGeometry(0, 0, width(), 32);
        m_recipientBanner->raise();
    }
}

// ─── QR scan mode ────────────────────────────────────────────────────────────

void CameraWidget::setScanMode(bool active)
{
    m_scanMode = active;
    m_scanThrottle = 0;
    qCInfo(lcCamera) << "Scan mode:" << (active ? "ON" : "OFF");
}

// ─── Placeholders ────────────────────────────────────────────────────────────

void CameraWidget::showStartingPlaceholder()
{
    if (!m_viewfinderLabel) return;
    m_viewfinderLabel->clear();
    m_viewfinderLabel->setText(tr("Starting camera…\n\nWaiting for libcamera bridge."));
    m_viewfinderLabel->setStyleSheet(QStringLiteral(
        "background: %1; color: %2; font-size: 14px;"
        " border-radius: 12px; qproperty-alignment: AlignCenter;")
        .arg(QLatin1String(Theme::BG_DEEP()))
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
}

void CameraWidget::showNoCameraPlaceholder()
{
    if (!m_viewfinderLabel) return;
    m_viewfinderLabel->clear();
    m_viewfinderLabel->setText(tr("No camera detected\n\nPlug in a USB webcam or check permissions."));
    m_viewfinderLabel->setStyleSheet(QStringLiteral(
        "background: %1; color: %2; font-size: 14px;"
        " border-radius: 12px; qproperty-alignment: AlignCenter;")
        .arg(QLatin1String(Theme::BG_DEEP()))
        .arg(QLatin1String(Theme::TEXT_DISABLED())));
}

// ─── Battery / lifecycle ─────────────────────────────────────────────────────

bool CameraWidget::shouldProcessFrames() const
{
    // V32.38r: app-inactive no longer freezes the preview INSTANTLY. On
    // desktop/Plasma, ApplicationActive flips false for transient reasons
    // (notifications, focus changes) while the camera page stays fully visible —
    // freezing the viewfinder on every such blip is wrong (sc-camdiag proved the
    // capture pipeline is healthy; the freeze was purely this gate). Mirror of
    // snapclone_videosrc::frame_gate_debounced (tested): ride through a brief
    // inactive window if still visible + screen-on; genuine backgrounding (grace
    // expired) and screen-off / offscreen still pause. On the phone, real
    // backgrounding keeps app_active false → grace expires → pauses correctly.
    if (!m_appActive) {
        // screen-off and offscreen are real pauses regardless of the grace blip.
        if (m_screenOff) return false;
        if (!m_widgetVisible && !m_scanMode) return false;
        const qint64 sinceMs = m_inactiveSinceMs > 0
            ? (QDateTime::currentMSecsSinceEpoch() - m_inactiveSinceMs)
            : kInactiveGraceMs + 1;   // unknown → treat as expired
        if (sinceMs <= kInactiveGraceMs) {
            return true;   // within grace: keep the viewfinder live
        }
        return false;      // genuinely backgrounded
    }
    // screen-off wins over everything once active (battery: nothing to show).
    if (m_screenOff) return false;
    // Scan mode (QR) needs frames even when CameraWidget itself is offscreen.
    if (m_scanMode) return true;
    // Default: only process when the widget is visible to save CPU/battery.
    return m_widgetVisible;
}

void CameraWidget::updateProcessingState()
{
    static bool wasProcessing = true;
    const bool now = shouldProcessFrames();
    if (now != wasProcessing) {
        qCInfo(lcCamera) << "Frame processing"
                         << (now ? "RESUMED" : "PAUSED")
                         << "(visible:" << m_widgetVisible
                         << ", appActive:" << m_appActive
                         << ", processed:" << m_frameProcessedCount
                         << ", dropped:" << m_frameDropCount << ")";
        wasProcessing = now;
    }
}


// ─── User rotation override ──────────────────────────────────────────────────

void CameraWidget::onRotateClicked()
{
    m_userRotation = (m_userRotation + 90) % 360;
    qCInfo(lcCamera) << "User rotation set to" << m_userRotation << "degrees";

    // Persist so the choice survives restart
    QSettings s;
    s.setValue(QStringLiteral("camera/userRotation"), m_userRotation);

    // Update the button tooltip so the next tap's effect is discoverable
    if (m_rotateButton) {
        m_rotateButton->setToolTip(
            tr("Rotation: %1° (tap to cycle to %2°)")
                .arg(m_userRotation)
                .arg((m_userRotation + 90) % 360));
    }

    // Refresh the viewfinder immediately if we have a recent frame —
    // don't wait for the next frame to arrive.
    if (m_backend && !m_backend->lastFrame().isNull())
        onPreviewFrame(m_backend->lastFrame());
}

QImage CameraWidget::applyUserRotation(const QImage &src) const
{
    if (m_userRotation == 0 || src.isNull()) return src;
    QTransform t;
    t.rotate(m_userRotation);
    // V32.39+ PERF: camera userRotation is a mounting correction — always a
    // multiple of 90°. For an exact 90/180/270 rotation every destination pixel
    // maps to exactly one source pixel, so nearest-neighbour (FastTransformation)
    // yields BIT-IDENTICAL output to bilinear (SmoothTransformation) while
    // skipping the per-pixel interpolation work — a free win on the per-frame
    // preview path. Smooth is kept only for the (unexpected) non-90° case.
    const int norm = ((m_userRotation % 360) + 360) % 360;
    const Qt::TransformationMode mode = (norm % 90 == 0)
        ? Qt::FastTransformation : Qt::SmoothTransformation;
    return src.transformed(t, mode);
}


// ─── Video recording ─────────────────────────────────────────────────────────
//
// Flow:
//   1. ShutterButton::recordStarted → onRecordStarted()
//      Creates VideoRecorder, picks the current PipeWire node from the
//      camera selector, starts gst-launch writing H.264 .mkv to a temp path.
//   2. While recording: VideoRecorder::elapsedTick → onRecorderTick → updates
//      the red "REC 0:NN" overlay. Stop is triggered by either the user
//      releasing/tapping (recordStopped) or VideoRecorder hitting max duration.
//   3. ShutterButton::recordStopped → onRecordStopped → tells recorder to stop.
//      VideoRecorder::recordingStopped fires (file finalized) → kicks off
//      VideoTranscoder for H.264 → AV1.
//   4. VideoTranscoder::transcodeFinished → emit snapCaptured (just like
//      photos) so MainWindow's send pipeline takes over.

void CameraWidget::startPreviewPolling()
{
    if (!m_recorder) return;
    const QString previewPath = m_recorder->previewFilePath();
    if (previewPath.isEmpty()) {
        qCDebug(lcCamera) << "startPreviewPolling: recorder has no preview path";
        return;
    }
    // Reset the mtime tracking so the first poll always loads even if the
    // previous recording left a file with a known mtime (edge case after
    // a crashed recorder).
    m_previewLastMtime = 0;

    if (!m_previewPollTimer) {
        m_previewPollTimer = new QTimer(this);
        // V32.32: dropped from 50ms (20Hz) to 100ms (10Hz) to match the
        // recorder pipeline's 10fps preview output. Polling faster than
        // the source produces JPEGs is pure overhead — half the ticks
        // found nothing new even at 20Hz. 10Hz matches source rate and
        // halves the polling CPU.
        m_previewPollTimer->setInterval(100);  // 10 Hz poll
        connect(m_previewPollTimer, &QTimer::timeout, this, [this]() {
            // Recorder may have torn down between ticks — bail gracefully.
            if (!m_recorder || !m_viewfinderLabel) return;
            const QString path = m_recorder->previewFilePath();
            if (path.isEmpty()) return;
            const QFileInfo fi(path);
            if (!fi.exists()) return;
            // mtime check avoids re-decoding the same JPEG 20× per second
            // when the recorder is producing 10 fps preview frames.
            // multifilesink writes atomically so a successful load means
            // we get a coherent JPEG.
            const qint64 mt = fi.lastModified().toMSecsSinceEpoch();
            if (mt == m_previewLastMtime) return;
            m_previewLastMtime = mt;
            QImage img;
            if (!img.load(path, "JPEG")) {
                // Race: file got replaced between exists() and load().
                // Skip this tick; next mtime change will re-trigger.
                return;
            }
            // V32.32: do NOT call applyUserRotation here. As of V32.32 the
            // VideoRecorder pipeline applies user rotation BEFORE the tee
            // (videoflip element), so the JPEG written by the preview
            // branch is already in the user's desired orientation.
            // Calling applyUserRotation would double-rotate.
            //
            // The LIVE (non-recording) viewfinder path still calls
            // applyUserRotation because that path consumes raw frames
            // from GstLaunchBackend's separate pipeline which doesn't
            // have the videoflip insertion.
            QImage display = img;
            // V32.32: mirror in front-camera preview for natural
            // selfie composition. The recorded .mkv stays un-mirrored
            // — same convention as photo capture.
            if (m_mirrorPreview) {
                display = display.mirrored(true, false);
            }
            // V32.38o: route through renderToViewfinder (don't inline setPixmap)
            // so the recording-mode preview gets the SAME treatment as the live
            // path: placeholder text/style reset (otherwise a leftover
            // "Starting camera…"/error stylesheet keeps its padding+colored
            // border around the polled frame), DPR-correct scaling (crisp on
            // HiDPI), and full-bleed expand+crop (fill, not letterbox). Rotation
            // is already applied upstream by the recorder's videoflip, and
            // renderToViewfinder does not rotate, so no double-rotation.
            renderToViewfinder(display);
        });
    }
    qCInfo(lcCamera) << "Preview poll started for" << previewPath;
    m_previewPollTimer->start();
}

void CameraWidget::stopPreviewPolling()
{
    if (m_previewPollTimer) {
        m_previewPollTimer->stop();
    }
    m_previewLastMtime = 0;
}

void CameraWidget::onRecordStarted()
{
    if (m_recorder) {
        qCWarning(lcCamera) << "onRecordStarted: recorder already exists";
        return;
    }

    // Guard against starting a new recording while a previous one is still
    // transcoding. The two share the same output dir and previously a new
    // recording would silently kill the running transcode (race on file
    // names + on the v4l2 device on desktop). Now we refuse and tell the
    // user to wait.
    //
    // The transcode is short (5-15s typically) so this is rarely hit, but
    // when hit it produced confusing behavior — the prior video would just
    // disappear from the inbox. The shutter button is reset so the user
    // can immediately try again once the indicator clears.
    // Lock the camera selector for the duration of the recording (and
    // through the transcode that follows) — switching mid-record breaks
    // the recording pipeline AND the viewfinder restore.
    if (m_cameraSelector) m_cameraSelector->setEnabled(false);

    if (m_transcoder) {
        // We hit the concurrent-recording guard — re-enable the selector
        // since we're not actually starting a recording.
        if (m_cameraSelector) m_cameraSelector->setEnabled(true);
        qCInfo(lcCamera) << "onRecordStarted: previous recording still encoding — refused";
        if (m_shutter) {
            m_shutter->setRecording(false);
            m_shutter->setLocked(false);
        }
        if (m_recordIndicator) {
            m_recordIndicator->setText(tr("Encoding previous video — wait a moment"));
            m_recordIndicator->setStyleSheet(QStringLiteral(
                "background: rgba(180,140,40,0.92); color: white;"
                "padding: 6px 12px; border-radius: 12px;"
                "font-size: 12px;"));
            m_recordIndicator->adjustSize();
            m_recordIndicator->show();
            QTimer::singleShot(2500, this, [this]() {
                if (m_recordIndicator && !m_recorder) m_recordIndicator->hide();
            });
        }
        return;
    }

    // Resolve the source path/node for the recorder. We use the same id
    // as the active backend so we get rear/front consistent with the viewfinder.
    QString pwNode;
    if (m_cameraSelector && m_cameraSelector->count() > 0) {
        pwNode = m_cameraSelector->currentData().toString();
    }
    // V32.34+: if the user is in PIP mode, the dropdown's currentData
    // is `kPipSentinel` ("__pip__") — a non-existent pipewire node.
    // Passing that to gst-launch produces target-object=__pip__ which
    // fails negotiation immediately (device log: many "Format
    // negotiation failed" entries on recorder pipelines).
    //
    // The intent when recording in PIP mode is to record the MAIN
    // stream (rear by default) — the PIP overlay is a UI-layer render,
    // not a recordable composition. Substitute the actual rear node.
    // If the user wanted the front stream they'd have switched the
    // dropdown out of PIP first.
    //
    // This sits BEFORE the v4l2 check so the v4l2 detection logic
    // also sees the real device path.
    if (pwNode == QString::fromLatin1(kPipSentinel)) {
        const QString realId = firstRearDeviceId();
        qCInfo(lcCamera) << "Recording requested while in PIP mode — "
                         << "substituting main camera node:" << realId;
        pwNode = realId;
    } else if (pwNode == QString::fromLatin1(kPipExtSentinel)) {
        // V32.38+: in "Camera + USB PIP" the MAIN stream is the internal
        // webcam; record that, not the sentinel. (PIP overlay isn't recorded.)
        const QString realId = firstInternalDeviceId();
        qCInfo(lcCamera) << "Recording requested while in USB-PIP mode — "
                         << "substituting main camera node:" << realId;
        pwNode = realId;
    }

    // CRITICAL: some backends hold the camera device exclusively, so the
    // recorder's gst-launch can't open it until the backend releases it. This
    // is true for BOTH:
    //   • QtCameraBackend on desktop — holds /dev/videoN; v4l2src needs it free.
    //   • the direct LibcameraBackend — has acquire()'d a libcamera camera id
    //     like /base/.../camera@29; libcamerasrc needs it free.
    // Both id forms are absolute paths. A camera-bridge PipeWire node
    // ("virtual-cam-rear") is NOT a path and CAN be read by several consumers
    // at once, so for that case the backend keeps running. (VideoRecorder picks
    // the matching gst source element — v4l2src / libcamerasrc / pipewiresrc —
    // from the same id string, so the two stay in lock-step.)
    const bool isExclusiveDevice = pwNode.startsWith(QStringLiteral("/"));
    if (isExclusiveDevice && m_backend && m_backend->isActive()) {
        qCInfo(lcCamera) << "Stopping preview backend so the recorder can open"
                         << pwNode << "(will resume after recording)";
        m_backend->stop();
        m_viewfinderPausedForRecording = true;

        // CLEAR the viewfinder pixmap immediately. Otherwise the last
        // frame captured before stop() stays on screen for the entire
        // recording window (1500 ms kernel wait + N seconds of record)
        // and the user thinks the camera has frozen.
        //
        // We replace the stale frame with a black background and a clear
        // "RECORDING" placeholder so it's visually OBVIOUS we've switched
        // modes. The existing m_recordIndicator (the small red dot in the
        // corner) stays visible too — it shows elapsed time.
        if (m_viewfinderLabel) {
            // Render a solid-black pixmap at the current viewfinder size
            // with centered "RECORDING" text. Using a pixmap rather than
            // a stylesheet change keeps consistency with the rest of the
            // frame-display path (setPixmap is how onVideoFrame updates).
            const QSize vfSize = m_viewfinderLabel->size();
            if (vfSize.width() > 0 && vfSize.height() > 0) {
                QPixmap pm(vfSize);
                pm.fill(QColor(20, 20, 20));  // not pure black — feels less broken
                QPainter painter(&pm);
                painter.setRenderHint(QPainter::Antialiasing);
                painter.setRenderHint(QPainter::TextAntialiasing);
                // Large pulsing red dot above the word REC (mirrors the
                // small corner indicator but full-screen-scale so it's
                // unmistakable that we're recording, not frozen).
                const int cx = vfSize.width() / 2;
                const int cy = vfSize.height() / 2;
                painter.setBrush(QColor(220, 60, 60));
                painter.setPen(Qt::NoPen);
                painter.drawEllipse(QPoint(cx, cy - 24), 18, 18);
                painter.setPen(QColor(240, 240, 240));
                QFont f = painter.font();
                f.setPointSizeF(qMax(14.0, vfSize.height() / 32.0));
                f.setBold(true);
                painter.setFont(f);
                painter.drawText(QRect(0, cy + 8, vfSize.width(), 40),
                                 Qt::AlignHCenter | Qt::AlignTop,
                                 tr("RECORDING"));
                painter.end();
                // V32.38o: clear any leftover placeholder text/style (e.g. a
                // prior "Starting camera…" or error stylesheet with padding +
                // colored background) before showing this pixmap, otherwise its
                // 20px padding insets the RECORDING placeholder with a stray
                // colored border. Same reset renderToViewfinder does.
                if (!m_viewfinderLabel->text().isEmpty()
                    || !m_viewfinderLabel->styleSheet().isEmpty()) {
                    m_viewfinderLabel->setText(QString());
                    m_viewfinderLabel->setStyleSheet(
                        QStringLiteral("background: #000000;"));
                }
                m_viewfinderLabel->setPixmap(pm);
            } else {
                m_viewfinderLabel->setPixmap(QPixmap());  // just clear
            }
        }
    }

    // ── Decide where the intermediate .mkv lives ────────────────────────────
    // Per XDG Base Directory Spec, recordings are CACHE: regenerable
    // (re-record), non-essential after the snap is sent, deleted in
    // onTranscodeFinished/Failed. CacheLocation maps to:
    //   • Linux:  $XDG_CACHE_HOME/SnapClone  (defaults to ~/.cache/SnapClone)
    //   • macOS:  ~/Library/Caches/SnapClone
    //   • Win:    %LOCALAPPDATA%\SnapClone\cache
    // Backup tools (deja-dup, restic, etc.) auto-exclude ~/.cache, which is
    // exactly what we want for ephemeral video files.
    //
    // Persistent app data (SQLite DB, identity keys) stays in
    // AppLocalDataLocation. Files the user explicitly saves go to
    // MoviesLocation (handled in VideoSnapViewerDialog).
    //
    // Why not TempLocation (/tmp): on sandboxed/Flatpak systems /tmp is
    // app-private and cleaned aggressively; can also be tmpfs-backed and
    // small. CacheLocation is guaranteed user-writable on a real disk.
    const QString recDir = QStandardPaths::writableLocation(
                               QStandardPaths::CacheLocation)
                         + QStringLiteral("/recordings");
    if (!QDir().mkpath(recDir)) {
        qCWarning(lcCamera) << "Cannot create recordings dir:" << recDir
                            << "— recording will likely fail";
    }
    const QString outPath = recDir + QStringLiteral("/rec-")
                          + QString::number(QDateTime::currentMSecsSinceEpoch())
                          + QStringLiteral(".mkv");

    m_recorder = new Video::VideoRecorder(this);
    constexpr int kMaxDurationMs = 30 * 60 * 1000;  // 30 minutes per user spec

    connect(m_recorder, &Video::VideoRecorder::elapsedTick,
            this, &CameraWidget::onRecorderTick);
    connect(m_recorder, &Video::VideoRecorder::recordingStopped,
            this, [this](const QString &path, int /*dur*/) {
        qCInfo(lcCamera) << "Recording done:" << path << "— starting transcode";
        m_recordIndicator->hide();
        m_shutter->setRecording(false);
        m_shutter->setLocked(false);
        // Note: the recording indicator is reused for the "Encoding N%"
        // overlay (set by the transcoder's progress signal below), so no
        // explicit status-bar handoff is needed here.
        m_recordingPath = path;

        // Stop polling the preview file — recorder has exited, no more
        // frames coming. The viewfinder will get live Qt frames once the
        // backend resume completes (below).
        stopPreviewPolling();

        // Resume the viewfinder NOW (not after transcode). The recorder's
        // gst-launch has exited and released /dev/video0; the transcoder
        // reads from a FILE, not the camera, so the camera is free.
        // Previously we waited until onTranscodeFinished to resume, which
        // froze the viewfinder for the full encode duration (5-30 s on
        // desktop, longer on mobile). Users assumed the camera had died.
        //
        // Defer 500 ms — gst-launch's v4l2src releases USB bandwidth on
        // its own teardown, and Qt's QCamera::start() calls VIDIOC_STREAMON
        // which can race that release otherwise. Mirror of the start-
        // recording wait, just shorter since gst-launch has fully exited.
        if (m_viewfinderPausedForRecording && m_backend) {
            QPointer<CameraWidget> self(this);
            QTimer::singleShot(500, this, [self]() {
                if (!self) return;
                auto *backend = self->m_backend.get();
                if (self->m_viewfinderPausedForRecording && backend) {
                    qCInfo(lcCamera) << "Resuming viewfinder after recording "
                                        "(transcode continues in background)";
                    self->resumeSelectedCamera();  // V32.35+: keep selected cam
                    self->m_viewfinderPausedForRecording = false;
                }
            });
        }

        // Kick off transcode
        if (m_transcoder) {
            // V32.19: disconnect signals BEFORE deleteLater. Otherwise a
            // late-arriving transcodeFinished from the old transcoder would
            // run onTranscodeFinished, which calls `m_transcoder->deleteLater()`
            // on what is now the NEW transcoder — killing the in-flight encode
            // and corrupting the send for the new recording.
            disconnect(m_transcoder, nullptr, this, nullptr);
            m_transcoder->deleteLater();
        }
        m_transcoder = new Video::VideoTranscoder(this);
        connect(m_transcoder, &Video::VideoTranscoder::transcodeFinished,
                this, &CameraWidget::onTranscodeFinished);
        connect(m_transcoder, &Video::VideoTranscoder::transcodeFailed,
                this, &CameraWidget::onTranscodeFailed);
        connect(m_transcoder, &Video::VideoTranscoder::progress,
                this, [this](int pct) {
            // V32.32: themed transcode indicator with backend-kind glyph.
            //
            // BEFORE: hardcoded rgba(0,140,200,...) blue — clashed with
            // every theme except the default. Now uses Theme::ACCENT()
            // so the indicator matches whatever palette the user
            // selected in the carousel.
            //
            // The glyph at the start shows which silicon is doing the
            // work: 💻 CPU, 🎮 GPU, ⚡ dedicated hardware encoder block.
            // When the user adds load they see a clear "the system IS
            // working hard right now" indicator AND know whether their
            // SC_USE_GPU / SC_USE_HW_ENCODER env vars actually took
            // effect (the glyph would update accordingly).
            const auto kind = m_transcoder->activeBackendKind();
            const char *glyph;
            const char *kindLabel;
            switch (kind) {
                case Video::IVideoEncoderBackend::Kind::Hardware:
                    glyph = "⚡"; kindLabel = "HW"; break;
                case Video::IVideoEncoderBackend::Kind::Gpu:
                    glyph = "🎮"; kindLabel = "GPU"; break;
                case Video::IVideoEncoderBackend::Kind::Cpu:
                default:
                    glyph = "💻"; kindLabel = "CPU"; break;
            }
            // Show "💻 CPU · Encoding 42%" — the kind label makes the
            // glyph readable even on monochrome terminals / Linux mobile
            // builds where emoji rendering varies.
            m_recordIndicator->setText(
                tr("%1 %2 · Encoding %3%")
                    .arg(QString::fromUtf8(glyph),
                         QString::fromUtf8(kindLabel))
                    .arg(pct));
            m_recordIndicator->setStyleSheet(QStringLiteral(
                "background: %1; color: %2;"
                "padding: 4px 12px; border-radius: 12px;"
                "font-weight: 600; font-size: 13px;")
                .arg(QLatin1String(Theme::ACCENT()))
                .arg(QLatin1String(Theme::TEXT_INVERSE())));
            m_recordIndicator->adjustSize();
            m_recordIndicator->show();
        });

        // Default requested output is .webm (AV1/VP9/VP8). If the VVC backend
        // is selected, the transcoder rewrites this to .mkv internally and the
        // transcodeFinished signal carries the actual path — we don't reuse
        // this local after start() except for the fallback below.
        const QString requestedOut = path.left(path.lastIndexOf('.'))
                               + QStringLiteral(".webm");
        if (!m_transcoder->start(path, requestedOut)) {
            // Transcode couldn't start — fall back to sending the .mkv directly
            qCWarning(lcCamera) << "Transcode failed to start — sending .mkv directly";
            onTranscodeFinished(path);
        }
    });
    connect(m_recorder, &Video::VideoRecorder::recordingFailed,
            this, [this](const QString &reason) {
        qCWarning(lcCamera) << "Recording failed:" << reason;
        m_recordIndicator->hide();
        m_shutter->setRecording(false);
        m_shutter->setLocked(false);

        // Stop polling the preview file — recorder torn down, no more
        // frames. Critical to call BEFORE deleteLater on m_recorder so
        // the poll timer's lambda doesn't try to read previewFilePath()
        // on a half-destroyed object. (Even though the timer is stopped
        // first, deferred-delete plus an already-queued timer slot can
        // race; stopping the timer before pulling the recorder out is
        // the safe ordering.)
        stopPreviewPolling();

        m_recorder->deleteLater();
        m_recorder = nullptr;
        if (m_cameraSelector) m_cameraSelector->setEnabled(true);
        // Resume viewfinder if we paused it for recording. Even on failure,
        // gst-launch may have briefly held the device — defer 500ms so the
        // kernel processes its STREAMOFF before Qt does STREAMON. Without
        // this defer, a recording failure followed by Qt resume sometimes
        // leaves the viewfinder permanently black until camera reselect.
        if (m_viewfinderPausedForRecording && m_backend) {
            QPointer<CameraWidget> self(this);
            QTimer::singleShot(500, this, [self]() {
                if (!self) return;
                // Cache the raw pointer first. GCC 15's -Wnull-dereference
                // warns when chaining `unique_ptr::operator bool()` followed
                // by `unique_ptr::operator->()` across inline expansion —
                // it can't see the no-throw contract of operator bool.
                // Caching gives the analyzer a single, clearly-checked
                // pointer to reason about.
                auto *backend = self->m_backend.get();
                if (self->m_viewfinderPausedForRecording && backend) {
                    self->resumeSelectedCamera();  // V32.35+: keep selected cam
                    self->m_viewfinderPausedForRecording = false;
                }
            });
        }
        // Surface the reason to the user via status bar (status bar lives in
        // MainWindow — easiest path is via the existing error signal pattern).
        // Direct: write to viewfinder overlay briefly. Fallback to log.
        if (m_viewfinderLabel) {
            // Show a transient error message in the recording indicator slot
            m_recordIndicator->setText(reason);
            m_recordIndicator->setStyleSheet(QStringLiteral(
                "background: rgba(180,40,40,0.92); color: white;"
                "padding: 6px 12px; border-radius: 12px;"
                "font-size: 12px;"));
            m_recordIndicator->adjustSize();
            m_recordIndicator->show();
            QTimer::singleShot(5000, this, [this]() {
                if (m_recordIndicator) m_recordIndicator->hide();
            });
        }
    });

    // Show the REC indicator IMMEDIATELY so the user gets visual feedback
    // even if we have to delay the actual recorder start (v4l2 release lag).
    m_recordIndicator->setText(QStringLiteral("● REC 0:00"));
    m_recordIndicator->setStyleSheet(QStringLiteral(
        "background: rgba(232,60,60,0.85); color: white;"
        "padding: 4px 12px; border-radius: 12px;"
        "font-weight: 600; font-size: 13px;"));
    m_recordIndicator->adjustSize();
    m_recordIndicator->show();
    positionTopOverlays();   // V32.38l: clear the floating top bar in full-bleed

    // On v4l2 (single-consumer) we just stopped the Qt backend — give Qt's
    // FFmpeg pipeline AND the kernel time to fully release the device before
    // gst-launch opens it. The Qt FFmpeg backend tears down its V4L2 buffer
    // pool ASYNCHRONOUSLY, so even after backend->stop() returns, USERPTR
    // memory may still be pinned for an unpredictable duration (50ms-1500ms
    // depending on kernel scheduling, GPU pressure, and codec teardown).
    //
    // Instead of a fixed sleep, we POLL the device. We try opening it with
    // O_NONBLOCK; if that succeeds we close immediately and start gst-launch.
    // If it fails (EBUSY / EAGAIN), back off and retry. Cap total wait at 3s
    // so a stuck driver eventually surfaces a real recording failure rather
    // than hanging the UI forever.
    //
    // On PipeWire (multi-consumer), no polling needed — fire immediately.
    auto launchRecorder = [this, pwNode, outPath, kMaxDurationMs]() {
        if (!m_recorder) return;  // user already cancelled / state changed
        // V32.32: pass the same user-rotation override the live viewfinder
        // and photo-capture paths already apply, so the .mkv file written
        // for the recipient is upright instead of sensor-native. Without
        // this, the FP5 rear camera (landscape sensor in portrait body)
        // produced sideways recordings despite the in-app preview looking
        // correct — preview ran through applyUserRotation but the recorder
        // pipeline didn't.
        m_recorder->setUserRotation(m_userRotation);
        if (!m_recorder->start(pwNode, outPath, kMaxDurationMs)) {
            // start() failed at the QProcess::start level (gst-launch missing
            // from PATH, etc). The recordingFailed signal will not have fired
            // because the process never spawned, so we manually clean up state.
            qCWarning(lcCamera) << "Recorder start() returned false";
            if (m_recordIndicator) m_recordIndicator->hide();
            m_recorder->deleteLater();
            m_recorder = nullptr;
            if (m_shutter) {
                m_shutter->setRecording(false);
                m_shutter->setLocked(false);
            }
            if (m_cameraSelector) m_cameraSelector->setEnabled(true);
            if (m_viewfinderPausedForRecording && m_backend) {
                resumeSelectedCamera();  // V32.35+: keep selected cam
                m_viewfinderPausedForRecording = false;
            }
            return;
        }
        // Recorder running. If the viewfinder is paused (v4l2 path), poll
        // the recorder's tee-branch JPEG preview file to display live
        // frames in the viewfinder during recording. PipeWire path leaves
        // the Qt viewfinder running as-is.
        if (m_viewfinderPausedForRecording) startPreviewPolling();
    };

    if (!m_viewfinderPausedForRecording) {
        // PipeWire path — fire immediately
        launchRecorder();
    } else {
        // v4l2 path — give the kernel time to process VIDIOC_STREAMOFF and
        // release USB bandwidth (for UVC devices like FaceTime HD this is
        // critical: the USB host controller's "periodic bandwidth slot"
        // is held until the kernel processes the alt-setting reset, which
        // happens AFTER QCamera teardown completes async on the FFmpeg
        // backend's worker thread).
        //
        // Why a fixed wait: the actual gating ioctl is VIDIOC_STREAMON
        // inside gst's decide_allocation(), and STREAMON has no
        // pre-checkable equivalent. REQBUFS-based polling (which we tried
        // before) returns success even when STREAMON would fail with EBUSY
        // / ENOSPC because they test different things (REQBUFS = memory,
        // STREAMON = bandwidth). The only reliable signal is "did Qt's
        // backend actually release the device" and Qt provides no event
        // for that.
        //
        // 1500 ms is empirically sufficient on Asahi/Fedora with FaceTime
        // HD UVC. The user will see a brief "REC" indicator delay before
        // capture starts, which is acceptable for a recording mode switch.
        // QtCameraBackend::stop() now does FULL teardown (destroys
        // QCamera/Session/Sink), so the kernel actually gets to process
        // STREAMOFF — without that change, NO wait was long enough.
        qCInfo(lcCamera) << "v4l2 path: Qt backend released, waiting 1500ms"
                         << "for kernel STREAMOFF + USB bandwidth release"
                         << "before launching gst-launch";
        QTimer::singleShot(1500, this, [launchRecorder]() {
            launchRecorder();
        });
    }
}

void CameraWidget::onRecordLocked()
{
    qCInfo(lcCamera) << "Recording locked (hands-free until 30 min or tap to stop)";
    // Visual cue: append "(LOCKED)" so user knows tap-to-stop will work
    if (m_recordIndicator) {
        QString t = m_recordIndicator->text();
        if (!t.contains(QLatin1String("LOCKED")))
            m_recordIndicator->setText(t + QStringLiteral(" 🔒"));
        m_recordIndicator->adjustSize();
    }
}

void CameraWidget::onRecordStopped(int /*durMs*/)
{
    if (!m_recorder) return;
    qCInfo(lcCamera) << "Stopping recording from shutter";
    m_recorder->stop();
    // recordingStopped signal will fire and clean up via the lambda above
}

void CameraWidget::onRecorderTick(int elapsedMs)
{
    if (!m_recordIndicator) return;
    const int sec = elapsedMs / 1000;
    const int m = sec / 60;
    const int s = sec % 60;
    QString lockTag;
    if (m_shutter && m_shutter->isLocked()) lockTag = QStringLiteral(" 🔒");
    m_recordIndicator->setText(QStringLiteral("● REC %1:%2%3")
        .arg(m).arg(s, 2, 10, QChar('0')).arg(lockTag));
    m_recordIndicator->adjustSize();
}

void CameraWidget::onTranscodeFinished(const QString &outputPath)
{
    qCInfo(lcCamera) << "Transcode finished:" << outputPath << "— sending as snap";
    if (m_recordIndicator) m_recordIndicator->hide();

    // Read the transcoded file into memory and emit snapCaptured. The MainWindow
    // handler will encode it as an snap.encryptedBlob and route it through the
    // existing sendSnap pipeline. The recipient gets MediaType::Video.
    // NOTE: outputPath is whatever the transcoder actually wrote — usually
    // .webm (AV1/VP9/VP8), but .mkv when the VVC backend was selected (webm
    // can't carry H.266). We send these bytes regardless of extension; the
    // receiver's viewer detects the container. Do NOT assume .webm here.
    QFile f(outputPath);
    if (!f.open(QIODevice::ReadOnly)) {
        qCWarning(lcCamera) << "Cannot read transcoded file:" << outputPath;
        return;
    }
    const QByteArray bytes = f.readAll();
    f.close();

    // We pack into a QImage with a synthetic format: this is a hack until we
    // extend snapCaptured to carry mediaType. For now, emit the existing
    // signal but stash the path in a property the MainWindow handler can
    // detect.
    // CLEANER alternative: emit a dedicated signal videoSnapCaptured(path, durationMs).
    emit videoSnapCaptured(outputPath,
        m_recorder ? static_cast<int>(m_recorder->elapsedMs()) : 0);

    // Cleanup recorder/transcoder
    if (m_recorder) { m_recorder->deleteLater(); m_recorder = nullptr; }
    if (m_transcoder) { m_transcoder->deleteLater(); m_transcoder = nullptr; }
    // Remove the ORIGINAL recording source (m_recordingPath, the raw .mkv from
    // the recorder) — the transcoded output (outputPath) is what gets sent.
    if (!m_recordingPath.isEmpty()) QFile::remove(m_recordingPath);
    m_recordingPath.clear();

    // Resume viewfinder if we paused for recording (v4l2 single-consumer case).
    // Defer 500 ms — gst-launch's v4l2src releases USB bandwidth on its own
    // teardown, and Qt's QCamera::start() calls VIDIOC_STREAMON which would
    // race that release otherwise (mirror of the start-recording path's
    // 1500ms wait, just shorter since we've fully exited gst-launch already).
    if (m_viewfinderPausedForRecording && m_backend) {
        QPointer<CameraWidget> self(this);
        QTimer::singleShot(500, this, [self]() {
            if (!self) return;
            if (self->m_viewfinderPausedForRecording && self->m_backend) {
                self->resumeSelectedCamera();  // V32.35+: keep selected cam
                self->m_viewfinderPausedForRecording = false;
            }
        });
    }
    if (m_cameraSelector) m_cameraSelector->setEnabled(true);
}

void CameraWidget::onTranscodeFailed(const QString &reason)
{
    qCWarning(lcCamera) << "Transcode failed:" << reason
                        << "— falling back to sending raw .mkv";
    // Fall back: just send the original recording
    if (!m_recordingPath.isEmpty()) {
        emit videoSnapCaptured(m_recordingPath,
                               m_recorder ? static_cast<int>(m_recorder->elapsedMs()) : 0);
    }
    if (m_recorder) { m_recorder->deleteLater(); m_recorder = nullptr; }
    if (m_transcoder) { m_transcoder->deleteLater(); m_transcoder = nullptr; }
    m_recordingPath.clear();
    if (m_recordIndicator) m_recordIndicator->hide();

    // Same deferred resume as onTranscodeFinished — see rationale there.
    if (m_viewfinderPausedForRecording && m_backend) {
        QPointer<CameraWidget> self(this);
        QTimer::singleShot(500, this, [self]() {
            if (!self) return;
            if (self->m_viewfinderPausedForRecording && self->m_backend) {
                self->resumeSelectedCamera();  // V32.35+: keep selected cam
                self->m_viewfinderPausedForRecording = false;
            }
        });
    }
    if (m_cameraSelector) m_cameraSelector->setEnabled(true);
}

// ─── V32.32: tap-to-focus ────────────────────────────────────────────────────
//
// The user taps the viewfinder → eventFilter catches the press → we call
// runFastFocusSweep() which drives the VCM through 5 positions, picks the
// peak-sharpness one, and leaves the lens parked there. Whole sweep ~750ms.
//
// re-entry protection: m_focusInProgress prevents overlapping sweeps. A
// second tap during a sweep is dropped silently (the user isn't shown
// feedback about the dropped tap — the lens is moving, they'll see it).

bool CameraWidget::eventFilter(QObject *obj, QEvent *ev)
{
    // V32.34+ landmark hint: tap on the pill opens the AddLandmarkDialog
    // with the captured frame. We accept the press to keep it from
    // propagating to the viewfinder underneath (which would also trigger
    // a focus sweep — visually confusing). Release returns false so
    // normal click semantics aren't broken.
    if (obj == m_landmarkHint && ev->type() == QEvent::MouseButtonPress) {
        onLandmarkHintTapped();
        return true;
    }
    // V32.33: viewfinder resize → reposition PIP overlay (bottom-right
    // tracking). The PIP label is parented to the viewfinder so it
    // composites correctly, but Qt doesn't auto-anchor children to corners.
    if (obj == m_viewfinderLabel && ev->type() == QEvent::Resize) {
        // V32.38m: re-render the cached frame at the NEW size. Without this the
        // viewfinder keeps the old-size pixmap (QLabel doesn't rescale it — not
        // setScaledContents) until the next live frame, so on window resize /
        // device rotation / keyboard show-hide the preview briefly shows wrong
        // (full-bleed: black gaps or mis-crop; classic: wrong letterbox) — and
        // INDEFINITELY if frames are paused (backgrounded) or the stream stalled.
        // Reposition the floating bars/scrims FIRST so renderToViewfinder's
        // scrim-backdrop sampling uses the new geometry, then re-blit the most
        // recent frame through the same scale/crop/dpr path.
        repositionPipOverlay();
        if (m_fullBleed) layoutFloatingBars();
        QImage cached;
        {
            QMutexLocker lock(&m_frameMutex);
            cached = m_latestFrame;   // implicit-shared copy (O(1))
        }
        if (!cached.isNull()) {
            if (m_mirrorPreview) cached = cached.mirrored(true, false);
            renderToViewfinder(cached);
        }
        // V32.34+ AF UI: keep the AF badge anchored to bottom-left as the
        // viewfinder resizes. updateAfBadge handles positioning.
        if (m_afBadge && m_afBadge->isVisible()) updateAfBadge();
        // V32.34+ landmark hint: keep the pill centered on the bottom
        // edge through resize events too. We recompute its position
        // only if it's currently visible to skip work when not needed.
        if (m_landmarkHint && m_landmarkHint->isVisible() && m_viewfinderLabel) {
            m_landmarkHint->adjustSize();
            const int x = (m_viewfinderLabel->width() - m_landmarkHint->width()) / 2;
            const int y = m_viewfinderLabel->height() - m_landmarkHint->height() - 24;
            m_landmarkHint->move(qMax(8, x), qMax(8, y));
        }
        return false;  // don't consume; let the label resize normally
    }
    // V32.38d: bar-gap tap-to-focus. In full-bleed the top/bottom bars span the
    // full width and are mouse-OPAQUE (they must be — a mouse-transparent parent
    // swallows its children's clicks too, verified). Their controls only occupy
    // part of that width; the stretch gaps between controls look like live
    // preview but are bar pixels, so a tap there would otherwise be a dead zone
    // (no focus, no nothing). If a press lands on the bar itself and NOT on one
    // of its child controls, forward it to the viewfinder focus handler so those
    // gaps behave like the rest of the preview.
    if ((obj == m_topBarWidget || obj == m_bottomBarWidget)
        && (ev->type() == QEvent::MouseButtonPress
            || ev->type() == QEvent::TouchBegin)) {
        auto *bar = static_cast<QWidget *>(obj);
        QPointF localF;
        if (ev->type() == QEvent::MouseButtonPress) {
            localF = static_cast<QMouseEvent *>(ev)->position();
        } else {
            const auto pts = static_cast<QTouchEvent *>(ev)->points();
            if (pts.isEmpty()) return false;
            localF = pts.first().position();
        }
        // Defer to the child ONLY if it's actually interactive (button/combo).
        // A non-interactive child (e.g. the "Shoot then choose" QLabel) would
        // otherwise swallow the press and become a dead spot — treat those like
        // the bar itself and forward to focus below.
        if (QWidget *hit = bar->childAt(localF.toPoint())) {
            const bool interactive =
                qobject_cast<QAbstractButton *>(hit)
                || qobject_cast<QComboBox *>(hit)
                || hit->focusPolicy() != Qt::NoFocus;
            if (hit != bar && interactive) {
                if (ev->type() == QEvent::TouchBegin) {
                    // On touch devices the bar accepts touch (needed for gap
                    // taps), which can suppress Qt's mouse synthesis for a child
                    // that doesn't accept touch — so a finger tap on the button
                    // would be lost. Synthesize the click to the child (same
                    // approach SwipeNavigator uses) and consume the touch so Qt
                    // doesn't also synthesize (which would double-activate).
                    const QPoint inChild = hit->mapFrom(bar, localF.toPoint());
                    const QPointF global = hit->mapToGlobal(inChild);
                    QCoreApplication::postEvent(hit, new QMouseEvent(
                        QEvent::MouseButtonPress, QPointF(inChild), QPointF(inChild),
                        global, Qt::LeftButton, Qt::LeftButton, Qt::NoModifier,
                        Qt::MouseEventSynthesizedByApplication));
                    QCoreApplication::postEvent(hit, new QMouseEvent(
                        QEvent::MouseButtonRelease, QPointF(inChild), QPointF(inChild),
                        global, Qt::LeftButton, Qt::NoButton, Qt::NoModifier,
                        Qt::MouseEventSynthesizedByApplication));
                    return true;  // consumed: we delivered it ourselves
                }
                return false;     // mouse press: let normal delivery handle it
            }
        }
        if (m_recorder && m_recorder->isRecording()) return false;
        if (m_viewfinderLabel) {
            // Focus ONLY — no double-tap-flip from the chrome strip (flipping the
            // camera because you tapped twice near the top bar is surprising).
            // Map the bar-local point into viewfinder-local coords.
            const QPoint inVf = m_viewfinderLabel->mapFrom(bar, localF.toPoint());
            triggerFocusOnly(QPointF(inVf));
        }
        return false; // don't consume
    }
    if (obj == m_viewfinderLabel
        && (ev->type() == QEvent::MouseButtonPress
            || ev->type() == QEvent::TouchBegin)) {
        // Don't trigger focus during recording — the lens move would
        // produce a visible focus pull mid-recording. (If users WANT
        // a focus pull they'd ask for it; default is "don't surprise me".)
        // Double-tap-to-flip is also suppressed mid-recording — switching
        // cameras would tear the recording.
        if (m_recorder && m_recorder->isRecording()) {
            return false;
        }
        // V32.35+: extract the tap point from EITHER a mouse press or a touch
        // begin. Plasma Mobile may deliver finger taps as QTouchEvent without
        // synthesizing the QMouseEvent the old code required — that was why a
        // mouse click showed the AF reticle but a finger tap didn't. We funnel
        // both into one handler.
        QPointF tapPoint;
        bool havePoint = false;
        if (ev->type() == QEvent::MouseButtonPress) {
            if (auto *me = static_cast<QMouseEvent *>(ev)) {
                tapPoint = me->position();
                havePoint = true;
            }
        } else { // TouchBegin
            if (auto *te = static_cast<QTouchEvent *>(ev)) {
                const auto pts = te->points();
                if (!pts.isEmpty()) {
                    // position() is in this widget's local coords (the
                    // viewfinder label, which the event filter is installed on).
                    tapPoint = pts.first().position();
                    havePoint = true;
                }
            }
        }
        if (!havePoint) {
            return false;
        }
        // V32.32 ghost-tap debounce is now folded into handleViewfinderTap,
        // which must distinguish a deliberate double-tap from a digitizer
        // bounce — so the old "swallow anything within 250ms" rule can't live
        // here anymore (it would eat the second tap of a flip). See
        // handleViewfinderTap for the new timing model.
        handleViewfinderTap(tapPoint);
        return false; // don't consume; let normal label handling continue
    }
    return QWidget::eventFilter(obj, ev);
}

// ─── V32.35+: single-tap-focus / double-tap-flip ─────────────────────────────
//
// A viewfinder tap is ambiguous until the double-tap window passes: one tap
// means "focus here", two means "flip the camera". We can't know which on the
// first press, so:
//
//   * First tap: stash the point, record the time, arm m_pendingFocusTimer for
//     kDoubleTapMs. Show the reticle immediately (responsive feedback) — if it
//     turns out to be a flip, the camera switch visually supersedes it.
//   * Second tap within kDoubleTapMs: cancel the pending focus and flip.
//   * Timer expires with no second tap: it was a single tap → run the focus
//     sweep for the stored point.
//
// Tradeoff: tap-to-focus is delayed by kDoubleTapMs. This is the conventional
// phone-camera behavior and the only correct way to disambiguate without a
// modifier. A genuine digitizer bounce (two events a few ms apart) will look
// like a fast double-tap and trigger a flip — but a real bounce is sub-~40ms
// and a human double-tap is ~120-300ms, so we additionally ignore a "second
// tap" that arrives implausibly fast (< kBounceMs) as a bounce, not a flip.
void CameraWidget::triggerFocusOnly(const QPointF &point)
{
    // No double-tap arming: immediate single focus. Mirrors the deferred
    // single-tap path of handleViewfinderTap minus the flip logic.
    if (m_focusInProgress.load()) return;
    m_lastFocusTapPoint = point;
    if (m_focusReticle) m_focusReticle->triggerAt(point);
    QMetaObject::invokeMethod(this, "runFastFocusSweep", Qt::QueuedConnection);
}

void CameraWidget::handleViewfinderTap(const QPointF &point)
{
    constexpr qint64 kDoubleTapMs = 280;  // max gap counted as a double-tap
    constexpr qint64 kBounceMs    = 40;   // faster than this = digitizer bounce
    const qint64 now = QDateTime::currentMSecsSinceEpoch();

    // Lazily create the deferred-focus timer (single-shot).
    if (!m_pendingFocusTimer) {
        m_pendingFocusTimer = new QTimer(this);
        m_pendingFocusTimer->setSingleShot(true);
        connect(m_pendingFocusTimer, &QTimer::timeout, this, [this]() {
            // The double-tap window passed with no second tap → it was a
            // single tap. Run focus for the stored point (re-applying the
            // original re-entry guard).
            if (m_focusInProgress.load()) return;
            m_lastFocusTapPoint = m_pendingFocusPoint;
            if (m_focusReticle) m_focusReticle->triggerAt(m_pendingFocusPoint);
            QMetaObject::invokeMethod(this, "runFastFocusSweep",
                                      Qt::QueuedConnection);
        });
    }

    const qint64 sinceFirst = now - m_firstTapMs;
    const bool timerArmed = m_pendingFocusTimer->isActive();

    if (timerArmed && sinceFirst >= kBounceMs && sinceFirst <= kDoubleTapMs) {
        // Second deliberate tap inside the window → this is a double-tap: flip.
        m_pendingFocusTimer->stop();
        m_firstTapMs = 0;
        // V32.35+: if a focus sweep somehow started (e.g. the deferred single-
        // tap fired right at the window edge), abort it so the lens doesn't
        // keep moving while we switch cameras — the "dump the cheap cast"
        // bail-out. Harmless if no sweep is running.
        m_focusAbort = true;
        flipCameraFacing();
        return;
    }
    if (timerArmed && sinceFirst < kBounceMs) {
        // Implausibly fast "second tap" → digitizer bounce. Ignore it; the
        // armed single-tap focus stays pending.
        return;
    }

    // First tap (or a tap after a previous gesture completed): arm the
    // deferred single-tap focus and show immediate reticle feedback.
    m_firstTapMs = now;
    m_pendingFocusPoint = point;
    if (m_focusReticle) m_focusReticle->triggerAt(point);
    m_pendingFocusTimer->start(kDoubleTapMs);
}

// V32.35+: flip front<->rear by selecting the next device with the opposite
// facing in m_cameraSelector. Setting the combo index fires onSelectorChanged —
// the exact path the menu uses — so the actual switch, mirroring, AF badge,
// and registry metadata all go through one codepath. If no opposite-facing
// device exists, this is a no-op (logged).
void CameraWidget::flipCameraFacing()
{
    if (!m_cameraSelector || !m_backend) return;

    // Determine the current device's facing from the registry.
    const QString currentId = m_cameraSelector->currentData().toString();
    const auto curInfo = Camera::CameraRegistry::instance().byId(currentId);
    const Camera::Facing want =
        (curInfo && curInfo->facing == Camera::Facing::Front)
            ? Camera::Facing::Rear
            : Camera::Facing::Front;

    // Find the first selector entry whose device has the wanted facing. We scan
    // the combo (not the registry) so we only land on devices actually present
    // and enumerated in the menu.
    for (int i = 0; i < m_cameraSelector->count(); ++i) {
        const QString id = m_cameraSelector->itemData(i).toString();
        if (id.isEmpty()) continue;                 // skip PIP/separator entries
        const auto info = Camera::CameraRegistry::instance().byId(id);
        if (info && info->facing == want) {
            if (i != m_cameraSelector->currentIndex()) {
                qCInfo(lcCamera) << "Double-tap flip →"
                                 << (want == Camera::Facing::Front ? "front" : "rear")
                                 << "device" << id;
                m_cameraSelector->setCurrentIndex(i);  // fires onSelectorChanged
            }
            return;
        }
    }
    qCInfo(lcCamera) << "Double-tap flip: no"
                     << (want == Camera::Facing::Front ? "front" : "rear")
                     << "device available — ignoring";
}

// V32.34+ AF UI: refresh the persistent "AF" / "AF —" badge based on the
// CURRENT camera's known focus support + VCM open state. Three states:
//   1. Camera unknown / probing in registry → hide the badge entirely.
//      Showing a transient "AF —" during a 200ms probe window is more
//      annoying than informative.
//   2. Camera known and supportsFocus=false (front/ultrawide) → "AF —"
//      in grey to communicate the affordance is genuinely unavailable.
//      Users would otherwise tap repeatedly wondering why nothing
//      happens.
//   3. Camera known and supportsFocus=true → "AF" in cyan. The VCM may
//      still be opening async — that's fine, the reticle handler will
//      flip to Failed if the user taps before it's ready, and we'll
//      re-call this after open completes (the m_vcm pointer flip is
//      what triggers the upgrade from "—" to "ready").
//
// Positioned bottom-left of the viewfinder, above the bottom UI bar.
// Repositioned on resize via the existing eventFilter resize hook.
void CameraWidget::updateAfBadge()
{
    if (!m_afBadge) return;
    const QString currentId = m_cameraSelector
                                  ? m_cameraSelector->currentData().toString()
                                  : QString();
    if (currentId.isEmpty() || currentId == QLatin1String(kPipSentinel)
        || currentId == QLatin1String(kPipExtSentinel)) {
        m_afBadge->hide();
        return;
    }
    const auto info = Camera::CameraRegistry::instance().byId(currentId);
    if (!info) {
        m_afBadge->hide();
        return;
    }
    const bool focusSupported = info->supportsFocus;
    const bool vcmReady       = (m_vcm && m_vcm->isOpen());
    if (focusSupported) {
        m_afBadge->setText(vcmReady ? QStringLiteral("AF") : QStringLiteral("AF …"));
        m_afBadge->setStyleSheet(QStringLiteral(
            "background: rgba(0,0,0,0.55); color: %1;"
            "padding: 3px 8px; border-radius: 8px;"
            "font-weight: 600; font-size: 11px; letter-spacing: 0.5px;")
            .arg(QLatin1String(vcmReady ? "#78dcff" : "#cfcfcf")));
    } else {
        m_afBadge->setText(QStringLiteral("AF —"));
        m_afBadge->setStyleSheet(QStringLiteral(
            "background: rgba(0,0,0,0.55); color: rgba(255,255,255,0.55);"
            "padding: 3px 8px; border-radius: 8px;"
            "font-weight: 600; font-size: 11px; letter-spacing: 0.5px;"));
    }
    m_afBadge->adjustSize();
    if (m_viewfinderLabel) {
        const int x = 12;
        const int y = m_viewfinderLabel->height() - m_afBadge->height() - 12;
        m_afBadge->move(x, qMax(12, y));
    }
    m_afBadge->show();
    m_afBadge->raise();
}

// ─── V32.32 / V32.34+ wiring: landmark candidate scoring ──────────────────
//
// runLandmarkScoreTick fires once per second from m_landmarkScoreTimer while
// the camera page is visible. It:
//   1. Reads the cached latest preview frame under m_frameMutex.
//   2. Skips if recording (the user is focused on the active shot) or
//      if a previous score is still in flight.
//   3. Downsamples to 256×256 for the Rust scorer (constant work
//      regardless of source resolution; the scorer also does its own
//      grid sampling but starting from a 256² source keeps the per-tick
//      input pass bounded).
//   4. Dispatches sc_landmark_score on a worker thread; on completion,
//      if the score crosses kLandmarkHintThreshold AND the scene's
//      blurHash isn't in the recently-dismissed cooldown table, shows
//      the "+ Save as landmark" pill and stashes the frame for the
//      tap handler.
//
// Wrapped in #ifdef SNAPCLONE_RUST_SALIENCY so builds without the Rust
// crate skip the work entirely. Without that flag, the hint never shows
// and the feature is effectively off — a downgrade, not a build break.
void CameraWidget::runLandmarkScoreTick()
{
#ifdef SNAPCLONE_RUST_SALIENCY
    // V32.34+: opt-out via QSettings camera/landmarkHint/enabled=false.
    // Users who don't want the affordance can disable it without a
    // rebuild. Default ON. Read on every tick rather than cached because
    // (a) QSettings is fast, (b) we want toggling to take effect within
    // 1 second without restarting the camera, and (c) the tick itself
    // is already 1Hz so the per-tick cost dominates this read by orders
    // of magnitude.
    {
        QSettings s;
        if (!s.value(QStringLiteral("camera/landmarkHint/enabled"), true).toBool()) {
            // Also tear down a visible pill if the user just toggled off.
            if (m_landmarkHint && m_landmarkHint->isVisible()) {
                m_landmarkHint->hide();
                if (m_landmarkHintAutoHide) m_landmarkHintAutoHide->stop();
            }
            return;
        }
    }
    // Bail conditions: not active, no fresh frame, currently recording,
    // or a prior score is still resolving. The in-flight guard ensures
    // we don't queue work faster than it completes on slow devices.
    if (!m_backend || !m_backend->isActive()) return;
    if (m_recorder && m_recorder->isRecording()) return;
    if (m_landmarkScoreInFlight) return;

    QImage frame;
    {
        QMutexLocker lock(&m_frameMutex);
        if (m_latestFrame.isNull()) return;
        frame = m_latestFrame;  // implicit-shared copy
    }
    m_landmarkScoreInFlight = true;

    // Downsample to 256×256 on the worker — the score is scale-invariant
    // for our heuristics (we re-grid internally anyway), and a constant
    // input size makes the per-tick cost predictable. Convert to RGBA8888
    // since that's what the Rust FFI expects.
    auto fut = QtConcurrent::run([frame]() -> float {
        const QImage small = frame
            .scaled(256, 256, Qt::KeepAspectRatio, Qt::FastTransformation)
            .convertToFormat(QImage::Format_RGBA8888);
        if (small.isNull()) return 0.0f;
        return ::sc_landmark_score(small.constBits(),
                                    static_cast<uint32_t>(small.width()),
                                    static_cast<uint32_t>(small.height()));
    });

    auto *watcher = new QFutureWatcher<float>(this);
    connect(watcher, &QFutureWatcher<float>::finished, this,
            [this, watcher, frame]() {
        const float score = watcher->result();
        watcher->deleteLater();
        m_landmarkScoreInFlight = false;
        qCDebug(lcCamera) << "Landmark candidate score:" << score;

        if (score < kLandmarkHintThreshold) {
            // Score dropped below threshold while pill is showing →
            // start the auto-hide timer if it isn't running. The pill
            // continues to show for a moment so users can still tap it
            // even on a slight viewfinder jiggle.
            return;
        }
        // Cooldown check: if this scene was recently dismissed, don't
        // bother the user again. The fingerprint is the blurHash of the
        // current frame; comparison is exact-hex (no Hamming distance
        // here — that would be too lenient for "did the user just say
        // no to this exact view").
        const QByteArray blur = SnapClone::Core::LandmarkHash::blurHash(frame);
        const QString blurHex = QString::fromLatin1(blur.toHex());
        const qint64 now = QDateTime::currentMSecsSinceEpoch();
        // Sweep expired cooldown entries while we're here. The hash
        // table is small (usually <5) so this is sub-microsecond.
        for (auto it = m_landmarkHintCooldown.begin();
             it != m_landmarkHintCooldown.end();) {
            if (now - it.value() > kLandmarkHintCooldownMs) {
                it = m_landmarkHintCooldown.erase(it);
            } else {
                ++it;
            }
        }
        if (m_landmarkHintCooldown.contains(blurHex)) {
            return;
        }

        // All clear — surface the pill. Stash the frame the score was
        // computed against so onLandmarkHintTapped uses exactly that
        // (the live frame at tap-time would be a moment later).
        if (m_landmarkHint && !m_landmarkHint->isVisible()) {
            m_landmarkHintFrame = frame;
            // Position bottom-center of the viewfinder. updateAfBadge-
            // style logic but anchored bottom-center rather than corner.
            if (m_viewfinderLabel) {
                m_landmarkHint->adjustSize();
                const int x = (m_viewfinderLabel->width()
                               - m_landmarkHint->width()) / 2;
                const int y = m_viewfinderLabel->height()
                              - m_landmarkHint->height() - 24;
                m_landmarkHint->move(qMax(8, x), qMax(8, y));
            }
            m_landmarkHint->show();
            m_landmarkHint->raise();
        }
        if (m_landmarkHintAutoHide) m_landmarkHintAutoHide->start();
    });
    watcher->setFuture(fut);
#endif
}

void CameraWidget::onLandmarkHintTapped()
{
    if (!m_landmarkHint) return;
    m_landmarkHint->hide();
    if (m_landmarkHintAutoHide) m_landmarkHintAutoHide->stop();
    if (m_landmarkHintFrame.isNull()) return;
    if (!m_controller) return;

    // Add to cooldown so we don't immediately re-show after the dialog
    // closes (the same view is still in the viewfinder and would score
    // above threshold again on the very next tick). 1-hour window means
    // the user can revisit the same view later in the day without
    // having explicitly saved it the first time.
    const QByteArray blur = SnapClone::Core::LandmarkHash::blurHash(m_landmarkHintFrame);
    if (!blur.isEmpty()) {
        const QString blurHex = QString::fromLatin1(blur.toHex());
        m_landmarkHintCooldown.insert(blurHex,
                                       QDateTime::currentMSecsSinceEpoch());
    }

    // Open AddLandmarkDialog with current location. The frame's blur+edge
    // hashes will be computed and attached by the dialog (extended in a
    // follow-up; V31 dialog ignores the frame). Pass it via signal/slot
    // for forward compat — the dialog can refuse it if unsupported.
    const Core::UserId selfId{ m_controller->stableId() };
    const auto loc = m_controller->lastKnownLocation(selfId);
    const double lat = loc ? loc->lat       : qQNaN();
    const double lon = loc ? loc->lon       : qQNaN();
    const double acc = loc ? loc->accuracyM : 0.0;

    auto *dialog = new AddLandmarkDialog(selfId, lat, lon, acc, this);
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    // V32.34+: pass the frame so the dialog can compute photo hashes.
    // Older AddLandmarkDialog without setCapturedFrame just ignores
    // this — the slot is added in this iteration; missing slot is a
    // compile error caught at build time, not a runtime failure.
    dialog->setCapturedFrame(m_landmarkHintFrame);
    connect(dialog, &AddLandmarkDialog::saved, this,
            [this](Core::Landmark lm) {
        if (!m_controller || !m_controller->storageManager()) return;
        if (m_controller->storageManager()->saveLandmark(lm)) {
            qCInfo(lcCamera) << "Landmark saved via viewfinder hint:" << lm.id;
        }
    });
    dialog->open();
}

void CameraWidget::runFastFocusSweep()
{
    // V32.32 (revision): async CDAF sweep.
    //
    // BEFORE: ran the 5-step sweep synchronously on the GUI thread,
    // blocking input for ~750ms. During that window, touch events
    // buffered up in Qt's queue, and a single physical tap could
    // deliver as two TouchBegins after the thread unblocked — a
    // "ghost tap." User-visible symptom: jittery focus / second
    // focus immediately after first.
    //
    // NOW: dispatch the sweep to QtConcurrent's global thread pool.
    // The worker:
    //   - Reads m_latestFrame under m_frameMutex
    //   - Drives the VCM via ioctl (microseconds per call, no race
    //     because m_focusInProgress=true blocks GUI-thread re-entry
    //     and onSceneChangeTick checks the same flag)
    //   - Returns a CdafResult
    // Then a QFutureWatcher::finished signal fires on the GUI thread
    // and we read the result + update baseline atomically.
    //
    // m_focusInProgress is std::atomic<bool>: GUI thread checks it
    // (re-entry guard, scene-tick guard), worker doesn't touch it
    // (worker starts only AFTER the GUI thread successfully CAS'd it
    // to true, and the watcher clears it on the GUI thread).

    // Re-entry guard: if a sweep is already in flight, drop this one.
    // We use a CAS to atomically check-and-set so there's no window
    // where two GUI-thread callers (eventFilter + sceneChangeTick)
    // could both pass the check.
    bool expected = false;
    if (!m_focusInProgress.compare_exchange_strong(expected, true)) {
        return;
    }

    if (!m_vcm || !m_vcm->isOpen()) {
        // VCM not available — could be still warming up, could be the
        // front camera (no AF), could be a system without root for the
        // rebind. Silent no-op; release the in-progress flag.
        m_focusInProgress = false;
        // V32.34+ AF UI: if a reticle was just shown for a tap, flip to
        // Failed so the user gets visual confirmation that focus isn't
        // available — better than leaving it cyan-pulsing forever.
        if (m_focusReticle && m_focusReticle->isVisible()) {
            m_focusReticle->setState(Camera::FocusReticle::State::Failed);
        }
        return;
    }

    // Capture by raw pointer; the VCM is owned by this widget and not
    // torn down while m_focusInProgress is true (see onSelectorChanged
    // which waits for the watcher before destroying VCM).
    Camera::VcmController *vcm = m_vcm.get();

    // V32.35+: ANALYZE BEFORE MOVING. The cheap part of autofocus (measuring
    // current-frame sharpness) is separable from the expensive, visible part
    // (driving the lens motor). If the frame is ALREADY sharp, moving the lens
    // would only risk a needless focus pull. We reuse m_focusBaseline — the
    // sharpness recorded after the last GOOD focus, the SAME signal the
    // scene-change re-focus uses — rather than inventing an absolute threshold
    // (laplacianSharpness is an unnormalized variance; its scale is scene- and
    // resolution-dependent, so a fixed cutoff would be a guess). If we have a
    // baseline and the current frame is at/above ~90% of it, we're still in
    // focus: confirm the reticle and skip the motor sweep entirely.
    if (m_focusBaseline > 0.0) {
        QImage cur;
        {
            QMutexLocker lock(&m_frameMutex);
            cur = m_latestFrame;
        }
        if (!cur.isNull()) {
            const double now = Camera::laplacianSharpness(cur);
            constexpr double kAlreadySharpFrac = 0.90;
            if (now >= m_focusBaseline * kAlreadySharpFrac) {
                qCInfo(lcCamera) << "Tap focus: frame already sharp (" << now
                                 << ">= " << (m_focusBaseline * kAlreadySharpFrac)
                                 << ") — skipping lens sweep";
                if (m_focusReticle && m_focusReticle->isVisible()) {
                    m_focusReticle->setState(Camera::FocusReticle::State::Locked);
                }
                m_focusInProgress = false;
                return;
            }
        }
    }

    // The worker reads m_latestFrame through this guarded getter. Each
    // call locks the mutex, copies the QImage handle (refcount bump),
    // returns. The copy outlives the lock release — safe.
    auto frameGetter = [this]() -> QImage {
        QMutexLocker lock(&m_frameMutex);
        return m_latestFrame; // implicit-shared copy
    };

    // V32.35+: clear any stale abort from a previous sweep before arming.
    m_focusAbort = false;

    auto future = QtConcurrent::run([vcm, frameGetter, this]() -> Camera::CdafResult {
        // The worker polls m_focusAbort before each lens move (thread-safe
        // atomic read). A double-tap flip sets it, stopping the motor early.
        return Camera::runFastFocus(*vcm, frameGetter, /*settleMs=*/100,
                                    [this]() { return m_focusAbort.load(); });
    });

    auto *watcher = new QFutureWatcher<Camera::CdafResult>(this);
    connect(watcher, &QFutureWatcher<Camera::CdafResult>::finished,
            this, [this, watcher]() {
        const Camera::CdafResult result = watcher->result();
        watcher->deleteLater();
        m_focusInProgress = false;
        // V32.32: capture the post-focus sharpness as the new baseline.
        // Done on the GUI thread so m_focusBaseline / m_sceneChangeTimer
        // are accessed only here and in onSceneChangeTick (also GUI).
        if (result.succeeded && result.finalSharpness > 0.0) {
            m_focusBaseline = result.finalSharpness;
            if (!m_sceneChangeTimer) {
                m_sceneChangeTimer = new QTimer(this);
                m_sceneChangeTimer->setInterval(1500);
                connect(m_sceneChangeTimer, &QTimer::timeout,
                        this, &CameraWidget::onSceneChangeTick);
            }
            if (!m_sceneChangeTimer->isActive()) m_sceneChangeTimer->start();
        }
        // V32.34+ AF UI: update reticle state. Only if the reticle is
        // currently visible — auto-refocus (scene-change tick) doesn't
        // call triggerAt(), so the reticle stays hidden and setState()
        // is a no-op for those. Manual taps DO show it, and now it
        // transitions to Locked or Failed for visible confirmation.
        if (m_focusReticle && m_focusReticle->isVisible()) {
            using Reticle = Camera::FocusReticle;
            m_focusReticle->setState(result.succeeded
                ? Reticle::State::Locked
                : Reticle::State::Failed);
        }
    });
    watcher->setFuture(future);
}

// V32.32: scene-change detection tick. Runs at 1.5s intervals (slow
// enough not to compete with the user, fast enough to react before the
// user gives up and taps to refocus manually). Samples current
// sharpness, compares to the post-focus baseline. If sharpness has
// dropped to less than kReFocusThreshold of baseline, the subject has
// likely moved — kick off another fast focus sweep.
//
// Threshold tuning:
//   - 0.5 = aggressive (refocuses on minor changes)
//   - 0.6 = balanced (default — refocus on subject motion)
//   - 0.8 = conservative (only refocus on dramatic changes)
//
// Skipped when focus is locked (user opt-out) or recording video
// (focus pulls are disruptive mid-record).
void CameraWidget::onSceneChangeTick()
{
    if (m_focusLocked) return;
    if (m_focusInProgress.load()) return;
    if (m_recorder && m_recorder->isRecording()) return;
    if (!m_vcm || !m_vcm->isOpen()) return;

    // V32.32: read the cached frame under m_frameMutex. The worker
    // thread may also be reading it concurrently — both are read-only
    // accesses, but the QImage HANDLE itself can be reassigned by
    // onPreviewFrame on the GUI thread, so we lock around the copy.
    QImage frame;
    {
        QMutexLocker lock(&m_frameMutex);
        frame = m_latestFrame;
    }
    if (frame.isNull()) return;

    const double current = Camera::laplacianSharpness(frame);
    if (m_focusBaseline < 1.0) return;  // no baseline yet
    constexpr double kReFocusThreshold = 0.6;
    if (current < m_focusBaseline * kReFocusThreshold) {
        // Sharpness collapsed — likely subject moved, re-focus.
        QMetaObject::invokeMethod(this, "runFastFocusSweep",
                                  Qt::QueuedConnection);
    }
}

} // namespace SnapClone::UI
