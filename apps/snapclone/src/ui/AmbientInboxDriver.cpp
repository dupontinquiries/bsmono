// ─── UI/AmbientInboxDriver.cpp ───────────────────────────────────────────────
// V32.30 — see header.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/AmbientInboxDriver.hpp"
#include "ui/AmbientPaletteDeriver.hpp"
#include "ui/Theme.hpp"
#include "core/AppController.hpp"
#include "storage/StorageManager.hpp"

#include <QApplication>
#include <QLoggingCategory>

namespace {
Q_LOGGING_CATEGORY(lcAmbInbox, "snapclone.ui.ambient.inbox")
}

namespace SnapClone::UI {

AmbientInboxDriver::AmbientInboxDriver(QObject *parent)
    : QObject(parent)
{
}

AmbientInboxDriver::~AmbientInboxDriver() = default;

void AmbientInboxDriver::start(Core::AppController *controller)
{
    if (!controller) {
        qCWarning(lcAmbInbox) << "start: nullptr controller — ignored";
        return;
    }
    // V32.33 fix: guard against duplicate start() calls.
    //
    // Symptom observed in Phone B logs 2026-05-15: two `started` lines
    // logged within 2 seconds, each followed by a derivation, despite
    // the 30s throttle. The throttle is per-snap-received; it doesn't
    // protect against start() being called multiple times. The fix:
    // if we're already active for the same controller, treat the
    // second start() as a no-op — don't re-derive, don't re-connect.
    //
    // Without this, anything that emits ambientInboxRequested(true)
    // more than once (palette restore + carousel click; double-tap on
    // the chip; whatever) causes redundant derivation work that drowns
    // perf signal in the logs.
    if (m_active && m_controller.data() == controller) {
        qCDebug(lcAmbInbox) << "start: already active for this controller, skipping";
        return;
    }
    if (m_controller && m_controller.data() != controller) {
        disconnect(m_controller.data(), nullptr, this, nullptr);
    }
    m_controller = controller;
    // Direct connection — snapReceived fires on the GUI thread (verified
    // via the V32.28 audit: SnapRouter::inboundReady → AppController →
    // snapReceived is all main-thread).
    connect(controller, &Core::AppController::snapReceived,
            this, &AmbientInboxDriver::onSnapReceived,
            Qt::UniqueConnection);
    m_active = true;
    // Reset throttle so the initial derivation runs unthrottled — the
    // user just picked Ambient: Inbox, they expect immediate effect.
    m_lastDeriveMs = 0;
    deriveNow();
    qCInfo(lcAmbInbox) << "started — auto-refresh on snapReceived "
                          "(throttle:" << kMinIntervalMs << "ms)";
}

void AmbientInboxDriver::stop()
{
    if (!m_active) return;
    if (m_controller) disconnect(m_controller.data(), nullptr, this, nullptr);
    m_active = false;
    qCInfo(lcAmbInbox) << "stopped";
}

void AmbientInboxDriver::onSnapReceived(Core::Snap /*snap*/)
{
    if (!m_active) return;
    // Throttle check. Burst of N snaps within kMinIntervalMs ms → ONE
    // derivation at the start (when m_lastDeriveMs is older than threshold)
    // and zero more until the window passes.
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    if (now - m_lastDeriveMs < kMinIntervalMs) {
        qCDebug(lcAmbInbox) << "throttled — last derive"
                            << (now - m_lastDeriveMs) << "ms ago";
        return;
    }
    // Safety guard: user may have switched to a different synthetic
    // (ambient_camera) or a static palette since we started. Don't
    // stomp their pick. The synthetic-id check is the canonical way
    // to verify which mode is in charge.
    if (Theme::isSyntheticActive()) {
        const QString currentId = QString::fromUtf8(
            Theme::syntheticStorage().id.c_str());
        if (currentId != QStringLiteral("ambient_inbox")) {
            qCDebug(lcAmbInbox) << "synthetic id is"
                                << currentId
                                << "— not ambient_inbox, skipping";
            return;
        }
    } else {
        // V32.33+ Phase 1 audit: previously the fall-through here
        // would derive a NEW ambient_inbox palette and re-establish
        // it as the synthetic — overriding the user's choice. The
        // original comment said to skip silently to avoid "racing
        // the cleanup", but the actual flow was running deriveNow
        // anyway. Now we actually skip.
        qCDebug(lcAmbInbox) << "no synthetic active — user reverted to "
                               "static palette; skipping derive";
        return;
    }
    deriveNow();
}

void AmbientInboxDriver::deriveNow()
{
    if (!m_controller || !m_controller->storageManager()) return;
    const Core::UserId selfId{ m_controller->stableId() };
    const auto inbox = m_controller->storageManager()->loadInboxSnaps(selfId);
    QStringList paths;
    for (const auto &s : inbox) {
        if (s.localPath.isEmpty()) continue;
        if (s.hasVideo()) continue;
        paths.append(s.localPath);
    }
    if (paths.isEmpty()) {
        qCDebug(lcAmbInbox) << "no inbox photos — skipping derivation";
        return;
    }
    // Derive against the CURRENTLY-active palette so iterative refreshes
    // build on the previous synthetic (matches the AmbientCameraDriver
    // chaining model). On first call right after start(), this is
    // whatever static palette was active before the user picked
    // Ambient: Inbox.
    const Theme::Palette base = Theme::activePalette();
    const Theme::Palette synth = AmbientPaletteDeriver::fromImagePaths(paths, base);
    Theme::setSyntheticPalette(synth, "ambient_inbox", "Ambient: Inbox");
    // V32.32: dedup via Theme::applyGlobalStyleSheetIfChanged — if the
    // derived palette is identical to the last-applied sheet (common
    // when the inbox content hasn't changed since last derive), skip
    // the expensive qApp->setStyleSheet entirely.
    Theme::applyGlobalStyleSheetIfChanged();
    m_lastDeriveMs = QDateTime::currentMSecsSinceEpoch();
    qCInfo(lcAmbInbox) << "derived from" << paths.size() << "inbox photos";
}

} // namespace SnapClone::UI
