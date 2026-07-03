// ─── UI/AmbientCameraDriver.cpp ──────────────────────────────────────────────
// V32.30 — see header for design philosophy.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/AmbientCameraDriver.hpp"
#include "ui/AmbientPaletteDeriver.hpp"
#include "ui/Theme.hpp"
#include "ui/CameraWidget.hpp"

#include <QApplication>
#include <QTimer>
#include <QLoggingCategory>
#include <QtConcurrent/QtConcurrentRun>
#include <QFuture>
#include <QFutureWatcher>

namespace {
Q_LOGGING_CATEGORY(lcAmbCam, "snapclone.ui.ambient.camera")
}

namespace SnapClone::UI {

AmbientCameraDriver::AmbientCameraDriver(QObject *parent)
    : QObject(parent)
{
    m_timer = new QTimer(this);
    m_timer->setInterval(2000); // V32.32: 2 Hz → 0.5 Hz to reduce
        // qApp->setStyleSheet pressure during heavy camera+ambient load.
        // Each re-derive triggers an app-wide re-style which repaints
        // every widget. 1Hz was visibly heavy on slower devices; 2s
        // gives the eye enough motion to feel "alive" without burning
        // CPU. EMA smoothing (kSmoothingAlpha=0.15) means each derive
        // contributes ~15% of the new palette anyway, so dropping the
        // rate just stretches the same exponential to a longer
        // ~8.7s half-life rather than 4.3s — still subjectively
        // smooth.
    connect(m_timer, &QTimer::timeout, this, &AmbientCameraDriver::onDeriveTick);

    // V32.34+ battery: gate the derive timer on app-foreground state.
    // Without this, the timer keeps firing every 2s while the app is
    // backgrounded; each fire dispatches the sampler to the threadpool
    // and triggers a Theme::applyGlobalStyleSheetIfChanged when the
    // worker returns. Wasteful when no pixels reach the screen.
    //
    // ApplicationActive       → on screen, sample.
    // ApplicationInactive     → just lost focus — keep sampling (user
    //                            likely returning).
    // ApplicationSuspended    → backgrounded — stop the timer.
    // ApplicationHidden       → window hidden — stop.
    if (auto *gui = qobject_cast<QGuiApplication *>(QGuiApplication::instance())) {
        m_appActive = (gui->applicationState() == Qt::ApplicationActive
                    || gui->applicationState() == Qt::ApplicationInactive);
        connect(gui, &QGuiApplication::applicationStateChanged,
                this, [this](Qt::ApplicationState s) {
            const bool nowActive = (s == Qt::ApplicationActive
                                  || s == Qt::ApplicationInactive);
            if (nowActive == m_appActive) return;
            m_appActive = nowActive;
            if (!m_timer) return;
            if (m_appActive && m_active) {
                m_timer->start();
            } else {
                m_timer->stop();
            }
        });
    }
}

AmbientCameraDriver::~AmbientCameraDriver() = default;

void AmbientCameraDriver::start(CameraWidget *cam)
{
    if (!cam) {
        qCWarning(lcAmbCam) << "start: nullptr camera widget — ignored";
        return;
    }
    if (m_cam && m_cam.data() != cam) {
        // Retarget: disconnect from previous before binding to new.
        disconnect(m_cam.data(), nullptr, this, nullptr);
    }
    m_cam = cam;
    // Connect to the public re-emit signal added to CameraWidget for
    // exactly this purpose (V32.30). UniqueConnection so a duplicate
    // start() doesn't double-fire.
    connect(cam, &CameraWidget::previewFrameAvailable,
            this, &AmbientCameraDriver::onFrame,
            Qt::UniqueConnection);
    m_active = true;
    // V32.34+: only start the derive timer when the app is on screen.
    // start() called while backgrounded will queue m_active=true and
    // the app-state listener will resume the timer on foreground.
    if (m_appActive) m_timer->start();
    qCInfo(lcAmbCam) << "started — sampling camera viewfinder at 1Hz";
}

void AmbientCameraDriver::stop()
{
    if (!m_active) return;
    m_timer->stop();
    if (m_cam) disconnect(m_cam.data(), nullptr, this, nullptr);
    m_active = false;
    m_latestFrame = QImage();
    // V32.30 (Wave 2): reset EMA state so a re-activation seeds from
    // raw values on its first tick rather than blending against
    // stale colors from a prior session.
    m_smoothedAccent     = QColor();
    m_smoothedBackground = QColor();
    m_smoothedValid      = false;
    qCInfo(lcAmbCam) << "stopped";
}

void AmbientCameraDriver::onFrame(const QImage &frame)
{
    // Cheap shallow-copy — QImage is implicitly shared. We DON'T derive
    // here — the timer drives derivation to enforce the 1Hz cap.
    m_latestFrame = frame;
}

void AmbientCameraDriver::onDeriveTick()
{
    if (!m_active) return;
    if (m_latestFrame.isNull()) return; // no frame yet
    if (!Theme::isSyntheticActive()) {
        // The user switched away from a synthetic palette mid-session.
        // We could stop ourselves here, but the click handler in
        // SettingsPage already does stop() explicitly. Belt+braces:
        // skip this tick to avoid stomping their static pick.
        return;
    }
    // V32.34+: prevent stacked derives. If the previous run hasn't
    // landed yet (device under heavy load, derive took >2s), skip this
    // tick. The next one will pick up the latest frame. Without this
    // guard, slow devices could accumulate workers and drive the
    // threadpool into queue-saturation.
    if (m_deriveInFlight) {
        return;
    }

    // V32.34+: derive on a worker thread. The QImage shallow-copies
    // cheaply into the worker (Qt's implicit sharing); the worker can
    // touch its pixel data freely because m_latestFrame uses copy-on-
    // write semantics — if we overwrite m_latestFrame on the next
    // onFrame call, the worker's copy stays alive on its own ref.
    m_deriveInFlight = true;
    const QImage frameForWorker = m_latestFrame;  // shallow copy

    auto *watcher = new QFutureWatcher<AmbientPaletteDeriver::AmbientColors>(this);
    connect(watcher, &QFutureWatcher<AmbientPaletteDeriver::AmbientColors>::finished,
            this, [this, watcher]() {
        watcher->deleteLater();
        m_deriveInFlight = false;
        if (!m_active) return;
        if (!Theme::isSyntheticActive()) return;  // user moved on

        const auto colors = watcher->result();
        if (!colors.accent.isValid() && !colors.background.isValid()) {
            return;
        }

        // EMA blending. Lerp in HSL space rather than RGB so a swing from
        // green to magenta doesn't pass through grey at the midpoint —
        // HSL gives a smoother perceptual transition.
        auto emaBlend = [](const QColor &fresh, const QColor &prior) -> QColor {
            if (!fresh.isValid()) return prior;
            if (!prior.isValid()) return fresh;
            float fH, fS, fL, fA, pH, pS, pL, pA;
            fresh.getHslF(&fH, &fS, &fL, &fA);
            prior.getHslF(&pH, &pS, &pL, &pA);
            if (fH < 0.0f) fH = 0.0f; if (fH > 1.0f) fH = 1.0f;
            if (pH < 0.0f) pH = 0.0f; if (pH > 1.0f) pH = 1.0f;
            float diff = fH - pH;
            if (diff > 0.5f)  pH += 1.0f;
            else if (diff < -0.5f) fH += 1.0f;
            float h = pH + kSmoothingAlpha * (fH - pH);
            if (h >= 1.0f) h -= 1.0f;
            if (h < 0.0f)  h += 1.0f;
            const float s = pS + kSmoothingAlpha * (fS - pS);
            const float l = pL + kSmoothingAlpha * (fL - pL);
            return QColor::fromHslF(qBound(0.0f, h, 1.0f),
                                    qBound(0.0f, s, 1.0f),
                                    qBound(0.0f, l, 1.0f));
        };

        if (!m_smoothedValid) {
            m_smoothedAccent     = colors.accent;
            m_smoothedBackground = colors.background;
            m_smoothedValid      = true;
        } else {
            m_smoothedAccent     = emaBlend(colors.accent,     m_smoothedAccent);
            m_smoothedBackground = emaBlend(colors.background, m_smoothedBackground);
        }

        const Theme::Palette synth = AmbientPaletteDeriver::fromColors(
            m_smoothedAccent, m_smoothedBackground, Theme::activePalette());
        Theme::setSyntheticPalette(synth, "ambient_camera", "Ambient: Camera");
        Theme::applyGlobalStyleSheetIfChanged();
    });

    // Run the sampler off the GUI thread. AmbientPaletteDeriver::deriveColors
    // is a pure function (no shared state), so this is safe.
    QFuture<AmbientPaletteDeriver::AmbientColors> fut =
        QtConcurrent::run([frameForWorker]() {
            return AmbientPaletteDeriver::deriveColors(frameForWorker);
        });
    watcher->setFuture(fut);
}

} // namespace SnapClone::UI
