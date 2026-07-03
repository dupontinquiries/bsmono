#include "ui/MainWindow.hpp"
#include "ui/LoginWidget.hpp"
#include "ui/InboxWidget.hpp"
#include "network/NetworkManager.hpp"   // V32.39+: voice payload → sendToPeer
#include "ui/MonthlyExportReminder.hpp"  // V32.34+: monthly export gate
#include "ui/SnapExporter.hpp"           // V32.34+: zip/tar export action
#include "ui/ContactPickerDialog.hpp"
#include "ui/SnapComposerDialog.hpp"
#include "core/MediaMetadataStripper.hpp"   // V32.39+: strip EXIF/GPS before send
#include "ui/MediaSendSummary.hpp"          // V32.39+: post-send privacy/size receipt
#include <QDialog>
#include <QTimer>
#include "ui/CameraWidget.hpp"
#include "ui/ContactsPage.hpp"
#include "network/PollinatorDiscovery.hpp"
#include "ui/DiagnosticsDialog.hpp"
#include "ui/SwipeNavigator.hpp"
#include "ui/posture/PostureController.hpp"
#include "ui/UnifiedNavBar.hpp"   // V32.34+: unified bottom bar
#include "ui/TabBar.hpp"
#include "ui/SettingsPage.hpp"
#include "ui/wayfinder/WayfinderPage.hpp"
#include "ui/ComponentTestWindow.hpp"
#include "ui/Theme.hpp"
#include "ui/PagePeelHint.hpp"
#include "ui/AmbientBlobs.hpp"   // V32.30: pulsing color overlay
#include "ui/ModalSurface.hpp"   // V32.32: layered overlay/dialog manager
#include "ui/ModalLayerTestPanel.hpp" // V32.32: ModalSurface test harness
#include "ui/AmbientCameraDriver.hpp"   // V32.30: live-viewfinder palette
#include "ui/AmbientInboxDriver.hpp"    // V32.30 wave 2: auto-refresh inbox
#include "core/AppController.hpp"
#include "core/AppContext.hpp"  // V32.21: complete type needed for appContext()-> calls
#include "core/SessionManager.hpp"
#include "storage/StorageManager.hpp"

#include <QPainter>
#include <QPainterPath>
#include <QPointer>
#include <QWindow>
#include <QFileDialog>
#include <QStandardPaths>  // V32.34+: DownloadLocation for export default
#include <QStackedWidget>
#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QStatusBar>
#include <QLoggingCategory>
#include <QIODevice>
#include <QDataStream>
#include <QBuffer>
#include <QLabel>
#include <QPushButton>
#include <QScreen>
#include <QFrame>
#include <QPropertyAnimation>
#include <QApplication>
#include <QGraphicsOpacityEffect>
#include <QAbstractAnimation>
#include <QAbstractButton>
#include <QSettings>
#include "ui/TouchFeedback.hpp"
#include "ui/SvgIcon.hpp"
#include "ui/UxGuide.hpp"
#include "ui/ChevronConfirm.hpp"
#include "ui/QrScanDialog.hpp"
#include "ui/MobileDialog.hpp"               // V32.34+: wayfinder action sheet
#include "ui/VirtualKeyboardWatcher.hpp"  // V32.34+: on-screen keyboard avoidance
#include <QAbstractScrollArea>            // V32.34+: ensureWidgetVisible walk-up
#include <QScrollArea>                    // V32.34+: ensureWidgetVisible convenience
#include <QScrollBar>                     // V32.34+: manual scroll for generic scroll areas
#include <QInputDevice>
#include <QTimer>
#include <QResizeEvent>
#include <QShowEvent>
#include <QFileInfo>

// ─── UI/MainWindow.cpp ────────────────────────────────────────────────────────
//
// Layout:
//   SwipeNavigator (central widget, fills window)
//     [0] Camera   [1] Inbox   [2] Friends   [3] Settings
//
// Login overlay:
//   A LoginWidget is created as a direct child of the QMainWindow (not the
//   swiper) and positioned to cover the entire central widget area.
//
//   Key ordering constraint:
//     • The overlay starts HIDDEN (Qt default).
//     • If auto-login already succeeded at construction time, we leave it
//       hidden and go straight to full-screen camera — no animation needed.
//     • If auto-login hasn't fired yet (shouldn't happen with current flow
//       but possible if initialization is deferred), we show the overlay and
//       hide it when loginSucceeded fires.
//
//   This avoids the previous bug where:
//     1. overlay was created visible (default for child widgets after show())
//     2. hideLoginOverlay() checked isVisible() before show() → returned false
//     3. Overlay appeared over camera on first paint
//
// Sizing:
//   SwipeNavigator::layoutPages() uses width()/height() which are 0 at
//   construction.  The pages get their real geometry in the first resizeEvent,
//   which fires reliably before the first paint.  We also call layoutPages()
//   explicitly from our resizeEvent override to ensure the overlay tracks.
// ─────────────────────────────────────────────────────────────────────────────

Q_LOGGING_CATEGORY(lcUi, "snapclone.ui")

namespace SnapClone::UI {

static constexpr int kPageCamera      = 0;
static constexpr int kPageInbox       = 1;
static constexpr int kPageFriends     = 2;
static constexpr int kPageWayfinder   = 3;  // NEW
static constexpr int kPageSettings    = 4;
// History: an earlier `kPagePollinator` constant (and matching PollinatorPage
// UI page) was removed when Nearby became an inline panel inside the Friends
// tab. The Friends page itself remains; the "Pollinator" name lingers only
// on the network-layer beacon class (PollinatorDiscovery) which is unrelated
// to UI page state.

// ─── Construction ─────────────────────────────────────────────────────────────


// ─── Touchscreen probe ────────────────────────────────────────────────────────

[[maybe_unused]] static bool hasTouchscreen()
{
    static int cached = -1;
    if (cached >= 0) return cached != 0;
    for (const QInputDevice *dev : QInputDevice::devices()) {
        if (dev->type() == QInputDevice::DeviceType::TouchScreen ||
            dev->type() == QInputDevice::DeviceType::TouchPad) {
            cached = 1; return true;
        }
    }
    if (isMobileFormFactor()) { cached = 1; return true; }
    cached = 0; return false;
}

// ─── MainWindow ───────────────────────────────────────────────────────────────


Q_LOGGING_CATEGORY(lcUi, "snapclone.ui")

MainWindow::MainWindow(Core::AppController *controller, QWidget *parent)
    : QMainWindow(parent)
    , m_controller(controller)
{
    setWindowTitle(QStringLiteral("SnapClone"));
    setMinimumSize(320, 480);
    buildUi();
    connectSignals();
    setStyleSheet(Theme::globalStyleSheet());

    // UxGuide lifecycle
    UxGuide::instance().onAppLaunched();
    QTimer::singleShot(1200, this, [this]() {
        UxGuide::instance().onSessionStarted();
    });
    // Show first-run swipe tutorial after the window is fully painted
    QTimer::singleShot(800, this, &MainWindow::showSwipeHintIfFirstRun);

    // Page-peel edge hint — always shown, fades after user learns to swipe
    QTimer::singleShot(2000, this, [this]() {
        const int swipes = UxGuide::instance()
            .familiarityFor(QLatin1String(Feature::kSwipe)).useCount;
        if (swipes < PagePeelHint::kMinSwipesBeforeHide) {
            m_edgeHint = new PagePeelHint(m_swiper);
            static_cast<PagePeelHint*>(m_edgeHint)->setPageState(kPageInbox, 4);
        }
    });

    // ── KDE/Kvantum blur-behind hint ─────────────────────────────────────────
#if defined(Q_OS_LINUX)
    if (QGuiApplication::platformName().contains(QLatin1String("xcb"))) {
        QWindow *nativeHandle = windowHandle();
        if (!nativeHandle && nativeParentWidget())
            nativeHandle = nativeParentWidget()->windowHandle();
        if (nativeHandle) {
            const QVariant blurProp = QVariant::fromValue(QRect(0, 0, 32767, 32767));
            nativeHandle->setProperty("_kde_net_wm_blur_behind_region", blurProp);
        }
    }
#endif
}

// V32.30: keep AmbientBlobs sized to match the wrapper QWidget. The
// blobs widget is installed as a child of `wrapper` in buildUi(); we
// catch QResizeEvent on wrapper here and propagate the new size.
bool MainWindow::eventFilter(QObject *watched, QEvent *event)
{
    if (event->type() == QEvent::Resize && m_ambientBlobs) {
        if (auto *w = qobject_cast<QWidget *>(watched)) {
            m_ambientBlobs->setGeometry(w->rect());
        }
    }
    return QMainWindow::eventFilter(watched, event);
}

void MainWindow::buildUi()
{
    // Create the wrapper FIRST so all children have a stable parent.
    // setCentralWidget is called exactly once with this wrapper.
    auto *wrapper = new QWidget(this);
    auto *wlay    = new QVBoxLayout(wrapper);
    wlay->setContentsMargins(0, 0, 0, 0);
    wlay->setSpacing(0);

    // ── SwipeNavigator (parented to wrapper) ─────────────────────────────────
    m_swiper = new SwipeNavigator(wrapper);

    // ── Pages (parented to swiper) ───────────────────────────────────────────
    m_cameraPage   = new CameraWidget(m_swiper);
    // V32.32: hand the controller in so the capture path can write
    // selfie face crops to the process-wide Face::Library for Phase 1
    // of the face-bandwidth project. Optional — without it the
    // capture path skips face work and behaves exactly as before.
    m_cameraPage->setController(m_controller);
    m_inboxPage    = new InboxWidget(m_swiper);
    m_friendsPage   = new ContactsPage(m_swiper);
    // V32.31: attach controller so ContactsPage can reach storage +
    // self-id for landmark creation. Mirrors the V32.29 SettingsPage
    // pattern.
    m_friendsPage->setController(m_controller);
    m_wayfinderPage = new WayfinderPage(m_controller, m_swiper);
    m_settingsPage  = new SettingsPage(m_swiper);
    // V32.29: attach controller so SettingsPage can reach storage for
    // the ambient-inbox theme derivation. Other settings features still
    // work without this; the controller is purely an enabler.
    m_settingsPage->setController(m_controller);

    m_swiper->addPage(m_cameraPage,    tr("Camera"));
    m_swiper->addPage(m_inboxPage,     tr("Inbox"));
    m_swiper->addPage(m_friendsPage,   tr("Friends"));
    m_swiper->addPage(m_wayfinderPage, tr("Wayfinder"));
    m_swiper->addPage(m_settingsPage,  tr("Settings"));
    m_swiper->goToPage(kPageInbox, /*animate=*/false);

    // ── TabBar (parented to wrapper, sibling of swiper) ──────────────────────
    m_tabBar = new TabBar(wrapper);
    m_tabBar->addTab(QStringLiteral("◉"), tr("Camera"));
    m_tabBar->setTabSvgIcon(0, SvgIcon::tabIcon(QStringLiteral("camera")));
    m_tabBar->addTab(QStringLiteral("◈"), tr("Inbox"));
    m_tabBar->setTabSvgIcon(1, SvgIcon::tabIcon(QStringLiteral("inbox")));
    m_tabBar->addTab(QStringLiteral("◎"), tr("Friends"));
    m_tabBar->setTabSvgIcon(2, SvgIcon::tabIcon(QStringLiteral("friends")));
    if (m_wayfinderPage) m_tabBar->addTab(QStringLiteral("◍"), tr("Wayfinder"));
    // No SVG icon for wayfinder yet — uses unicode ◍ glyph
    m_tabBar->addTab(QStringLiteral("◌"), tr("Settings"));
    m_tabBar->setTabSvgIcon(4, SvgIcon::tabIcon(QStringLiteral("settings")));
    m_tabBar->setCurrentIndex(kPageInbox);

    statusBar()->setSizeGripEnabled(false);

    // Layout: swiper takes all remaining vertical space, tab bar fixed at bottom
    wlay->addWidget(m_swiper, 1);
    // V32.34+ unified nav: when SC_UNIFIED_NAV is on, the bottom bar becomes the
    // morphing UnifiedNavBar (current page's sub-tabs inline + page arrows)
    // instead of the static page-icon TabBar. The TabBar is still constructed
    // (its badge/shimmer wiring is reused elsewhere) but hidden so the two
    // don't both show. The navigator's gesture detents drive the same model
    // the bar renders, so they always agree about "where am I".
    if (SwipeNavigator::unifiedNavEnabled()) {
        m_unifiedBar = new UnifiedNavBar(m_swiper, wrapper);
        m_tabBar->hide();
        wlay->addWidget(m_unifiedBar, 0);
        // V32.34+: keep the bar's sub-tab count pills live. The bar reads
        // counts from SubTabbed::subTabBadge() on paint, so it just needs a
        // repaint trigger when a page's counts change. Inbox exposes
        // unreadCountChanged; Contacts exposes badgesChanged (added for this).
        // Both feed the bar's refreshBadges() slot. (Pages without a
        // count-change signal simply update on tab/page switches as before.)
        if (m_inboxPage)
            connect(m_inboxPage, &InboxWidget::unreadCountChanged,
                    m_unifiedBar, [this](int) { m_unifiedBar->refreshBadges(); });
        if (m_friendsPage)
            connect(m_friendsPage, &ContactsPage::badgesChanged,
                    m_unifiedBar, &UnifiedNavBar::refreshBadges);
    } else {
        wlay->addWidget(m_tabBar, 0);
    }
    // V32.30: ambient color splotches. When the user picks a synthetic
    // palette (Settings → Theme → Ambient: Inbox), this overlay paints
    // 3 slowly-drifting radial-gradient blobs in the derived accent
    // colors, giving the calm background tones some pulse and breath.
    // The overlay is transparent-for-mouse-events so it doesn't intercept
    // any clicks; it's a pure visual layer. With a STATIC palette active,
    // paintEvent is a no-op (zero perf cost).
    //
    // Z-order: blobs sit ON TOP (raise_()) because the global stylesheet
    // gives every QWidget a BG_BASE fill — putting blobs underneath would
    // be obscured. Alpha is capped at 0.18 in paintEvent so even painted
    // over content the blobs stay subtle. WA_TransparentForMouseEvents
    // ensures clicks/taps still reach widgets underneath.
    //
    // Parented to wrapper, sized to match it via the resize event below.
    // showEvent on first display kicks off the 30fps timer.
    auto *blobs = new AmbientBlobs(wrapper);
    blobs->setGeometry(wrapper->rect());
    blobs->raise(); // z-stack: on top, painted with low alpha
    // Track wrapper resizes — keep blobs filling the wrapper canvas.
    wrapper->installEventFilter(this);
    m_ambientBlobs = blobs;

    // V32.42+: posture engine. Owns the dim overlay (parented to wrapper, so it
    // covers the swiper + nav bar + ambient blobs once raised), the proximity/
    // motion source, and the dim→screensaver state machine. It arms only in the
    // inbox / a conversation that's been voice-active in the last 5 minutes, and
    // re-emits the gated request, which we route to the open conversation's call.
    m_postureController = new PostureController(wrapper, m_swiper, this);
    if (m_inboxPage)
        connect(m_postureController, &PostureController::gatedModeRequested,
                m_inboxPage, &InboxWidget::applyVoiceGated);

    setCentralWidget(wrapper);

    // V32.32: ModalSurface — unified overlay/dialog/toast/confirm layer manager.
    // Parented to MainWindow (NOT the wrapper) so it sits at the same hierarchy
    // level as the login overlay and offline banner — all three are top-level
    // peers of the centralWidget. ModalSurface itself is invisible; its six
    // internal layer containers light up when widgets are presented into them.
    //
    // Phase 1 ships infrastructure + a test panel only. Real dialogs continue
    // to use MobileDialog. Migration of ContactEditorDialog, SnapComposerDialog,
    // etc. happens in subsequent turns once the infrastructure is proven on
    // hardware.
    m_modalSurface = new ModalSurface(this);
    m_modalSurface->setGeometry(rect());
    m_modalSurface->raise(); // above central widget, login overlay, offline banner
    m_modalSurface->show();

    // ── Login overlay (parented to MainWindow so it overlays everything) ──────
    m_loginOverlay = new LoginWidget(this);
    m_loginOverlay->hide();

    // Offline banner — shows at the top when network is unreachable.
    // Hidden by default; shown by networkReachabilityChanged(false) handler.
    m_offlineBanner = new QLabel(tr("⚠ Offline — snaps will queue until connection returns"), this);
    m_offlineBanner->setObjectName(QStringLiteral("offlineBanner"));
    m_offlineBanner->setAlignment(Qt::AlignCenter);
    m_offlineBanner->setStyleSheet(QStringLiteral(
        "background: %1; color: white;"
        "font-size: 13px; font-weight: 600;"
        "padding: 8px; border-radius: 0;")
        .arg(QLatin1String(Theme::WARNING())));
    m_offlineBanner->setAttribute(Qt::WA_TransparentForMouseEvents, true);
    m_offlineBanner->hide();
    repositionOfflineBanner();

    // V32.32 (revision 2): ModalSurface must end up at the TOP of
    // MainWindow's child stack. It was constructed earlier (line ~272)
    // but LoginWidget and m_offlineBanner are added AFTER it, and Qt
    // puts newer children on top. Re-raise here so the modal stack
    // is above everything else. The login overlay handles its own
    // visibility-vs-modal-stack via being hidden by default; if it
    // ever needs to overlay a modal it can call raise() itself
    // (none of the current login flows do this).
    if (m_modalSurface) m_modalSurface->raise();
}

void MainWindow::connectSignals()
{
    // Tab bar ↔ swiper sync
    connect(m_tabBar, &TabBar::tabSelected,
            this, [this](int idx) { m_swiper->goToPage(idx); });
    connect(m_swiper, &SwipeNavigator::pageChanged,
            m_tabBar, &TabBar::setCurrentIndex);
    // V32.38f: morph the shared bottom bar to the camera glass look ONLY while
    // the camera page is current; every other page keeps the normal opaque bar.
    // Driven from pageChanged (instant) so the bar crossfades as the page lands.
    if (m_unifiedBar) {
        connect(m_swiper, &SwipeNavigator::pageChanged, m_unifiedBar,
                [this](int idx) { m_unifiedBar->setCameraGlassMode(idx == kPageCamera); });
        // Initialise for the starting page.
        m_unifiedBar->setCameraGlassMode(m_swiper->currentIndex() == kPageCamera);
    }

    // V32.34+ DEBOUNCE LAYER SEPARATION. The tab-bar highlight above follows
    // pageChanged so it tracks the page INSTANTLY (responsiveness). The
    // EXPENSIVE / consequential work below — usage stats, AppContext page
    // propagation (which the NETWORK layer reacts to: peer priorities, retry
    // pressure), ambient-layer toggling, milestone celebrations, edge-hint
    // state — now runs on pageSettled instead, so a fast multi-page swipe fires
    // it ONCE for the page the user actually lands on rather than once per page
    // blown past. That removes the per-intermediate-page network churn and the
    // battery cost of redundant work. See SwipeNavigator's pageAnimating/
    // pageSettled split and snapclone-debounce.
    connect(m_swiper, &SwipeNavigator::pageSettled,
            this, [this](int page) {
        UxGuide::instance().recordUse(QLatin1String(Feature::kSwipe));
        static const char* pageFeatures[] = {
            Feature::kCamera, Feature::kInbox,
            Feature::kAddContact, Feature::kSettings };
        if (page >= 0 && page < 4)
            UxGuide::instance().recordUse(QLatin1String(pageFeatures[page]));

        // V32.21: tell AppContext which page is visible so network-layer
        // consumers can adjust priorities. E.g. when the user is parked on
        // the inbox we know they're waiting for messages and probably want
        // peers reachable; on settings we can take retry pressure off.
        if (m_controller && m_controller->appContext()) {
            static const char *pageNames[] = {
                "camera", "inbox", "friends", "wayfinder", "settings"
            };
            QString name = (page >= 0 && page < 5) ? pageNames[page] : QString();
            m_controller->appContext()->setCurrentPage(name);
        }

        // V32.32: pause AmbientBlobs animation on the camera page. The
        // viewfinder dominates the screen, blobs over it are visually
        // wasted, and stopping the 10fps repaint of a window-sized
        // translucent layer is a measurable CPU win during camera +
        // ambient-mode load. Resume on any other page.
        //
        // m_ambientBlobs is declared as QWidget* in the header (to keep
        // AmbientBlobs.hpp out of MainWindow.hpp's include graph), so
        // we cast here to access setActive(). The cast is safe because
        // the only assignment to m_ambientBlobs is `new AmbientBlobs(...)`
        // at construction time.
        if (auto *blobs = qobject_cast<AmbientBlobs *>(m_ambientBlobs)) {
            blobs->setActive(page != kPageCamera);
        }

        // Camera lifecycle is driven by CameraWidget::showEvent/hideEvent —
        // SwipeNavigator fires those when the visible page changes. We don't
        // double-call here because it would tear down + respawn the gst
        // pipeline on every swipe, which causes visible UI jitter.
        // (The previous explicit start/stopCamera calls are intentionally
        // removed — they conflicted with showEvent/hideEvent.)
        auto ms = UxGuide::instance().pendingMilestones();
        for (const auto &m : ms) showCelebration(m.key, m.title, m.body);
        UxGuide::instance().clearPendingMilestones();

        // Update edge peel hint
        if (m_edgeHint) {
            auto *eh = static_cast<PagePeelHint *>(m_edgeHint);
            eh->setPageState(page, 4);
            eh->dismissIfFamiliar();
            if (!eh->isVisible()) m_edgeHint = nullptr;
        }
    });

    // UxGuide signals → toasts
    connect(&UxGuide::instance(), &UxGuide::hintReady,
            this, [this](const QString &key, const QString &title,
                         const QString &body, QWidget *) {
        showHintToast(key, title, body);
    });
    // NOTE: milestoneAchieved is intentionally NOT wired to showCelebration here.
    // UxGuide also queues milestones into pendingMilestones() which the page-
    // change handler drains. Wiring both produces DOUBLE celebration toasts
    // (one immediate, one on next page change after the snap-send → inbox jump).
    // The page-change handler is the single source of truth; milestoneAchieved
    // is used only for logging.
    connect(&UxGuide::instance(), &UxGuide::milestoneAchieved,
            this, [](const QString &key, const QString &, const QString &) {
        qCDebug(lcUi) << "Milestone achieved:" << key << "(toast deferred to next page change)";
    });
    connect(&UxGuide::instance(), &UxGuide::chainStepHint,
            this, [this](const QString &, const QString &hint,
                         const QString &detail, int) {
        const QString msg = detail.isEmpty() ? hint : hint + QStringLiteral("\n") + detail;
        statusBar()->showMessage(msg, 5000);
    });

    // ── Controller signals → UI updates ───────────────────────────────────────
    if (m_controller) {
        // V32.20: enable conversation grouping in the inbox. setSelfId
        // gates the grouped UI on — without it, addSnap falls back to the
        // legacy flat-list view (one row per snap). Order matters: set
        // these BEFORE wiring snapReceived, so the very first inbound
        // snap renders as a grouped row.
        if (m_inboxPage) {
            m_inboxPage->setSelfId(Core::UserId{ m_controller->stableId() });
            m_inboxPage->setStorage(m_controller->storageManager());
            // Reload from storage so historical conversations (outbound-
            // only ones too) appear immediately without waiting for new
            // traffic. AppController separately calls addSnap() for each
            // inbound snap to repopulate m_snaps; that's idempotent with
            // our conversation aggregate.
            m_inboxPage->reloadConversationsFromStorage();

            // V32.34+ monthly export reminder. Show the "save your snaps"
            // card on the first launch of a new month — but only if there's
            // actually something worth exporting (≥ kExportReminderMinSnaps
            // recent media files). Below that we'd be nagging a user with an
            // ~empty folder. Auto-sync users never see it
            // (shouldShowThisMonth() returns false when a sync path is set).
            if (MonthlyExportReminder::shouldShowThisMonth()) {
                constexpr int kExportReminderMinSnaps = 3;
                const QStringList recent =
                    m_inboxPage->recentMediaPaths(kExportReminderMinSnaps + 8);
                if (recent.size() >= kExportReminderMinSnaps) {
                    m_inboxPage->showExportReminder(
                        recent.mid(0, 4));  // preview up to 4 thumbs
                    MonthlyExportReminder::markShownThisMonth();
                }
            }
            connect(m_inboxPage, &InboxWidget::exportReminderTriggered,
                    this, &MainWindow::runSnapExport);
            connect(m_inboxPage, &InboxWidget::exportReminderSnoozed,
                    this, []() { MonthlyExportReminder::snoozeForThisMonth(); });
        }

        // V32.24: push the contact list to friendsPage immediately at
        // startup. Previously the friends page sat empty for ~1 second
        // until peerConnectionStatus / peerStatUpdate signals started
        // firing per-peer as connections came up — the user perceives
        // that gap as "contacts are slow to load." But loadContacts is
        // a single indexed SELECT on `users` — fast. The slowness was
        // entirely the missing init call. Mirroring the inbox-init
        // pattern above is the simplest fix; one line of "abstraction"
        // we don't need to invent.
        if (m_friendsPage && m_controller->storageManager()) {
            const auto contacts = m_controller->storageManager()
                ->loadContacts(Core::UserId{ m_controller->stableId() });
            m_friendsPage->setContacts(contacts);
        }

        // V32.39+: group-voice transport. Outbound packets from the open
        // conversation's call go straight to the peer (networkManager() resolved
        // lazily at send time, so this is robust to startup ordering); inbound
        // voice packets are routed to the open conversation via the inbox.
        if (m_inboxPage) {
            connect(m_inboxPage, &InboxWidget::voicePayloadToSend, this,
                    [this](const Core::UserId &recipient, const QByteArray &payload) {
                // V32.47: tell the controller we're sending voice so the call
                // registers as active even when we're the only one talking.
                if (m_controller) m_controller->noteVoiceActivity();
                if (auto *net = m_controller ? m_controller->networkManager() : nullptr)
                    net->sendToPeer(recipient, payload);
            });
            connect(m_controller, &Core::AppController::voicePayloadReceived,
                    m_inboxPage, &InboxWidget::onVoicePayloadFromPeer);
        }

        // Inbound snap arrives → add to inbox, badge tab bar, record milestone
        connect(m_controller, &Core::AppController::snapReceived,
                this, [this](const Core::Snap &snap) {
            if (m_inboxPage) m_inboxPage->addSnap(snap);
            // Badge the Inbox tab — count unread snaps
            const int unread = m_inboxPage ? m_inboxPage->unreadCount() : 0;
            if (m_tabBar && unread > 0)
                m_tabBar->setBadge(kPageInbox, unread);
            // V32.34+: refresh the contacts cards' "last snap / N new" line
            // so the conversation summary on each card stays current.
            if (m_friendsPage)
                m_friendsPage->refreshConversationActivityAndRebuild();
            UxGuide::instance().recordUse(QLatin1String(Feature::kSnapReceived));
            // Brief shimmer on tab bar to draw attention
            if (m_tabBar) TouchFeedback::shimmer(m_tabBar);
        });

        // Track which peers are currently online (have an established
        // session). Used to decide whether a sent snap shows "Sent to X"
        // vs "Queued — will deliver on reconnect" in the status bar.
        connect(m_controller, &Core::AppController::peerConnected,
            this, [this](const Core::UserId &id) {
        m_onlinePeers.insert(id.value);
    });
    connect(m_controller, &Core::AppController::peerDisconnected,
            this, [this](const Core::UserId &id) {
        m_onlinePeers.remove(id.value);
    });

    connect(m_controller, &Core::AppController::snapDelivered,
                this, [this](const Core::SnapId &id) {
            if (m_inboxPage) m_inboxPage->markOutboundDelivered(id);
            // V32.34+: find which peer this snap was queued for, drop it,
            // and refresh that contact's queued pill. The map is the same
            // one populated at send time. Iterating is cheap (few peers,
            // few in-flight snaps); we stop at the first owner.
            for (auto it = m_outboundSnapsByPeer.begin();
                 it != m_outboundSnapsByPeer.end(); ++it) {
                if (it.value().remove(id.value)) {
                    if (m_friendsPage)
                        m_friendsPage->setOutboundQueued(
                            Core::UserId{ it.key() }, it.value().size());
                    if (it.value().isEmpty())
                        m_outboundSnapsByPeer.erase(it);
                    break;
                }
            }
            // V32.34+: a delivered outbound snap is the most recent activity
            // for that thread — refresh the contacts cards so the "last snap"
            // line reflects it.
            if (m_friendsPage)
                m_friendsPage->refreshConversationActivityAndRebuild();
        });

        // Peer connection status → propagate to ContactsPage so user sees state
        connect(m_controller, &Core::AppController::peerConnectionStatus,
                this, [this](const Core::UserId &peerId,
                             const QString &phase, const QString &detail) {
            if (m_friendsPage) m_friendsPage->setConnectionStatus(peerId, phase, detail);
        });

        // Per-peer stat updates ("Direct", "Relay", "STUN", etc.)
        connect(m_controller, &Core::AppController::peerStatUpdate,
                this, [this](const Core::UserId &peerId, const QString &stat) {
            if (m_friendsPage) m_friendsPage->setNetworkStat(peerId, stat);
        });

        // Network reachability — show/hide offline banner
        connect(m_controller, &Core::AppController::networkReachabilityChanged,
                this, [this](bool online) {
            qCInfo(lcUi) << "Network reachability:" << (online ? "online" : "offline");
            if (m_offlineBanner) {
                if (online) {
                    m_offlineBanner->hide();
                } else {
                    m_offlineBanner->show();
                    m_offlineBanner->raise();
                }
            }
            statusBar()->showMessage(
                online ? tr("Online") : tr("Offline — snaps will queue"),
                online ? 2000 : 5000);
        });

        // Friend shared their location — toast it. Full coordinates appear
        // when the user opens the snap viewer (existing functionality).
        connect(m_controller, &Core::AppController::peerLocationReceived,
                this, [this](const Core::UserId &peerId,
                             const Core::PeerLocation &/*loc*/) {
            QString name = peerId.value;
            if (m_controller->storageManager()) {
                if (auto c = m_controller->storageManager()->loadContact(peerId)) {
                    name = c->displayName.isEmpty() ? peerId.value : c->displayName;
                }
            }
            statusBar()->showMessage(
                tr("[loc] %1 shared their location").arg(name), 4000);
        });

        // Snap view expired (timed view window elapsed) — close any open
        // viewer for it (enforces ephemerality + removes the ~4s stuck-
        // dialog lag) and remove it from the outbound/inbox tracking.
        connect(m_controller, &Core::AppController::snapViewExpired,
                this, [this](const Core::SnapId &id) {
            if (m_inboxPage) {
                m_inboxPage->closeViewerForSnap(id);
                m_inboxPage->removeSnap(id);
            }
        });
    }

    // ── ContactsPage signals → controller actions ─────────────────────────────
    if (m_friendsPage && m_controller) {
        // User added a friend by IP/port (e.g. via QR code or manual entry)
        connect(m_friendsPage, &ContactsPage::addFriendRequested,
                this, [this](const QString &host, quint16 port) {
            // host can be either an "sc-XXXXXX" DHT ID or a URL/IP string
            if (host.startsWith(QStringLiteral("sc-"))) {
                m_controller->connectByDhtId(host);
            } else {
                Core::UserId friendId{host};
                Core::PeerAddress addr{host, port};
                m_controller->addFriend(friendId, addr);
            }
        });

        // User tapped a nearby peer in the beacon list — connect via DHT
        connect(m_friendsPage, &ContactsPage::nearbyPeerConnectRequested,
                this, [this](const QString &scId) {
            if (!scId.isEmpty())
                m_controller->connectByDhtId(scId);
        });

        // Contact deleted in editor — wipe from storage and refresh the UI list.
        connect(m_friendsPage, &ContactsPage::contactDeleted,
                this, [this](const Core::UserId &id) {
            if (m_controller->storageManager()) {
                const bool ok = m_controller->storageManager()->deleteContact(id);
                if (ok) {
                    statusBar()->showMessage(tr("Contact removed"), 2000);
                    if (m_friendsPage) {
                        // Refresh friends list to drop the deleted entry.
                        // The simplest reload: re-load contacts from storage.
                        const auto contacts = m_controller->storageManager()->loadContacts(
                            Core::UserId{m_controller->stableId()});
                        m_friendsPage->setContacts(contacts);
                    }
                } else {
                    statusBar()->showMessage(tr("Failed to remove contact"), 3000);
                }
            }
        });

        // Contact edited in editor — persist updates
        connect(m_friendsPage, &ContactsPage::contactEdited,
                this, [this](const Core::Contact &contact) {
            if (m_controller->storageManager())
                m_controller->storageManager()->saveContact(contact);
        });

        // Beacon toggle — start/stop pollinator advertising
        // Location share request — sends current GPS fix to recipient as a
        // Location snap. Gated on QSettings("privacy/shareLocation", false)
        // so the user must opt in via Settings before any transmission.
        connect(m_friendsPage, &ContactsPage::locationShareRequested,
                this, [this](const Core::UserId &recipient) {
            QSettings s;
            const bool optedIn = s.value(
                QStringLiteral("privacy/shareLocation"), false).toBool();
            if (!optedIn) {
                statusBar()->showMessage(
                    tr("Enable 'Share location' in Settings first"), 4000);
                return;
            }
            const bool ok = m_controller->sendLocationSnap(recipient, QString{});
            if (ok) {
                statusBar()->showMessage(
                    tr("Sent your location"), 2000);
            } else {
                // sendLocationSnap returns false when no GPS fix available
                statusBar()->showMessage(
                    tr("No location fix yet — try again in a moment"), 3000);
            }
        });

        // V32.40+: Call button on a contact card → open that conversation
        // already in-call. Switch to the Inbox first so the thread is in context
        // (the dialog is a sheet over it), then start the call.
        connect(m_friendsPage, &ContactsPage::callRequested,
                this, [this](const Core::UserId &id, const QString &displayName) {
            if (m_swiper) m_swiper->goToPage(kPageInbox, /*animate=*/false);
            if (m_inboxPage) m_inboxPage->startCallWith(id, displayName);
        });

        // V32.41+: "Add to call" breakout action → escalate the active call to a
        // group (or start one if none is active).
        connect(m_friendsPage, &ContactsPage::addToCallRequested,
                this, [this](const Core::UserId &id, const QString &displayName) {
            if (m_swiper) m_swiper->goToPage(kPageInbox, /*animate=*/false);
            if (m_inboxPage) m_inboxPage->addToActiveCall(id, displayName);
        });

        connect(m_friendsPage, &ContactsPage::beaconToggleRequested,
                this, [this](bool start) {
            if (auto *pol = m_controller->pollinatorDiscovery()) {
                if (start) pol->start();
                else       pol->stop();
                // Reflect new state back to ContactsPage so the button label
                // flips ("Start Beacon" ↔ "Stop Beacon") and the panel updates.
                // Without this, m_beaconActive in ContactsPage stays false and
                // every tap triggers a fresh start — user can never stop.
                if (m_friendsPage) m_friendsPage->setBeaconActive(start);
            }
        });
    }

    // ── InboxWidget signals → controller / UI actions ─────────────────────────
    if (m_inboxPage && m_controller) {
        // Snap opened by user — mark as viewed (triggers expiry timer).
        //
        // PREV BUG: the snapSaved connect was nested INSIDE the snapOpened
        // lambda body, so a fresh connection was made every time a snap was
        // opened. After N opens, snapSaved would fire N times, and the
        // statusBar()->showMessage call in the snapOpened lambda was actually
        // a connect-call returning a QMetaObject::Connection that was
        // discarded. Worse, the markSnapViewed → loadSnap → … chain inside
        // the inner lambda could trigger UI work mid-mark which crashed on
        // mobile when m_inboxPage's signal was emitted re-entrantly.
        connect(m_inboxPage, &InboxWidget::sendSnapRequested, this,
                [this](const Core::Snap &snap) {
            if (m_controller) m_controller->sendSnap(snap);   // V32.45+: ephemeral text
        });
        connect(m_inboxPage, &InboxWidget::snapOpened,
                this, [this](const Core::SnapId &id) {
            // V32.21: opening a snap is a strong user-action signal too.
            if (m_controller && m_controller->appContext()) {
                m_controller->appContext()->noteUserAction();
            }
            m_controller->markSnapViewed(id);
        });

        // Snap saved to disk → confirmation in status bar
        connect(m_inboxPage, &InboxWidget::snapSaved,
                this, [this](const QString &path) {
            statusBar()->showMessage(
                tr("Saved to %1").arg(path), 4000);
        });

        // Unread count changed — update tab bar badge
        connect(m_inboxPage, &InboxWidget::unreadCountChanged,
                this, [this](int count) {
            if (m_tabBar) m_tabBar->setBadge(kPageInbox, count);
        });
    }

    // QR scan
    connect(m_friendsPage, &ContactsPage::qrScanRequested,
            this, [this]() {
        auto *dlg = new QrScanDialog(m_cameraPage, this);
        dlg->setAttribute(Qt::WA_DeleteOnClose);
        connect(dlg, &QrScanDialog::scanStartRequested,
                m_cameraPage, [this]() {
            // Ensure camera is actually running before scan mode begins.
            // setScanMode just enables frame emission — it doesn't start capture.
            m_cameraPage->startCamera();
            m_cameraPage->setScanMode(true);
        });
        connect(dlg, &QrScanDialog::scanStopRequested,
                m_cameraPage, [this]() { m_cameraPage->setScanMode(false); });
        connect(dlg, &QrScanDialog::scannedContact,
                this, [this](const QString &scId, quint16 port) {
            // V32.32: feedback on QR scan result. Previously the scan
            // succeeded silently — user had no idea whether it worked
            // or what happened. Now: status bar message identifies
            // either "Added X" (new contact) or "Already in contacts: X"
            // (duplicate scan). Either way the user knows the QR was
            // successfully read and what the outcome was.
            const bool alreadyKnown = m_controller
                && m_controller->storageManager()
                && m_controller->storageManager()->loadContact(
                       Core::UserId{scId}).has_value();
            if (alreadyKnown) {
                statusBar()->showMessage(
                    tr("Already in contacts: %1").arg(scId.left(12) +
                        (scId.size() > 12 ? QStringLiteral("…") : QString())),
                    4000);
            } else {
                statusBar()->showMessage(
                    tr("Adding contact %1…").arg(scId.left(12) +
                        (scId.size() > 12 ? QStringLiteral("…") : QString())),
                    4000);
            }
            emit m_friendsPage->addFriendRequested(scId, port);
        });
        dlg->open();
    });

    // Import vCard
    connect(m_friendsPage, &ContactsPage::importVCardRequested,
            this, [this]() {
        const QString path = QFileDialog::getOpenFileName(
            this, tr("Import Contacts"), {},
            tr("vCard (*.vcf);;All files (*)"));
        if (path.isEmpty()) return;
        QFile f(path);
        if (!f.open(QIODevice::ReadOnly | QIODevice::Text)) return;
        // VCardIO::importContacts handled by AppController
        qCInfo(lcUi) << "Import vCard:" << path;
    });

    // Settings
    connect(m_settingsPage, &SettingsPage::restartRequired,
            this, [this](const QString &reason) {
        statusBar()->showMessage(reason, 8000);
    });

    // Open ComponentTestWindow when user taps "UI Component Tests" in diagnostics
    connect(m_settingsPage, &SettingsPage::componentTestRequested,
            this, [this]() {
        auto *test = new ComponentTestWindow(this);
        test->setAttribute(Qt::WA_DeleteOnClose);
        test->show();
    });

    // Clear address cache for stale STUN/peer entries.
    // Clear cached peer addresses — forces re-resolution via DHT on next connect.
    connect(m_settingsPage, &SettingsPage::clearAddressCacheRequested,
            this, [this]() {
        qCInfo(lcUi) << "Clear address cache requested but no API yet";
        statusBar()->showMessage(tr("Address cache clear not implemented"), 3000);
    });

    // Factory reset — wipe identity + DB + settings
    connect(m_settingsPage, &SettingsPage::factoryResetRequested,
            this, [this]() {
        if (m_controller) m_controller->factoryReset();
        statusBar()->showMessage(tr("Factory reset complete — restart required"), 8000);
    });

    // V32.30: Ambient: Camera mode. Lazy-create the driver on first
    // activation; subsequent toggles just start/stop the existing one.
    // The driver subscribes to CameraWidget's previewFrameAvailable
    // signal and re-derives the synthetic palette at 1Hz.
    connect(m_settingsPage, &SettingsPage::ambientCameraRequested,
            this, [this](bool start) {
        if (start) {
            // V32.33: respect master enable. If user has disabled ambient
            // globally, palette-carousel taps still update the saved palette
            // but don't actually launch drivers. They'll launch when the
            // user re-enables ambient via the master switch.
            const bool ambientEnabled = QSettings(
                QStringLiteral("SnapCloneProject"),
                QStringLiteral("SnapClone"))
                .value(QStringLiteral("ui/ambient_enabled"), true)
                .toBool();
            if (!ambientEnabled) {
                qCInfo(lcUi) << "ambientCameraRequested(true) ignored — "
                                "master switch is OFF";
                return;
            }
            if (!m_ambientCameraDriver) {
                m_ambientCameraDriver = new AmbientCameraDriver(this);
            }
            auto *drv = qobject_cast<AmbientCameraDriver *>(m_ambientCameraDriver);
            if (drv && m_cameraPage) {
                drv->start(m_cameraPage);
                statusBar()->showMessage(
                    tr("Ambient: Camera — chrome will follow the viewfinder"),
                    4000);
            }
        } else {
            auto *drv = qobject_cast<AmbientCameraDriver *>(m_ambientCameraDriver);
            if (drv) drv->stop();
        }
    });

    // V32.30 (Wave 2): Ambient: Inbox auto-refresh. Same pattern —
    // lazy-create the driver, start/stop on signal. The driver does
    // the initial derivation on start (uses current inbox as seed)
    // and re-derives on every snapReceived (30s throttled).
    connect(m_settingsPage, &SettingsPage::ambientInboxRequested,
            this, [this](bool start) {
        if (start) {
            // V32.33: respect master enable (same rationale as camera path).
            const bool ambientEnabled = QSettings(
                QStringLiteral("SnapCloneProject"),
                QStringLiteral("SnapClone"))
                .value(QStringLiteral("ui/ambient_enabled"), true)
                .toBool();
            if (!ambientEnabled) {
                qCInfo(lcUi) << "ambientInboxRequested(true) ignored — "
                                "master switch is OFF";
                return;
            }
            if (!m_ambientInboxDriver) {
                m_ambientInboxDriver = new AmbientInboxDriver(this);
            }
            auto *drv = qobject_cast<AmbientInboxDriver *>(m_ambientInboxDriver);
            if (drv && m_controller) {
                drv->start(m_controller);
                statusBar()->showMessage(
                    tr("Ambient: Inbox — chrome will follow new snaps"),
                    4000);
            }
        } else {
            auto *drv = qobject_cast<AmbientInboxDriver *>(m_ambientInboxDriver);
            if (drv) drv->stop();
        }
    });

    // V32.33: master kill-switch for all ambient drivers. When the user
    // toggles "Enable ambient drivers" OFF in Settings, stop both drivers
    // regardless of the current palette. When toggled back ON, re-trigger
    // whichever ambient mode the current palette implies (so the user's
    // palette choice is preserved across the off/on toggle).
    //
    // Lives here in MainWindow rather than in SettingsPage because the
    // driver instances are owned by MainWindow and the carousel widget
    // doesn't have direct access to them.
    connect(m_settingsPage, &SettingsPage::ambientMasterEnabledChanged,
            this, [this](bool enabled) {
        if (!enabled) {
            // Stop both drivers immediately. Don't touch the saved palette
            // — when the user re-enables, we'll restore from it.
            if (auto *drv = qobject_cast<AmbientInboxDriver *>(m_ambientInboxDriver)) {
                drv->stop();
            }
            if (auto *drv = qobject_cast<AmbientCameraDriver *>(m_ambientCameraDriver)) {
                drv->stop();
            }
            qCInfo(lcUi) << "Ambient master switch OFF — both drivers stopped.";
            return;
        }
        // Re-enabled: check the saved palette and re-trigger the matching
        // ambient driver. Same logic as startup-restore path below.
        const QString savedPalette = QSettings(
            QStringLiteral("SnapCloneProject"),
            QStringLiteral("SnapClone"))
            .value(QStringLiteral("ui/palette"), QStringLiteral("dark"))
            .toString();
        if (savedPalette == QLatin1String("ambient_inbox")) {
            emit m_settingsPage->ambientInboxRequested(true);
            qCInfo(lcUi) << "Ambient master switch ON — restoring ambient_inbox.";
        } else if (savedPalette == QLatin1String("ambient_camera")) {
            emit m_settingsPage->ambientCameraRequested(true);
            qCInfo(lcUi) << "Ambient master switch ON — restoring ambient_camera.";
        } else {
            qCInfo(lcUi) << "Ambient master switch ON — current palette is"
                         << savedPalette << "(not ambient), nothing to restart.";
        }
    });

    // V32.32: open ModalLayerTestPanel when user taps the test button
    // in Settings → Diagnostics. The panel is presented INTO the
    // ModalSurface's Dialog layer (it's a test of the system itself).
    connect(m_settingsPage, &SettingsPage::modalSurfaceTestRequested,
            this, [this]() {
        if (!m_modalSurface) return;
        auto *panel = new ModalLayerTestPanel(m_modalSurface);
        // Center on the modal surface.
        const QSize hostSize = m_modalSurface->size();
        const QSize panelSize = panel->size();
        panel->move((hostSize.width()  - panelSize.width())  / 2,
                    (hostSize.height() - panelSize.height()) / 2);
        m_modalSurface->present(panel, ModalSurface::Layer::Dialog,
                                /*dismissOnTap=*/true,
                                /*blocksTouches=*/true);
    });

    // Populate the Node ID display in Settings (was showing "sc-..." placeholder)
    if (m_settingsPage && m_controller) {
        const QString id = m_controller->stableId();
        if (!id.isEmpty())
            m_settingsPage->setNodeId(id);
    }

    // V32.32: restore ambient mode on startup. SettingsPage saves the
    // user's palette choice (including ambient_inbox / ambient_camera)
    // to QSettings; previously only the static palettes' selections
    // survived a restart. Now: if the saved id is an ambient mode,
    // re-trigger the appropriate driver. This must happen AFTER the
    // ambient*Requested connects above so the signal actually wires
    // through to the driver, and AFTER m_controller is set so the
    // inbox driver has storage to read from.
    {
        const QString savedPalette = QSettings(
            QStringLiteral("SnapCloneProject"),
            QStringLiteral("SnapClone"))
            .value(QStringLiteral("ui/palette"), QStringLiteral("dark"))
            .toString();
        // V32.33: check the master ambient enable flag first. If the user
        // previously disabled ambient drivers, don't restore them on
        // startup even if the saved palette is an ambient one. The
        // palette choice is preserved separately and the drivers can be
        // resumed later via the Settings toggle.
        const bool ambientEnabled = QSettings(
            QStringLiteral("SnapCloneProject"),
            QStringLiteral("SnapClone"))
            .value(QStringLiteral("ui/ambient_enabled"), true)
            .toBool();
        if (!ambientEnabled) {
            qCInfo(lcUi) << "Ambient master switch is OFF — not restoring"
                         << "ambient drivers despite palette" << savedPalette;
        } else if (savedPalette == QLatin1String("ambient_inbox")) {
            qCInfo(lcUi) << "Restoring ambient_inbox from QSettings";
            // Defer one event-loop turn so the SettingsPage UI has
            // finished painting its initial state — the carousel chip's
            // selected-state styling looks for the ambient-mode-active
            // flag which is set inside the handler.
            QTimer::singleShot(0, this, [this]() {
                emit m_settingsPage->ambientInboxRequested(true);
            });
        } else if (savedPalette == QLatin1String("ambient_camera")) {
            qCInfo(lcUi) << "Restoring ambient_camera from QSettings";
            QTimer::singleShot(0, this, [this]() {
                emit m_settingsPage->ambientCameraRequested(true);
            });
        }
    }

    // Snap captured → contact picker → send.
    //
    // V32.15: removed the "sticky recipient" path. Previously the user could
    // tap a contact card on the Friends page to set a default recipient,
    // and the handler would skip the picker if one was set. That UX broke
    // down with multiple ongoing conversations: there was no visual
    // selection indicator on the card, so users would tap, scroll away,
    // come back, and have no idea who was "active" — and snaps shipped
    // to the wrong person. The fix: ALWAYS show the picker after capture,
    // so the user re-confirms the recipient every time.
    //
    // We accept the small extra friction (one extra tap per send) in
    // exchange for never sending to the wrong contact silently. The
    // picker pre-selects the last-used contact for that hand (if any),
    // so the common case is still single-tap send.
    // Swipe-down on the shutter, or the bottom-right mode button → start a voice
    // chat: open the people picker in multi-select, then hand the chosen ids to
    // the inbox, which opens the call and escalates to everyone picked.
    connect(m_cameraPage, &CameraWidget::voiceChatRequested, this, [this]() {
        if (!m_controller) return;
        const auto contacts = m_controller->storageManager()
            ? m_controller->storageManager()->loadContacts(
                  Core::UserId{ m_controller->stableId() })
            : QList<Core::Contact>{};
        if (contacts.isEmpty()) {
            statusBar()->showMessage(
                tr("Add a friend first — swipe right to Friends"), 4000);
            m_swiper->goToPage(kPageFriends);
            return;
        }
        auto *picker = new ContactPickerDialog(contacts, this);
        picker->setAttribute(Qt::WA_DeleteOnClose);
        picker->setMultiSelect(true, tr("Next"));
        picker->setHeaderTitle(tr("New voice chat"));
        connect(picker, &ContactPickerDialog::contactsConfirmed, this,
                [this](const QList<Core::UserId> &ids) {
            if (m_inboxPage) m_inboxPage->startGroupVoiceCall(ids);
        });
        picker->open();
    });
    connect(m_cameraPage, &CameraWidget::snapCaptured,
            this, [this](const QImage &image, int duration) {
        // V32.21: shutter tap is a strong "user wants snappy delivery"
        // signal — feed into AppContext so the network layer can push
        // past its normal exponential backoff for the next few seconds.
        if (m_controller && m_controller->appContext()) {
            m_controller->appContext()->noteUserAction();
        }
        if (!m_controller->sessionManager()) {
            statusBar()->showMessage(tr("Not logged in"), 3000); return;
        }
        const auto selfId = m_controller->sessionManager()->currentUserId();
        if (!selfId) { statusBar()->showMessage(tr("Not logged in"), 3000); return; }

        // Direct send path — entered ONLY via the picker's queued re-emit
        // below. The picker stashes the chosen recipient on m_pendingSnapRecipient
        // before re-emitting; if it's set, we use it and clear it. If it's
        // not set, we're on a fresh capture and we open the picker first.
        if (!m_pendingSnapRecipient.has_value()) {
            // Fresh capture — open picker.
            const auto contacts = m_controller->storageManager()
                ? m_controller->storageManager()->loadContacts(
                      Core::UserId{m_controller->stableId()})
                : QList<Core::Contact>{};

            if (contacts.isEmpty()) {
                statusBar()->showMessage(
                    tr("Add a friend first — swipe right to Friends"), 4000);
                UxGuide::instance().startChain(QStringLiteral("add_contact"));
                m_swiper->goToPage(kPageFriends);
                return;
            }

            auto *picker = new ContactPickerDialog(contacts, this);
            picker->setAttribute(Qt::WA_DeleteOnClose);
            connect(picker, &ContactPickerDialog::contactSelected,
                    this, [this, image, duration](const Core::UserId &uid) {
                // Stash the chosen recipient and queue a re-emit. The
                // queued path is critical — see V32.4 comment below for
                // why a synchronous re-emit crashes during the picker's
                // accept-animation teardown on mobile.
                m_pendingSnapRecipient = uid;
                QPointer<MainWindow> self(this);
                const QImage imgCopy = image;
                const int    durCopy = duration;
                QMetaObject::invokeMethod(this, [self, imgCopy, durCopy]() {
                    if (!self || !self->m_cameraPage) return;
                    emit self->m_cameraPage->snapCaptured(imgCopy, durCopy);
                }, Qt::QueuedConnection);
            });
            // If user dismisses without selecting, clear any leftover
            // pending state and don't send.
            connect(picker, &QDialog::rejected, this, [this]() {
                m_pendingSnapRecipient.reset();
            });
            // CRITICAL: defer the re-emit. The contactSelected signal fires
            // inside ContactPickerDialog::m_sendBtn::clicked, which then
            // calls accept() which kicks off the slide-out animation. If we
            // re-emit snapCaptured synchronously, the outer handler
            // re-enters with the picker still visible AND mid-animation;
            // when it then calls m_swiper->goToPage(kPageInbox) the
            // CameraWidget hides → backend->stop() → QProcess::waitForFinished
            // pumps the event loop while the picker dialog is in a
            // half-destroyed state. On mobile this manifests as a crash
            // shortly after reaching the inbox page. (See V32.4 work log.)
            picker->open();
            return;
        }

        // Picker selected — pop the recipient and send below.
        const Core::UserId friendId = *m_pendingSnapRecipient;
        m_pendingSnapRecipient.reset();

        // The image arriving here has ALREADY been through the composer
        // (CameraWidget::onFrameCaptured opened SnapComposerDialog #1, the
        // user added text and tapped Send, which emits snapCaptured). We do
        // NOT open a second composer — that was the bug producing two text-
        // editing windows and leaving a modal scrim if the user dismissed
        // the second one. Just encode and send.

        QByteArray imageBytes;
        QBuffer buf(&imageBytes);
        buf.open(QIODevice::WriteOnly);

        static constexpr int kMaxDimension = 1280;
        const QImage toSend = (image.width() > kMaxDimension || image.height() > kMaxDimension)
            ? image.scaled(kMaxDimension, kMaxDimension,
                           Qt::KeepAspectRatio, Qt::SmoothTransformation)
            : image;

        if (!toSend.save(&buf, "JPEG", 72)) {
            qCWarning(lcUi) << "Failed to encode image as JPEG";
            statusBar()->showMessage(tr("Failed to encode image"), 3000);
            return;
        }

        // V32.39+: strip EXIF/GPS/metadata from the outgoing image before it's
        // encrypted + sent (privacy gate). For a Qt-rendered snap this is usually
        // a no-op — a fresh QImage carries no camera metadata — but it's the
        // correct gate for any original-image media and catches anything the
        // encoder embeds. Output is never larger than the input.
        const int originalImageBytes = imageBytes.size();
        const Core::StripOutcome strip = Core::MediaStrip::stripImage(imageBytes);

        Core::Snap snap;
        snap.id            = Core::SnapId::generate();
        snap.senderId      = *selfId;
        snap.recipientId   = friendId;
        snap.mediaType     = Core::MediaType::Image;
        snap.sentAt        = QDateTime::currentDateTimeUtc();
        snap.expiresAt     = snap.sentAt.addSecs(24 * 3600);
        snap.encryptedBlob = strip.data;
        snap.durationSecs  = duration > 0 ? duration : 5;

        // Privacy gate: only attach location to photo snaps if user opted in.
        // Default OFF — user must enable in Settings → Privacy.
        // Separate from privacy/shareLocation which gates the explicit
        // share-pin button per-contact.
        QSettings privSettings;
        if (privSettings.value(QStringLiteral("privacy/sharePhotoLocation"),
                               false).toBool()) {
            if (const auto loc = m_controller->lastKnownLocation(
                    Core::UserId{ m_controller->stableId() })) {
                snap.locLat  = loc->lat;
                snap.locLon  = loc->lon;
                snap.locAccM = loc->accuracyM;
            }
        }

        m_controller->sendSnap(snap);
        {
            // Resolve recipient display name from the chosen UserId (no more
            // m_friendsPage->selectedContactName() — that depended on the
            // sticky-recipient state we just removed).
            QString recipName;
            if (m_controller->storageManager()) {
                if (const auto c = m_controller->storageManager()->loadContact(friendId))
                    recipName = c->nickname.isEmpty() ? c->displayName : c->nickname;
            }
            if (recipName.isEmpty()) recipName = friendId.value;
            m_inboxPage->addOutboundSnap(snap.id, snap.recipientId,
                                      recipName, QDateTime::currentDateTimeUtc());
            m_outboundSnapsByPeer[snap.recipientId.value].insert(snap.id.value);
            // V32.34+: surface the queued count on the contact card.
            if (m_friendsPage)
                m_friendsPage->setOutboundQueued(
                    snap.recipientId,
                    m_outboundSnapsByPeer[snap.recipientId.value].size());
        }
        m_swiper->goToPage(kPageInbox);
        statusBar()->showMessage(tr("Snap sent!"), 2000);

        // V32.39+: brief post-send receipt — strip result (privacy), media size
        // before/after strip, and recipient/encryption. Non-modal, auto-dismisses
        // after a few seconds. Gated by a setting (default on) so it's visible for
        // testing; set ui/showSendReceipt=false to silence it. Recipients = 1 for
        // a 1:1 snap (per-recipient dedup is a media-share/group feature).
        if (QSettings().value(QStringLiteral("ui/showSendReceipt"), true).toBool()) {
            auto *receipt = new QDialog(this);
            receipt->setAttribute(Qt::WA_DeleteOnClose);
            receipt->setModal(false);
            receipt->setWindowFlag(Qt::FramelessWindowHint);
            auto *lay = new QVBoxLayout(receipt);
            lay->setContentsMargins(0, 0, 0, 0);
            auto *summary = new MediaSendSummary(receipt);
            summary->setStripResult(strip.format, strip.removed);
            summary->setDedup(strip.data.size(), originalImageBytes);
            summary->setRecipients(1);
            lay->addWidget(summary);
            receipt->resize(summary->sizeHint());
            QTimer::singleShot(3000, receipt, &QDialog::accept);
            receipt->show();
            receipt->raise();
        }
        if (m_tabBar) TouchFeedback::shimmer(m_tabBar);
        UxGuide::instance().recordUse(QLatin1String(Feature::kSendSnap));
        // Drain any milestones that recordUse just queued
        auto pendingMs = UxGuide::instance().pendingMilestones();
        for (const auto &m : pendingMs) showCelebration(m.key, m.title, m.body);
        UxGuide::instance().clearPendingMilestones();
    });

    // Video snap captured — read file, route through sendSnap as MediaType::Video.
    //
    // V32.15: same always-picker treatment as the photo path above.
    // m_pendingSnapRecipient is reused — photo and video captures don't
    // fire concurrently, so one transient slot is enough.
    connect(m_cameraPage, &CameraWidget::videoSnapCaptured,
            this, [this](const QString &webmPath, int durationMs) {
        // Ensure the recording file is removed no matter which path we exit
        // through. Without this, every "no recipient selected" tap-and-hold
        // leaves a 2-5 MB .webm in ~/.local/share/SnapCloneProject/SnapClone/
        // recordings/ that nothing ever cleans up — disk fills over time.
        //
        // We can't use QScopeGuard (Qt 6.5+ but signature is fiddly), so
        // construct a tiny RAII shim inline. The lambda capture-by-value
        // ensures the path string survives until destructor runs.
        //
        // The cleaner is DISARMED before opening the picker — we hand the
        // file path to the picker's callback, which queues a re-emit; the
        // re-emit constructs a NEW cleaner that will eventually delete the
        // file once the send path completes (or fails) on that second turn.
        struct FileCleaner {
            QString path;
            bool armed = true;
            ~FileCleaner() { if (armed) QFile::remove(path); }
        };
        FileCleaner cleaner{ webmPath };

        if (!m_controller->sessionManager()) {
            statusBar()->showMessage(tr("Not logged in"), 3000);
            return;  // cleaner removes file
        }
        const auto selfId = m_controller->sessionManager()->currentUserId();
        if (!selfId) {
            statusBar()->showMessage(tr("Not logged in"), 3000);
            return;  // cleaner removes file
        }

        // Fresh capture (no pending recipient from a prior picker)?
        // Open the picker and queue a re-emit on selection.
        if (!m_pendingSnapRecipient.has_value()) {
            const auto contacts = m_controller->storageManager()
                ? m_controller->storageManager()->loadContacts(
                      Core::UserId{m_controller->stableId()})
                : QList<Core::Contact>{};

            if (contacts.isEmpty()) {
                statusBar()->showMessage(
                    tr("Add a friend first — swipe right to Friends"), 4000);
                UxGuide::instance().startChain(QStringLiteral("add_contact"));
                m_swiper->goToPage(kPageFriends);
                return;  // cleaner removes file
            }

            auto *picker = new ContactPickerDialog(contacts, this);
            picker->setAttribute(Qt::WA_DeleteOnClose);
            connect(picker, &ContactPickerDialog::contactSelected,
                    this, [this, webmPath, durationMs](const Core::UserId &uid) {
                m_pendingSnapRecipient = uid;
                QPointer<MainWindow> self(this);
                const QString pathCopy = webmPath;
                const int     durCopy  = durationMs;
                QMetaObject::invokeMethod(this, [self, pathCopy, durCopy]() {
                    if (!self || !self->m_cameraPage) return;
                    emit self->m_cameraPage->videoSnapCaptured(pathCopy, durCopy);
                }, Qt::QueuedConnection);
            });
            connect(picker, &QDialog::rejected, this, [this]() {
                m_pendingSnapRecipient.reset();
            });
            // CRITICAL: disarm the cleaner so the .webm survives until the
            // queued re-emit handles it. The new handler-invocation will
            // construct its own cleaner.
            cleaner.armed = false;
            picker->open();
            return;
        }

        // Picker selected — pop the recipient and proceed with send below.
        const Core::UserId friendId = *m_pendingSnapRecipient;
        m_pendingSnapRecipient.reset();

        QFile f(webmPath);
        if (!f.open(QIODevice::ReadOnly)) {
            qCWarning(lcUi) << "Cannot read recorded video:" << webmPath;
            statusBar()->showMessage(tr("Recording file unreadable"), 3000);
            return;  // cleaner removes file
        }
        const QByteArray bytes = f.readAll();
        f.close();
        // Cleaner will delete the file when scope exits — same behavior as the
        // explicit QFile::remove(webmPath) we used to have here.

        statusBar()->showMessage(tr("Sending video…"), 5000);

        Core::Snap snap;
        snap.id              = Core::SnapId::generate();
        snap.senderId        = *selfId;
        snap.recipientId     = friendId;
        snap.mediaType       = Core::MediaType::Video;
        snap.sentAt          = QDateTime::currentDateTimeUtc();
        snap.expiresAt       = snap.sentAt.addSecs(24 * 3600);
        snap.encryptedBlob   = bytes;
        snap.durationSecs    = qMax(1, durationMs / 1000);
        snap.videoDurationMs = durationMs;
        // Privacy gate: only attach location to photo snaps if user opted in.
        // Default OFF — user must enable in Settings → Privacy.
        // Separate from privacy/shareLocation which gates the explicit
        // share-pin button per-contact.
        QSettings privSettings;
        if (privSettings.value(QStringLiteral("privacy/sharePhotoLocation"),
                               false).toBool()) {
            if (const auto loc = m_controller->lastKnownLocation(
                    Core::UserId{ m_controller->stableId() })) {
                snap.locLat  = loc->lat;
                snap.locLon  = loc->lon;
                snap.locAccM = loc->accuracyM;
            }
        }

        m_controller->sendSnap(snap);
        // Resolve recipient display name from the chosen UserId.
        QString recipName;
        if (m_controller->storageManager()) {
            if (const auto c = m_controller->storageManager()->loadContact(friendId))
                recipName = c->nickname.isEmpty() ? c->displayName : c->nickname;
        }
        if (recipName.isEmpty()) recipName = friendId.value;

        m_inboxPage->addOutboundSnap(snap.id, snap.recipientId, recipName,
                                     QDateTime::currentDateTimeUtc());
        m_outboundSnapsByPeer[snap.recipientId.value].insert(snap.id.value);
        // V32.34+: surface the queued count on the contact card.
        if (m_friendsPage)
            m_friendsPage->setOutboundQueued(
                snap.recipientId,
                m_outboundSnapsByPeer[snap.recipientId.value].size());
        m_swiper->goToPage(kPageInbox);

        // Honest messaging — "Sent" implies delivery, but the snap may have
        // been queued for offline delivery. Use the network manager's online
        // state to differentiate. The actual delivery confirmation comes
        // later via snapDelivered (which updates the outbox card).
        const QString sizeKb = QString::number(bytes.size() / 1024);
        const bool online = m_onlinePeers.contains(friendId.value);
        if (online) {
            statusBar()->showMessage(tr("Video sent to %1 (%2 KB)")
                .arg(recipName.isEmpty() ? friendId.value : recipName, sizeKb), 3000);
        } else {
            statusBar()->showMessage(tr("Video queued — %1 is offline, will deliver on reconnect (%2 KB)")
                .arg(recipName.isEmpty() ? friendId.value : recipName, sizeKb), 5000);
        }
    });

    // Contact tapped on Wayfinder → small action sheet
    // ("View profile" / "Send a snap") rather than committing the user
    // to one default. They might have spotted a friend to message OR
    // to look up details for. Asking once is faster than the wrong
    // default forcing them to navigate back.
    //
    // Implementation: a small MobileDialog that closes itself on
    // either button. We construct it on the heap with WA_DeleteOnClose
    // so it cleans up after dismissal — no need to track the pointer.
    connect(m_wayfinderPage, &WayfinderPage::contactTapped,
                this, [this](const Core::UserId &id) {
            const QString displayName = m_controller->storageManager()
                ? [&]() {
                    const auto c = m_controller->storageManager()->loadContact(id);
                    return (c && !c->displayName.isEmpty()) ? c->displayName : id.value;
                }()
                : id.value;

            auto *dlg = new MobileDialog(this);
            dlg->setAttribute(Qt::WA_DeleteOnClose);
            dlg->setWindowTitle(displayName);
            // Compact: 320×220-ish. Two buttons stacked vertically with
            // a small header label naming the contact. On small phones
            // this still leaves a scrim around it indicating "tap
            // outside to dismiss".
            if (auto *scr = QGuiApplication::primaryScreen()) {
                const QSize avail = scr->availableSize();
                const int w = qMin(360, static_cast<int>(avail.width()  * 0.85));
                const int h = qMin(240, static_cast<int>(avail.height() * 0.40));
                dlg->resize(w, h);
            } else {
                dlg->resize(320, 220);
            }
            auto *root = new QVBoxLayout(dlg);
            root->setSpacing(Theme::SPACE_MD);
            root->setContentsMargins(Theme::SPACE_LG, Theme::SPACE_LG,
                                     Theme::SPACE_LG, Theme::SPACE_LG);
            auto *title = new QLabel(tr("What would you like to do?"), dlg);
            title->setStyleSheet(QStringLiteral("font-size: %1px; font-weight: 600;")
                                    .arg(Theme::FONT_LG));
            title->setAlignment(Qt::AlignCenter);
            title->setWordWrap(true);
            root->addWidget(title);

            auto *btnProfile = new QPushButton(tr("View profile"), dlg);
            auto *btnSnap    = new QPushButton(tr("Send a snap"), dlg);
            btnProfile->setMinimumHeight(Theme::TOUCH_LARGE);
            btnSnap   ->setMinimumHeight(Theme::TOUCH_LARGE);
            root->addWidget(btnProfile);
            root->addWidget(btnSnap);
            root->addStretch(1);

            // "View profile" — same as before: navigate to Friends with
            // this contact selected. ContactsPage::setSelectedContact
            // emits contactSelected which we already handle below.
            const Core::UserId capturedId = id;
            connect(btnProfile, &QPushButton::clicked, dlg, [this, capturedId, dlg]() {
                if (m_friendsPage) m_friendsPage->setSelectedContact(capturedId);
                m_swiper->goToPage(kPageFriends);
                dlg->close();
            });
            // "Send a snap" — set the camera page's recipient AND
            // navigate to it directly. This is the value-add that
            // wasn't available before: spot a friend on the map →
            // shoot a snap to them in one continuous flow.
            connect(btnSnap, &QPushButton::clicked, dlg, [this, capturedId, displayName, dlg]() {
                if (m_cameraPage) m_cameraPage->setRecipient(displayName);
                m_swiper->goToPage(kPageCamera);
                dlg->close();
            });

            dlg->show();
            dlg->raise();
        });

    // Contact selected → update camera banner.
    //
    // V32.15: The Wayfinder map-tap path still calls
    // m_friendsPage->setSelectedContact() (above) to navigate to the
    // friends page with the contact highlighted, which emits
    // contactSelected. We still respond by updating the camera label
    // hint — even though there's no longer a sticky recipient, the
    // label briefly shows the contact name as confirmation the user's
    // intent to message them was registered. The label reverts to
    // "Shoot then choose recipient" on next camera show.
    connect(m_friendsPage, &ContactsPage::contactSelected,
            this, [this](const Core::UserId &id) {
        if (m_cameraPage && m_controller->storageManager()) {
            const auto c = m_controller->storageManager()->loadContact(id);
            m_cameraPage->setRecipient(c ? c->displayName : id.value);
        }
    });

    // V32.34+: virtual-keyboard handling. On mobile (Phosh / postmarketOS
    // squeekboard, etc), the on-screen keyboard pops up over the bottom
    // half of the screen when a text input receives focus — often
    // covering the very input the user just tapped.
    //
    // Strategy: when the keyboard appears, find the currently focused
    // widget, walk up to its nearest QAbstractScrollArea ancestor, and
    // call ensureWidgetVisible() with a margin so the input sits well
    // above the keyboard edge. For widgets NOT inside a scroll area,
    // we'd need a different strategy (e.g. shift a dialog upward); v1
    // covers the dominant case (Settings page is the primary text-input
    // location and is inside a QScrollArea).
    //
    // We don't aggressively reverse on hide — the user has either
    // committed (scroll position is fine where it is) or dismissed
    // (also fine). Leaving the scroll position wherever it landed is
    // less jarring than animating it back.
    connect(&VirtualKeyboardWatcher::instance(),
            &VirtualKeyboardWatcher::visibilityChanged,
            this, [this](bool visible, const QRect &rect) {
        if (!visible) return;
        QWidget *focused = QApplication::focusWidget();
        if (!focused) return;

        // Walk up the parent chain looking for a scrollable ancestor.
        // We track BOTH a QScrollArea (which has the high-level
        // ensureWidgetVisible convenience) and the more general
        // QAbstractScrollArea (covers QListView, QTreeView etc that
        // don't inherit QScrollArea). Whichever we find first wins.
        QScrollArea         *qsa = nullptr;
        QAbstractScrollArea *qasa = nullptr;
        for (QWidget *w = focused->parentWidget(); w; w = w->parentWidget()) {
            if (!qsa)  qsa  = qobject_cast<QScrollArea *>(w);
            if (!qasa) qasa = qobject_cast<QAbstractScrollArea *>(w);
            if (qsa || qasa) break;  // first hit wins
        }

        const int margin = 12;
        if (qsa) {
            // QScrollArea has the high-level convenience — handles
            // viewport mapping and the math for us.
            qsa->ensureWidgetVisible(focused, margin, margin);
        } else if (qasa) {
            // Generic QAbstractScrollArea (e.g. QListView, QTreeView).
            // ensureWidgetVisible doesn't exist on the base class — it's
            // a QScrollArea-only convenience. For these, the visible
            // child widget is usually itself the viewport (or its
            // children are managed by an item-model), so a simple
            // viewport-relative position-and-scroll is the right move.
            //
            // Map the focused widget's TOP into the viewport's
            // coordinate system, then scroll the vbar so that y
            // is at least `margin` from the top of the viewport.
            // This pushes the input UP, well clear of the keyboard.
            QScrollBar *vbar = qasa->verticalScrollBar();
            if (vbar) {
                const QPoint inVp = focused->mapTo(qasa->viewport(), QPoint(0, 0));
                // If the input is already in the upper third, no
                // scroll needed — avoids re-snapping a comfortably
                // visible input.
                if (inVp.y() > qasa->viewport()->height() / 3) {
                    const int delta = inVp.y() - margin;
                    vbar->setValue(vbar->value() + delta);
                }
            }
        } else {
            qCDebug(lcUi) << "Keyboard shown but focused widget has no scroll-area ancestor"
                          << "— no auto-scroll possible. Focused:"
                          << focused->metaObject()->className();
            return;
        }
        qCDebug(lcUi) << "Keyboard shown — scrolled" << focused->metaObject()->className()
                      << "into view (keyboard rect:" << rect << ")";
    });
}

void MainWindow::showEvent(QShowEvent *e)
{
    QMainWindow::showEvent(e);
}

void MainWindow::resizeEvent(QResizeEvent *e)
{
    QMainWindow::resizeEvent(e);
    repositionOfflineBanner();
    if (m_edgeHint) m_edgeHint->setGeometry(m_swiper->rect());
    // V32.32: keep ModalSurface filling the entire main window. Its own
    // internal resizeEvent propagates the size to all layer containers.
    if (m_modalSurface) m_modalSurface->setGeometry(rect());
}

void MainWindow::repositionOfflineBanner()
{
    if (!m_offlineBanner) return;
    m_offlineBanner->setGeometry(0, 0, width(), 32);
}

void MainWindow::hideLoginOverlay()
{
    if (m_loginOverlay) m_loginOverlay->hide();
}

// ─── First-run swipe tutorial ─────────────────────────────────────────────────

void MainWindow::showSwipeHintIfFirstRun()
{
    QSettings s(QStringLiteral("SnapCloneProject"), QStringLiteral("SnapClone"));
    if (s.value(QStringLiteral("ux/swipeHintShown"), false).toBool()) return;

    m_swipeHint = new QWidget(this);
    m_swipeHint->setAttribute(Qt::WA_TranslucentBackground);
    m_swipeHint->setGeometry(rect());
    m_swipeHint->raise();

    auto *layout = new QVBoxLayout(m_swipeHint);
    layout->setAlignment(Qt::AlignCenter);
    layout->setSpacing(16);
    layout->setContentsMargins(32, 0, 32, 120);

    auto *scrim = new QWidget(m_swipeHint);
    scrim->setGeometry(0, 0, width(), height());
    scrim->setStyleSheet(QStringLiteral("background: rgba(0,0,0,0.72); border-radius: 0;"));
    scrim->lower();

    auto *arrowRow = new QHBoxLayout;
    arrowRow->setSpacing(8);
    arrowRow->setAlignment(Qt::AlignCenter);

    auto *leftArrow  = new QLabel(QStringLiteral("←"), m_swipeHint);
    auto *rightArrow = new QLabel(QStringLiteral("→"), m_swipeHint);
    auto *dotRow     = new QLabel(QStringLiteral("● ● ● ○"), m_swipeHint);

    const QString arrowStyle = QStringLiteral(
        "color: %1; font-size: 36px; background: transparent;")
        .arg(QLatin1String(Theme::ACCENT()));
    leftArrow->setStyleSheet(arrowStyle);
    rightArrow->setStyleSheet(arrowStyle);
    dotRow->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 18px; letter-spacing: 4px; background: transparent;")
        .arg(QLatin1String(Theme::ACCENT())));

    arrowRow->addWidget(leftArrow);
    arrowRow->addSpacing(8);
    arrowRow->addWidget(dotRow);
    arrowRow->addSpacing(8);
    arrowRow->addWidget(rightArrow);

    auto *leftFx  = new QGraphicsOpacityEffect(leftArrow);
    auto *rightFx = new QGraphicsOpacityEffect(rightArrow);
    leftArrow->setGraphicsEffect(leftFx);
    rightArrow->setGraphicsEffect(rightFx);

    auto *leftPulse = new QPropertyAnimation(leftFx, "opacity", m_swipeHint);
    leftPulse->setDuration(900); leftPulse->setStartValue(0.2);
    leftPulse->setKeyValueAt(0.5, 1.0); leftPulse->setEndValue(0.2);
    leftPulse->setEasingCurve(QEasingCurve::SineCurve);
    leftPulse->setLoopCount(-1); leftPulse->start();

    auto *rightPulse = new QPropertyAnimation(rightFx, "opacity", m_swipeHint);
    rightPulse->setDuration(900); rightPulse->setStartValue(1.0);
    rightPulse->setKeyValueAt(0.5, 0.2); rightPulse->setEndValue(1.0);
    rightPulse->setEasingCurve(QEasingCurve::SineCurve);
    rightPulse->setLoopCount(-1); rightPulse->start();

    auto *title = new QLabel(tr("Swipe between pages"), m_swipeHint);
    title->setAlignment(Qt::AlignCenter);
    title->setStyleSheet(QStringLiteral(
        "color: white; font-size: 22px; font-weight: 600; background: transparent;"));

    auto *sub = new QLabel(
        tr("Swipe in from the left or right edge — or make a long swipe —\n"
           "to move between Camera, Inbox, Friends and Settings"),
        m_swipeHint);
    sub->setAlignment(Qt::AlignCenter);
    sub->setWordWrap(true);
    sub->setStyleSheet(QStringLiteral(
        "color: rgba(255,255,255,0.75); font-size: 15px; background: transparent;"));

    auto *tabHint = new QLabel(
        tr("A short flick in the centre switches tabs within a page.\n"
           "The dots below show which page you're on."),
        m_swipeHint);
    tabHint->setAlignment(Qt::AlignCenter);
    tabHint->setWordWrap(true);
    tabHint->setStyleSheet(QStringLiteral(
        "color: rgba(255,255,255,0.55); font-size: 13px; background: transparent;"));

    auto *gotIt = new QPushButton(tr("Got it!"), m_swipeHint);
    gotIt->setFixedSize(160, 48);
    gotIt->setObjectName(QStringLiteral("accentBtn"));
    gotIt->setCursor(Qt::PointingHandCursor);

    layout->addStretch(1);
    layout->addLayout(arrowRow);
    layout->addWidget(title);
    layout->addWidget(sub);
    layout->addSpacing(8);
    layout->addWidget(tabHint);
    layout->addSpacing(16);
    layout->addWidget(gotIt, 0, Qt::AlignCenter);
    layout->addStretch(1);

    connect(gotIt, &QPushButton::clicked, this, &MainWindow::dismissSwipeHint);

    auto *overlayFx = new QGraphicsOpacityEffect(m_swipeHint);
    m_swipeHint->setGraphicsEffect(overlayFx);
    auto *fadeIn = new QPropertyAnimation(overlayFx, "opacity", m_swipeHint);
    fadeIn->setDuration(350); fadeIn->setStartValue(0.0); fadeIn->setEndValue(1.0);
    fadeIn->start(QAbstractAnimation::DeleteWhenStopped);

    m_swipeHint->show();
}

void MainWindow::dismissSwipeHint()
{
    if (!m_swipeHint) return;
    QSettings(QStringLiteral("SnapCloneProject"), QStringLiteral("SnapClone"))
        .setValue(QStringLiteral("ux/swipeHintShown"), true);

    auto *fx = qobject_cast<QGraphicsOpacityEffect *>(m_swipeHint->graphicsEffect());
    if (fx) {
        auto *fade = new QPropertyAnimation(fx, "opacity", m_swipeHint);
        fade->setDuration(220); fade->setStartValue(1.0); fade->setEndValue(0.0);
        connect(fade, &QPropertyAnimation::finished, m_swipeHint, &QWidget::deleteLater);
        fade->start(QAbstractAnimation::DeleteWhenStopped);
    } else {
        m_swipeHint->deleteLater();
    }
    m_swipeHint = nullptr;
}

void MainWindow::runSnapExport()
{
    // V32.34+ monthly export action. Ask where to save, then archive the snap
    // media folder via SnapExporter (zip if available, else tar.gz). The
    // reminder card has already dismissed itself by the time this runs.
    const QString suggested =
        QStandardPaths::writableLocation(QStandardPaths::DownloadLocation)
        + QLatin1Char('/') + SnapExporter::suggestedFileName();
    const QString path = QFileDialog::getSaveFileName(
        this, tr("Export snaps"), suggested,
        tr("Archives (*.zip *.tar.gz)"));
    if (path.isEmpty()) return;   // user cancelled

    const auto result = SnapExporter::createArchive(path);
    if (!result.ok) {
        showHintToast(QStringLiteral("export_failed"),
                      tr("Export failed"),
                      result.error.isEmpty() ? tr("Could not create the archive.")
                                             : result.error);
        return;
    }
    if (result.fileCount == 0) {
        showHintToast(QStringLiteral("export_empty"),
                      tr("Nothing to export"),
                      tr("You don't have any saved snap media yet."));
        return;
    }
    // V32.42+: the user chose to save this batch — keep the in-app copies so the
    // 30-day auto-expiry no longer purges them (they're now preserved both in the
    // archive and in-app). Ignoring the reminder, by contrast, lets them expire.
    if (m_controller && m_controller->storageManager()) {
        const int kept = m_controller->storageManager()->keepAllSnaps();
        qInfo("Export: kept %d snap(s) from auto-expiry", kept);
    }
    showHintToast(QStringLiteral("export_done"),
                  tr("Exported %n snap(s)", "", result.fileCount),
                  tr("Saved to %1").arg(QFileInfo(result.archivePath).fileName()));
}

// ─── Hint toast ───────────────────────────────────────────────────────────────

void MainWindow::showHintToast(const QString &key, const QString &title, const QString &body)
{
    Q_UNUSED(key);
    auto *toast = new QWidget(this);
    toast->setAttribute(Qt::WA_DeleteOnClose);
    toast->setStyleSheet(QStringLiteral(
        "background: %1; border-radius: 14px; border: 1px solid %2;")
        .arg(QLatin1String(Theme::BG_CARD()))
        .arg(QLatin1String(Theme::ACCENT())));

    auto *lay = new QVBoxLayout(toast);
    lay->setContentsMargins(16, 12, 16, 12);
    lay->setSpacing(4);

    auto *titleLbl = new QLabel(title, toast);
    titleLbl->setWordWrap(true);
    titleLbl->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 14px; font-weight: 600; background: transparent;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    lay->addWidget(titleLbl);

    if (!body.isEmpty()) {
        auto *bodyLbl = new QLabel(body, toast);
        bodyLbl->setWordWrap(true);
        bodyLbl->setStyleSheet(QStringLiteral(
            "color: %1; font-size: 13px; background: transparent;")
            .arg(QLatin1String(Theme::TEXT_SECONDARY())));
        lay->addWidget(bodyLbl);
    }

    auto *dismiss = new QPushButton(tr("Got it"), toast);
    dismiss->setStyleSheet(QStringLiteral(
        "QPushButton { background: %1; color: white; border: none;"
        "  border-radius: 6px; font-size: 12px; padding: 4px 12px; }"
        "QPushButton:pressed { background: %2; }")
        .arg(QLatin1String(Theme::ACCENT()))
        .arg(QLatin1String(Theme::ACCENT_DIM())));
    connect(dismiss, &QPushButton::clicked, toast, &QWidget::deleteLater);
    lay->addWidget(dismiss, 0, Qt::AlignRight);

    // Constrain toast to max 60% of window height so it never covers the tab bar
    // on small screens. On mobile (< 420px wide) use tighter margins and smaller font.
    const int margin   = (width() < 420) ? 8 : 12;
    const int tbH      = (m_unifiedBar && m_unifiedBar->isVisible())
                           ? m_unifiedBar->height()
                           : (m_tabBar ? m_tabBar->height() : 56);
    const int maxH     = (height() - tbH) * 6 / 10; // 60% of usable height
    toast->setMaximumWidth(width() - margin * 2);
    toast->setMaximumHeight(maxH);
    toast->adjustSize();
    const int toastH = qMin(toast->sizeHint().height(), maxH);
    const int toastY = height() - tbH - toastH - margin - 4;
    toast->setGeometry(margin, toastY, width() - margin * 2, toastH);

    auto *fx   = new QGraphicsOpacityEffect(toast);
    toast->setGraphicsEffect(fx);
    auto *anim = new QPropertyAnimation(fx, "opacity", toast);
    anim->setDuration(280); anim->setStartValue(0.0); anim->setEndValue(1.0);
    anim->setEasingCurve(QEasingCurve::OutCubic);
    anim->start(QAbstractAnimation::DeleteWhenStopped);

    toast->raise();
    toast->show();
    // Force-close after 8s. close() honours WA_DeleteOnClose AND ensures
    // the widget is hidden even if delete is delayed.
    QTimer::singleShot(8000, toast, [toast]() {
        if (toast) toast->close();
    });
}

void MainWindow::showCelebration(const QString &key, const QString &title, const QString &body)
{
    Q_UNUSED(key);
    auto *toast = new QWidget(this);
    toast->setAttribute(Qt::WA_DeleteOnClose);
    // CRITICAL: don't capture mouse/touch events. Without this, the toast can
    // sit invisible on top of the screen during the fade-in animation and
    // block all UI input — appearing as "screen went dark, app frozen."
    toast->setAttribute(Qt::WA_TransparentForMouseEvents, true);
    toast->setStyleSheet(QStringLiteral("background: %1; border-radius: 14px;")
        .arg(QLatin1String(Theme::ACCENT())));

    auto *lay = new QVBoxLayout(toast);
    lay->setContentsMargins(16, 12, 16, 12);
    lay->setSpacing(4);

    auto *titleLbl = new QLabel(title, toast);
    titleLbl->setWordWrap(true);
    titleLbl->setStyleSheet(QStringLiteral(
        "color: white; font-size: 15px; font-weight: 700; background: transparent;"));
    lay->addWidget(titleLbl);

    if (!body.isEmpty()) {
        auto *bodyLbl = new QLabel(body, toast);
        bodyLbl->setWordWrap(true);
        bodyLbl->setStyleSheet(QStringLiteral(
            "color: rgba(255,255,255,0.88); font-size: 13px; background: transparent;"));
        lay->addWidget(bodyLbl);
    }

    const int margin   = (width() < 420) ? 8 : 12;
    const int tbH      = (m_unifiedBar && m_unifiedBar->isVisible())
                           ? m_unifiedBar->height()
                           : (m_tabBar ? m_tabBar->height() : 56);
    const int maxH     = (height() - tbH) * 5 / 10;
    toast->setMaximumWidth(width() - margin * 2);
    toast->setMaximumHeight(maxH);
    toast->adjustSize();
    const int toastH = qMin(toast->sizeHint().height(), maxH);
    toast->setGeometry(margin, height() - tbH - toastH - margin - 4,
                       width() - margin * 2, toastH);

    // Opacity fade — OutCubic (NOT OutBack — overshoot causes brief invisibility
    // because the curve dips below 0 and QGraphicsOpacityEffect clamps to 0).
    auto *fx   = new QGraphicsOpacityEffect(toast);
    toast->setGraphicsEffect(fx);
    auto *anim = new QPropertyAnimation(fx, "opacity", toast);
    anim->setDuration(280); anim->setStartValue(0.0); anim->setEndValue(1.0);
    anim->setEasingCurve(QEasingCurve::OutCubic);
    anim->start(QAbstractAnimation::DeleteWhenStopped);

    toast->raise();
    toast->show();

    // Use close() not just deleteLater so WA_DeleteOnClose triggers.
    // close() also forces a hide which guarantees the toast goes away even if
    // delete is delayed.
    QTimer::singleShot(6000, toast, [toast]() {
        if (toast) toast->close();
    });
}

} // namespace SnapClone::UI

// NOTE: the stray `#include "MainWindow.moc"` was restored after an
// attempted removal caused build issues. AutoMoc emits a benign
// warning about it ("No relevant classes found") but its absence
// may interact poorly with some build-graph configurations. Keep
// the include even though there's no in-cpp Q_OBJECT to moc.
#include "MainWindow.moc"
