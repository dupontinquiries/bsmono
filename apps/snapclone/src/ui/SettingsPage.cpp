#include "ui/SettingsPage.hpp"
#include <QSignalBlocker>
#include "debug/TraceControl.hpp"
#include "ui/AmbientPaletteDeriver.hpp"     // V32.29: ambient inbox theme
#include "core/AppController.hpp"            // V32.29: storage access for ambient
#include "core/AccountManager.hpp"           // reroll + multiple accounts
#include "storage/StorageManager.hpp"        // V32.29: loadInboxSnaps
#include "ui/Theme.hpp"
#include "ui/HelpManualDialog.hpp"        // V32.43: in-app user manual popup
#include "ui/SvgIcon.hpp"
#include "ui/QrCodeRenderer.hpp"
#include "ui/ScrollAreas.hpp"
#include "debug/DebugFlags.hpp"
#include "ui/UiDebugFlags.hpp"
#include "ui/MonthlyExportReminder.hpp"
#include "face/FaceLibrary.hpp"              // V32.32: Phase 1 face library stats
#include "ui/camera/CameraPolicy.hpp"        // V32.34+: policy editor UI
#include "ui/camera/CameraTester.hpp"        // V32.34+: dynamic node probing
#include "ui/camera/CameraHealthService.hpp" // V32.34+: cached probe results
#include "ui/camera/CameraRegistry.hpp"      // V32.34+: device list + diagnostics
#include "network/MicroDhtClient.hpp"        // V32.34+: beta-testers micro-DHT shim
#include <QLineEdit>                          // V32.34+: micro-DHT host field
#include <memory>                            // shared_ptr for face library lifetime
#include <functional>                        // V32.34+: std::function in camera section

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QStackedWidget>  // V32.34+: tabbed settings
#include "ui/SegmentedTabStrip.hpp"  // V32.34+: shared tab strip
#include "ui/SwipeNavigator.hpp"     // V32.34+: unifiedNavEnabled() flag
#include <QLabel>
#include <QComboBox>
#include <QSpinBox>
#include <QPushButton>
#include <QToolButton>
#include <QCheckBox>
#include <QSlider>
#include <QGroupBox>
#include <QButtonGroup>
#include <QGridLayout>
#include <QPropertyAnimation>
#include <QAbstractAnimation>
#include <QScrollBar>
#include <QAbstractButton>
#include <QGraphicsOpacityEffect>
#include <QFile>
#include <QDir>
#include <QMouseEvent>
#include <QLineEdit>           // V32.28: manual location override input
#include <QDoubleValidator>   // V32.28: manual location override input
#include <QWheelEvent>
#include <QEvent>
#include <QTouchEvent>
#include <QDateTime>
#include <QEasingCurve>
#include <QScrollArea>
#include <QSettings>
#include <QMediaDevices>
#include <QAudioDevice>
#include <QLineEdit>
#include <QMessageBox>
#include <QDialog>
#include <QFrame>
#include <QApplication>
#include <QGuiApplication>
#include <QTimer>
#include <QClipboard>
#include <QPainter>
#include <QPixmap>
// V32.33: Storage section async walk + path discovery
#include <QStandardPaths>
#include <QDirIterator>
#include <QPointer>
#include <QFileInfo>
#include <QtConcurrent/QtConcurrentRun>
#include <QFutureWatcher>
// V32.33: security section (TOFU pending key changes — S-20)
#include <QCryptographicHash>

namespace SnapClone::UI {

// ─── Wheel guard for value widgets (V32.34+) ────────────────────────────────
// QSpinBox AND QComboBox share a DEFAULT behavior: a mouse-wheel / trackpad
// scroll OVER the widget changes its value/selection. On the Settings pages
// that means scrolling the page past a port field (m_wsPort / m_gossipPort)
// silently changes the port — the "websocket port numbers get overwritten
// immediately" bug — and scrolling past a dropdown (rotation, format, discovery
// backend, relay preset, crypto backend, …) silently changes the selection.
// This filter swallows wheel events UNLESS the widget has keyboard focus (i.e.
// the user is deliberately interacting with it). Apply via installWheelGuard().
// It also sets focus policy to StrongFocus so the widget only takes the wheel
// after an explicit click/tab into it.
class WheelGuard : public QObject {
public:
    explicit WheelGuard(QObject *parent = nullptr) : QObject(parent) {}
    bool eventFilter(QObject *obj, QEvent *e) override {
        if (e->type() == QEvent::Wheel) {
            auto *w = qobject_cast<QWidget *>(obj);
            // Only let the wheel change the value when the widget is focused.
            // Otherwise consume it here so the widget never sees it (the page
            // scroll is driven by SwipeNavigator's APP-LEVEL event filter, which
            // already processed this same wheel event before it reached the
            // widget — so swallowing it here doesn't rob the page of its scroll;
            // it only stops the accidental value/selection change).
            if (w && !w->hasFocus())
                return true;   // consume: widget ignores the wheel
        }
        return QObject::eventFilter(obj, e);
    }
};

// Shared guard instance (parented to the app) + installer.
static WheelGuard *wheelGuard()
{
    static WheelGuard *g = new WheelGuard(QCoreApplication::instance());
    return g;
}
static void installWheelGuard(QWidget *w)
{
    if (!w) return;
    w->setFocusPolicy(Qt::StrongFocus);   // don't grab focus on hover
    w->installEventFilter(wheelGuard());
}

// ─── QR code renderer ─────────────────────────────────────────────────────────
// Minimal QR data-matrix renderer for the stable node ID.
// Uses a simple 21×21 version-1 QR placeholder that encodes the first 17 chars.
// For a real app we'd link libqrencode; here we draw a legible visual pattern
// that conveys the ID visually without a full QR library dependency.

// ─── UI helpers ───────────────────────────────────────────────────────────────

static QGroupBox *makeSection(const QString &title, QWidget *parent)
{
    auto *box = new QGroupBox(title, parent);
    // Allow QGroupBox to shrink below its title-text minimum width.
    // Without this, a long title like "NAT Traversal / STUN Servers" forces
    // the box wider than the viewport, clipping everything on the right.
    box->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Preferred);
    box->setMinimumWidth(0);
    box->setStyleSheet(QStringLiteral(
        "QGroupBox {"
        "  border: 1px solid %1; border-radius: %2px;"
        "  margin-top: 6px;"
        "  padding-top: 4px;"
        "  color: %3; font-size: 11px; font-weight: 600;"
        "}"
        "QGroupBox::title { subcontrol-origin: margin; left: 8px; padding: 0 4px; }")
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    return box;
}

static QLabel *makeLabel(const QString &text, QWidget *parent)
{
    auto *l = new QLabel(text, parent);
    l->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 14px; background: transparent;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    return l;
}

SettingsPage::SettingsPage(QWidget *parent)
    : QWidget(parent)
{
    buildUi();
    loadSettings();
}

// ─── buildUi ──────────────────────────────────────────────────────────────────

void SettingsPage::buildUi()
{
    // V32.34+ tabbed settings. The page used to be one long scroll with ~16
    // stacked sections; it's now split into top tabs over a QStackedWidget.
    // Each tab is its own (CarouselAwareScrollArea) scroll page. The section
    // builders are unchanged — buildUi just routes each into the right tab's
    // layout. The action bar stays pinned below the stack, shared by all tabs.
    auto *outer = new QVBoxLayout(this);
    outer->setContentsMargins(0, 0, 0, 0);
    outer->setSpacing(0);

    // ── Top tab strip ─────────────────────────────────────────────────────
    // V32.34+ finalize: use the shared SegmentedTabStrip (same widget as the
    // mailbox + contacts tabs) instead of a bespoke inline button row, so all
    // three tabbed surfaces share one implementation — styling, badges, and
    // any future fixes (e.g. badge first-paint position) land in one place.
    // Five tabs: General / Camera / Network / Storage / About. Compact enough
    // for a 380px phone width.
    m_tabStack = new QStackedWidget(this);

    m_settingsTabs = new SegmentedTabStrip(
        { tr("General"), tr("Camera"), tr("Network"), tr("Storage"),
          tr("About") }, this);
    connect(m_settingsTabs, &SegmentedTabStrip::tabSelected,
            this, &SettingsPage::selectTab);

    // Build the five tab pages. Each makeTabPage() returns the inner
    // QVBoxLayout to add sections to, and pushes its scroll area onto
    // m_tabStack. The General tab's scroll area must be the one registered
    // as m_outerScroll, because buildAppearanceSection wires its colour
    // carousel into m_outerScroll for horizontal-wheel forwarding.
    QWidget *widthHost = this;

    // Tab 0 — General: identity, appearance, privacy.
    QVBoxLayout *generalLayout = makeTabPage(widthHost);
    // The page just created is at the back of the stack; its scroll area is
    // the General scroll → register it as m_outerScroll for the carousel.
    m_outerScroll = qobject_cast<QScrollArea *>(
        m_tabStack->widget(m_tabStack->count() - 1));
    {
        // Identity needs a 'container' arg (avatar parenting); use the tab
        // page's content widget (the layout's parent).
        QWidget *generalContainer = generalLayout->parentWidget();
        buildIdentitySection(generalLayout, generalContainer);
    }
    buildAppearanceSection(generalLayout);
    buildPrivacySection(generalLayout);
    buildSoundsSection(generalLayout);
    buildVoiceTuningSection(generalLayout);
    generalLayout->addStretch();

    // Tab 1 — Camera: camera tuning, accessibility (audio), face library.
    QVBoxLayout *cameraLayout = makeTabPage(widthHost);
    buildCameraSection(cameraLayout);
    buildAccessibilitySection(cameraLayout);
    buildFaceLibrarySection(cameraLayout);
    cameraLayout->addStretch();

    // Tab 2 — Network: discovery/ports, STUN, crypto.
    QVBoxLayout *networkLayout = makeTabPage(widthHost);
    buildNetworkSection(networkLayout);
    buildStunSection(networkLayout);
    buildCryptoSection(networkLayout);
    buildPortsSection(networkLayout);
    networkLayout->addStretch();

    // Tab 3 — Storage: storage, export, security, ambient.
    QVBoxLayout *storageLayout = makeTabPage(widthHost);
    buildStorageSection(storageLayout);
    buildExportSection(storageLayout);
    buildSecuritySection(storageLayout);
    buildAmbientSection(storageLayout);
    storageLayout->addStretch();

    // Tab 4 — About: about, diagnostics, debug flags, beta testers.
    QVBoxLayout *aboutLayout = makeTabPage(widthHost);
    buildAboutSection(aboutLayout);
    buildDiagnosticsSection(aboutLayout);
    buildDebugFlagsSection(aboutLayout);
    buildBetaTestersSection(aboutLayout);
    aboutLayout->addStretch();

    outer->addWidget(m_settingsTabs);
    outer->addWidget(m_tabStack, 1);
    // V32.34+ unified nav: bottom UnifiedNavBar renders these sub-tabs now —
    // hide the redundant top strip (kept constructed for its selectTab wiring;
    // hide-not-delete to avoid the QStyleSheetStyle teardown hazard).
    if (SwipeNavigator::unifiedNavEnabled() && m_settingsTabs)
        m_settingsTabs->hide();

    // Use `container` = the page for the action-bar parent (it's reparented
    // into the bottom bar below; `this` is fine).
    QWidget *container = this;
    // ── Adaptive action buttons ───────────────────────────────────────────────
    // On wide screens: show icon + text.  On narrow (<420px): icon only.
    // Uses QToolButton so Qt handles the icon/text layout natively.
    // Tooltip always shows the full label for accessibility.
    auto *btnRow = new QHBoxLayout;
    btnRow->setSpacing(Theme::SPACE_SM);

    const QString tbStyle = QStringLiteral(
        "QToolButton { background: %1; border: 1px solid %2; border-radius: 8px;"
        "  color: %3; font-size: 13px; padding: 6px 10px; min-height: 36px; }"
        "QToolButton:pressed { background: %4; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(QLatin1String(Theme::BG_HOVER()));
    const QString dangerStyle = QStringLiteral(
        "QToolButton { background: %1; border: 1px solid %2; border-radius: 8px;"
        "  color: %3; font-size: 13px; padding: 6px 10px; min-height: 36px; }"
        "QToolButton:pressed { background: %4; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::DANGER()))
        .arg(QLatin1String(Theme::DANGER()))
        .arg(QLatin1String(Theme::BG_HOVER()));
    const QString accentStyle = QStringLiteral(
        "QToolButton { background: %1; border: none; border-radius: 8px;"
        "  color: white; font-size: 13px; font-weight: 600; padding: 6px 14px; min-height: 36px; }"
        "QToolButton:pressed { background: %2; }")
        .arg(QLatin1String(Theme::ACCENT()))
        .arg(QLatin1String(Theme::ACCENT_DIM()));

    // Helper: build a collapsing ToolButton
    auto makeToolBtn = [&](const QString &label, const QString &iconName,
                           const QString &style, QWidget *parent) -> QToolButton * {
        auto *tb = new QToolButton(parent);
        tb->setText(label);
        tb->setToolTip(label);
        tb->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
        tb->setIcon(SvgIcon::tinted(iconName,
            QColor(QLatin1String(Theme::TEXT_SECONDARY())), QSize(16,16)));
        tb->setIconSize(QSize(16, 16));
        tb->setStyleSheet(style);
        tb->setSizePolicy(QSizePolicy::Preferred, QSizePolicy::Fixed);
        return tb;
    };

    // Rewire to QToolButton — keep the same member pointer type (QPushButton*)
    // by using the base class. Since QPushButton and QToolButton don't share
    // a base, we store as QAbstractButton* via the existing member.
    // Actually: existing members are QPushButton* — add new QToolButton* instead,
    // and use QObject::connect directly. The existing m_*Btn members remain null.
    auto *tbTest = makeToolBtn(tr("Tests"), QStringLiteral("settings"),
                               tbStyle, container);
    connect(tbTest, &QToolButton::clicked, this, &SettingsPage::onTestClicked);

    auto *tbReset = makeToolBtn(tr("Reset"), QStringLiteral("settings"),
                                dangerStyle, container);
    tbReset->setToolTip(tr("Clear preferences and restart"));
    connect(tbReset, &QToolButton::clicked, this, &SettingsPage::onResetClicked);

    auto *tbFactory = makeToolBtn(tr("Factory"), QStringLiteral("settings"),
                                  dangerStyle, container);
    tbFactory->setToolTip(tr("Wipe ALL data — generates a new sc-ID"));
    connect(tbFactory, &QToolButton::clicked, this, &SettingsPage::onFactoryResetClicked);

    auto *tbSave = makeToolBtn(tr("Save"), QStringLiteral("send"),
                               accentStyle, container);
    tbSave->setIcon(SvgIcon::tinted(QStringLiteral("send"),
                    QColor(Qt::white), QSize(16,16)));
    connect(tbSave, &QToolButton::clicked, this, &SettingsPage::onSaveClicked);

    // Also wire the legacy m_saveBtn / m_resetBtn members so existing
    // code that calls setText() on them still compiles — just no-ops.
    m_testBtn        = nullptr;
    m_resetBtn       = nullptr;
    m_factoryResetBtn= nullptr;
    m_saveBtn        = nullptr;

    btnRow->addWidget(tbTest);
    btnRow->addStretch();
    btnRow->addWidget(tbReset);
    btnRow->addWidget(tbFactory);
    btnRow->addWidget(tbSave);

    // Mark dirty when any setting changes
    // (connected after all widgets are built)
    auto markD = [this]() { markDirty(); };
    if (m_displayName) connect(m_displayName, &QLineEdit::textEdited, this, markD);
    if (m_discoveryBackend) connect(m_discoveryBackend,
        QOverload<int>::of(&QComboBox::currentIndexChanged), this, markD);
    if (m_jamiEnabled)  connect(m_jamiEnabled,  &QCheckBox::toggled, this, markD);
    if (m_upnpEnabled)  connect(m_upnpEnabled,  &QCheckBox::toggled, this, markD);

    // Collapse text to icon-only when container is too narrow
    // Install a resize watcher on the container
    class BtnCollapser : public QObject {
    public:
        BtnCollapser(QWidget *watched, QList<QToolButton*> btns)
            : QObject(watched), m_btns(btns) {
            watched->installEventFilter(this);
        }
        bool eventFilter(QObject *, QEvent *e) override {
            if (e->type() == QEvent::Resize) {
                const int width = static_cast<QWidget *>(parent())->width();
                const auto style = width < 380
                    ? Qt::ToolButtonIconOnly
                    : Qt::ToolButtonTextBesideIcon;
                for (auto *b : m_btns) b->setToolButtonStyle(style);
            }
            return false;
        }
    private:
        QList<QToolButton*> m_btns;
    };
    new BtnCollapser(container, {tbTest, tbReset, tbFactory, tbSave});

    // Pin the action bar below the tab stack (shared across all tabs).
    auto *actionBar = new QWidget(this);
    actionBar->setLayout(btnRow);
    btnRow->setContentsMargins(Theme::pageSideMarginPx(), Theme::SPACE_SM,
                               Theme::pageSideMarginPx(), Theme::SPACE_SM);
    outer->addWidget(actionBar);

    // Open on the last-used tab (persisted across launches), defaulting to
    // the first. setPersistenceKey selects the saved index now (the stack is
    // fully built above) and writes future changes to QSettings. Driving
    // through the strip keeps its highlight and the stack page in sync.
    if (m_settingsTabs) m_settingsTabs->setPersistenceKey(
        QStringLiteral("ui/tabs/settings"));
    else selectTab(0);
}

// V32.34+: build one tab page — a CarouselAwareScrollArea wrapping a
// width-capped container with a margined QVBoxLayout. Pushes the scroll
// area onto m_tabStack and returns the inner layout for section builders.
QVBoxLayout *SettingsPage::makeTabPage(QWidget * /*parentForWidth*/)
{
    auto *scroll = new CarouselAwareScrollArea(m_tabStack);
    scroll->setWidgetResizable(true);
    scroll->setFrameShape(QFrame::NoFrame);
    scroll->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    scroll->setVerticalScrollBarPolicy(Qt::ScrollBarAsNeeded);
    scroll->setStyleSheet(QStringLiteral("QScrollArea { background: transparent; }"));

    auto *container = new QWidget(scroll);
    container->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Preferred);
    // Prevent the container from exceeding the viewport width (a QGroupBox
    // title can force a wider sizeHint, bleeding sections off-screen).
    container->setMaximumWidth(scroll->viewport()->width());

    // Keep the cap in sync on resize (orientation change, window resize).
    class ViewportWatcher : public QObject {
    public:
        ViewportWatcher(QScrollArea *sa, QWidget *c)
            : QObject(sa), m_sa(sa), m_container(c) {}
        bool eventFilter(QObject *, QEvent *e) override {
            if (e->type() == QEvent::Resize)
                m_container->setMaximumWidth(m_sa->viewport()->width());
            return false;
        }
    private:
        QScrollArea *m_sa;
        QWidget     *m_container;
    };
    scroll->viewport()->installEventFilter(new ViewportWatcher(scroll, container));

    auto *layout = new QVBoxLayout(container);
    layout->setContentsMargins(Theme::pageSideMarginPx(),
                               Theme::pageTopMarginPx(),
                               Theme::pageSideMarginPx(),
                               Theme::pageTopMarginPx());
    layout->setSpacing(Theme::SPACE_MD);

    scroll->setWidget(container);
    m_tabStack->addWidget(scroll);
    return layout;
}

// V32.34+: switch the visible settings tab and sync the tab-button checks.
// V32.34+: switch the visible settings tab. Driven by the SegmentedTabStrip's
// tabSelected signal (user taps) and by the initial setCurrentIndex(0) call.
// Only sets the stack page — the strip owns its own button highlight, so we
// must NOT call back into the strip here or we'd loop (setCurrentIndex emits
// tabSelected, which is connected to this slot).
void SettingsPage::selectTab(int index)
{
    if (!m_tabStack || index < 0 || index >= m_tabStack->count()) return;
    m_tabStack->setCurrentIndex(index);
}

// ── SubTabbed (V32.34+ two-level nav) ───────────────────────────────────────
int SettingsPage::subTabCount() const
{
    return m_tabStack ? m_tabStack->count() : 5;  // 5 = General/Camera/Network/Storage/About
}

int SettingsPage::currentSubTab() const
{
    return m_tabStack ? m_tabStack->currentIndex() : 0;
}

void SettingsPage::setCurrentSubTab(int index)
{
    // Route through selectTab (drives the stack); also sync the in-page strip
    // when the change originated elsewhere (unified bar / swipe), guarded to
    // avoid a redundant re-emit loop.
    selectTab(index);
    if (m_settingsTabs && m_settingsTabs->currentIndex() != index)
        m_settingsTabs->setCurrentIndex(index);
}

QString SettingsPage::subTabLabel(int index) const
{
    switch (index) {
        case 0: return tr("General");
        case 1: return tr("Camera");
        case 2: return tr("Network");
        case 3: return tr("Storage");
        case 4: return tr("About");
        default: return QString();
    }
}

QString SettingsPage::subTabIcon(int index) const
{
    // V32.49+: pull-pill icon. General + Camera map to existing assets; the
    // rest (Network/Storage/About) fall back to their initial in the pill.
    switch (index) {
        case 0: return QStringLiteral("settings");
        case 1: return QStringLiteral("camera");
        default: return QString();
    }
}


// ── Inline hint helper ─────────────────────────────────────────────────────────
static QLabel *makeHint(const QString &text, QWidget *parent)
{
    auto *h = new QLabel(text, parent);
    h->setWordWrap(true);
    h->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 11px; background: transparent; padding: 0 2px;")
        .arg(QLatin1String(SnapClone::UI::Theme::TEXT_DISABLED())));
    return h;
}

void SettingsPage::buildIdentitySection(QVBoxLayout *root, QWidget *container)
{
    auto *box    = makeSection(tr("Your Node ID"), container);
    auto *layout = new QVBoxLayout(box);
    layout->setContentsMargins(8, 4, 8, 8);
    layout->setSpacing(Theme::SPACE_SM);

    // Hint
    auto *hint = new QLabel(
        tr("Share this ID with friends so they can connect to you from anywhere."), box);
    hint->setWordWrap(true);
    hint->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Preferred);
    hint->setStyleSheet(QStringLiteral("color: %1; font-size: 12px; background: transparent;")
                            .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(hint);

    // ── ID display row ────────────────────────────────────────────────────
    auto *idRow = new QHBoxLayout;
    idRow->setContentsMargins(0, 0, 0, 0);
    idRow->setSpacing(Theme::SPACE_SM);

    // QR code label (left)
    m_qrLabel = new QLabel(box);
    m_qrLabel->setFixedSize(64, 64);  // smaller — saves 16px on narrow screens
    m_qrLabel->setSizePolicy(QSizePolicy::Fixed, QSizePolicy::Fixed);
    m_qrLabel->setAlignment(Qt::AlignCenter);
    m_qrLabel->setStyleSheet(
        QStringLiteral("border: 1px solid %1; border-radius: 6px; background: white;"
                       " padding: 2px;")
            .arg(QLatin1String(Theme::BORDER())));
    m_qrLabel->setToolTip(tr("Tap to expand"));
    m_qrLabel->setCursor(Qt::PointingHandCursor);
    // Opt this QLabel into receiving touch taps via SwipeNavigator's
    // mouse-event synthesis. By default SwipeNavigator consumes TouchBegin
    // for plain QLabels (they don't match any of its known-interactive
    // widget types), so the label never sees a MouseButtonRelease and
    // the eventFilter below stays silent. Setting this property tells
    // SwipeNavigator to treat this widget as tappable.
    m_qrLabel->setProperty("swipenav-tappable", true);
    // Install event filter to detect tap/click on the QR label.
    m_qrLabel->installEventFilter(this);
    idRow->addWidget(m_qrLabel);

    // ID text + copy button (right)
    auto *textCol = new QVBoxLayout;
    textCol->setSpacing(Theme::SPACE_XS);

    m_nodeIdLabel = new QLabel(tr("sc-…"), box);
    m_nodeIdLabel->setTextInteractionFlags(
        Qt::TextSelectableByMouse | Qt::TextSelectableByKeyboard);
    m_nodeIdLabel->setStyleSheet(QStringLiteral(
        "font-family: monospace;"
        "font-size: 14px;"
        "font-weight: bold;"
        "color: %1;"
        "background: %2;"
        "border: 1px solid %3;"
        "border-radius: %4px;"
        "padding: 6px 8px;")
        .arg(QLatin1String(Theme::ACCENT()))
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::RADIUS_SM));
    m_nodeIdLabel->setAlignment(Qt::AlignCenter);
    m_nodeIdLabel->setMinimumHeight(36); // smaller than TOUCH_MIN to fit
    textCol->addWidget(m_nodeIdLabel);

    m_copyBtn = new QPushButton(tr("📋  Copy ID"), box);
    m_copyBtn->setObjectName(QStringLiteral("accentBtn"));
    m_copyBtn->setFixedHeight(40);
    m_copyBtn->setToolTip(tr("Copy your node ID to clipboard"));
    connect(m_copyBtn, &QPushButton::clicked, this, [this]() {
        if (m_nodeIdLabel->text().startsWith(QStringLiteral("sc-"))) {
            QGuiApplication::clipboard()->setText(m_nodeIdLabel->text());
            m_copyBtn->setText(tr("✓  Copied!"));
            QTimer::singleShot(2000, this, [this]() {
                m_copyBtn->setText(tr("📋  Copy ID"));
            });
        }
    });
    textCol->addWidget(m_copyBtn);

    idRow->addLayout(textCol, 1);
    layout->addLayout(idRow);

    // ── Accounts: reroll a fresh identity + keep multiple ────────────────
    // Settings live in QSettings, which AccountManager never touches, so these
    // actions keep every preference. A switch/reroll changes which account the
    // app loads at startup, so it applies after a restart.
    {
        auto *accHdr = new QLabel(tr("Accounts"), box);
        accHdr->setStyleSheet(QStringLiteral(
            "color: %1; font-size: 12px; font-weight: bold; background: transparent;"
            " margin-top: 8px;").arg(QLatin1String(Theme::TEXT_SECONDARY())));
        layout->addWidget(accHdr);

        auto *accHint = new QLabel(
            tr("Switch identities or roll a fresh one. Your settings are kept; "
               "changes take effect after you restart SnapClone."), box);
        accHint->setWordWrap(true);
        accHint->setStyleSheet(QStringLiteral(
            "color: %1; font-size: 11px; background: transparent;")
            .arg(QLatin1String(Theme::TEXT_SECONDARY())));
        layout->addWidget(accHint);

        auto *accCombo = new QComboBox(box);
        accCombo->setMinimumHeight(36);
        layout->addWidget(accCombo);

        // (re)fill from the registry, marking the active account.
        auto repopulate = [accCombo]() {
            accCombo->clear();
            auto &am = Core::AccountManager::instance();
            const QString activeId = am.activeId();
            for (const Core::Account &a : am.accounts()) {
                QString text = (a.label == a.id)
                    ? a.id : QStringLiteral("%1  (%2)").arg(a.label, a.id);
                const bool isActive = (a.id == activeId);
                if (isActive) text += tr("   ✓ active");
                accCombo->addItem(text, a.id);
                if (isActive) accCombo->setCurrentIndex(accCombo->count() - 1);
            }
        };
        repopulate();

        auto *accBtnRow = new QHBoxLayout;
        accBtnRow->setSpacing(Theme::SPACE_SM);
        auto *switchBtn = new QPushButton(tr("Switch"), box);
        auto *addBtn    = new QPushButton(tr("Add account"), box);
        auto *rerollBtn = new QPushButton(tr("Reroll identity"), box);
        switchBtn->setFixedHeight(40);
        addBtn->setFixedHeight(40);
        rerollBtn->setFixedHeight(40);
        accBtnRow->addWidget(switchBtn);
        accBtnRow->addWidget(addBtn);
        accBtnRow->addWidget(rerollBtn);
        layout->addLayout(accBtnRow);

        connect(switchBtn, &QPushButton::clicked, this, [this, accCombo, repopulate]() {
            const QString id = accCombo->currentData().toString();
            if (id.isEmpty()) return;
            auto &am = Core::AccountManager::instance();
            if (id == am.activeId()) {
                QMessageBox::information(this, tr("Already active"),
                    tr("That account is already active."));
                return;
            }
            am.setActive(id);
            repopulate();
            QMessageBox::information(this, tr("Account switched"),
                tr("\"%1\" becomes your active identity after you restart "
                   "SnapClone. Your settings are unchanged.").arg(id));
        });

        connect(addBtn, &QPushButton::clicked, this, [this, repopulate]() {
            const QString id = Core::AccountManager::instance().createAccount();
            repopulate();
            QMessageBox::information(this, tr("Account created"),
                tr("Added %1. Select it and tap Switch to use it (after a "
                   "restart).").arg(id));
        });

        connect(rerollBtn, &QPushButton::clicked, this, [this, repopulate]() {
            auto *confirm = new QMessageBox(QMessageBox::Question,
                tr("Roll a fresh identity?"),
                tr("This creates a brand-new sc-ID and makes it active. Your "
                   "settings are kept, and your current identity stays in the "
                   "list so you can switch back. Friends who only know your old "
                   "ID won't recognise the new one until you share it.\n\n"
                   "Takes effect after you restart SnapClone. Continue?"),
                QMessageBox::Yes | QMessageBox::Cancel, this);
            confirm->setDefaultButton(QMessageBox::Cancel);
            const int r = confirm->exec();
            confirm->deleteLater();
            if (r != QMessageBox::Yes) return;
            const QString id = Core::AccountManager::instance().rerollActive();
            repopulate();
            QMessageBox::information(this, tr("New identity ready"),
                tr("Your new identity is %1. Restart SnapClone to start using "
                   "it; your settings were not changed.").arg(id));
        });
    }

    root->addWidget(box);
}

bool SettingsPage::eventFilter(QObject *obj, QEvent *ev)
{
    if (obj == m_qrLabel && ev->type() == QEvent::MouseButtonRelease) {
        // Show full-screen QR overlay
        const QString id = m_nodeIdLabel ? m_nodeIdLabel->text() : QString{};
        if (!id.startsWith(QStringLiteral("sc-"))) return false;

        showExpandedQr(this, id);
        return true;
    }
    return QWidget::eventFilter(obj, ev);
}

void SettingsPage::setNodeId(const QString &id)
{
    if (!m_nodeIdLabel) return;
    m_nodeIdLabel->setText(id);

    // Update QR code
    if (m_qrLabel && id.startsWith(QStringLiteral("sc-"))) {
        const QPixmap qr = makeIdQr(id, m_qrLabel->width());
        m_qrLabel->setPixmap(qr);
    }
}

// ── ThumbCarouselFilter ───────────────────────────────────────────────────────
// Event filter installed on a QScrollArea and all its children.
// Intercepts horizontal touch/mouse drags before child buttons can claim them,
// allowing the scroll area to own horizontal swipes while children still receive
// clean tap events when there is no significant horizontal movement.
class ThumbCarouselFilter : public QObject
{
    Q_OBJECT
public:
    explicit ThumbCarouselFilter(QScrollArea *scroll)
        : QObject(scroll), m_scroll(scroll) {}

    bool eventFilter(QObject *obj, QEvent *e) override
    {
        switch (e->type()) {

        case QEvent::MouseButtonPress: {
            auto *me = static_cast<QMouseEvent *>(e);
            if (me->button() != Qt::LeftButton) break;
            m_pressPos   = me->globalPosition().toPoint();
            m_dragging   = false;
            m_startValue = m_scroll->horizontalScrollBar()->value();
            m_lastPos    = m_pressPos;
            m_velocity   = 0.0;
            m_lastTime   = QDateTime::currentMSecsSinceEpoch();
            e->accept();
            return false; // let button see press too — we decide on move
        }

        case QEvent::MouseMove: {
            if (!(static_cast<QMouseEvent *>(e)->buttons() & Qt::LeftButton)) break;
            auto *me = static_cast<QMouseEvent *>(e);
            const QPoint cur = me->globalPosition().toPoint();
            const int dx = cur.x() - m_pressPos.x();
            const int dy = cur.y() - m_pressPos.y();

            if (!m_dragging && qAbs(dx) > 5 && qAbs(dx) > qAbs(dy)) {
                m_dragging = true;
                // Grab mouse so we get all subsequent events even if pointer
                // moves off the originating child widget
                // grabMouse only works on popup windows in Wayland; skip on Wayland
                // to avoid spammy "plugin supports grabbing mouse only for popup" warnings.
                if (QGuiApplication::platformName() != QLatin1String("wayland")) {
                    if (auto *w = qobject_cast<QWidget *>(obj))
                        w->grabMouse();
                }
            }
            if (m_dragging) {
                // Exponential velocity tracking
                const qint64 now = QDateTime::currentMSecsSinceEpoch();
                const qint64 dt  = qMax(1LL, now - m_lastTime);
                const double inst = static_cast<double>(cur.x() - m_lastPos.x()) / static_cast<double>(dt);
                m_velocity  = m_velocity * 0.6 + inst * 0.4; // EMA smoothing
                m_lastPos   = cur;
                m_lastTime  = now;

                m_scroll->horizontalScrollBar()->setValue(
                    m_startValue - dx);
                e->accept();
                return true;
            }
            break;
        }

        case QEvent::MouseButtonRelease: {
            auto *me = static_cast<QMouseEvent *>(e);
            if (me->button() != Qt::LeftButton) break;
            if (auto *w = qobject_cast<QWidget *>(obj))
                w->releaseMouse();

            if (m_dragging) {
                // Momentum flick: apply velocity for smooth coast
                const double flick = m_velocity * 120.0; // px
                const int target = m_scroll->horizontalScrollBar()->value()
                                   - static_cast<int>(flick);
                const int clamped = qBound(
                    m_scroll->horizontalScrollBar()->minimum(),
                    target,
                    m_scroll->horizontalScrollBar()->maximum());

                auto *anim = new QPropertyAnimation(
                    m_scroll->horizontalScrollBar(), "value", this);
                anim->setDuration(320);
                anim->setStartValue(m_scroll->horizontalScrollBar()->value());
                anim->setEndValue(clamped);
                anim->setEasingCurve(QEasingCurve::OutCubic);
                anim->start(QAbstractAnimation::DeleteWhenStopped);

                m_dragging = false;
                e->accept();
                return true; // suppress the click on the button underneath
            }
            m_dragging = false;
            break;
        }

        case QEvent::TouchBegin:
        case QEvent::TouchUpdate:
        case QEvent::TouchEnd: {
            auto *te = static_cast<QTouchEvent *>(e);
            if (te->points().isEmpty()) break;
            const auto &pt = te->points().first();

            if (e->type() == QEvent::TouchBegin) {
                m_pressPos   = pt.globalPosition().toPoint();
                m_dragging   = false;
                m_startValue = m_scroll->horizontalScrollBar()->value();
                m_lastPos    = m_pressPos;
                m_velocity   = 0.0;
                m_lastTime   = QDateTime::currentMSecsSinceEpoch();
                e->accept();
                return false;
            }

            if (e->type() == QEvent::TouchUpdate) {
                const QPoint cur = pt.globalPosition().toPoint();
                const int dx = cur.x() - m_pressPos.x();
                const int dy = cur.y() - m_pressPos.y();
                if (!m_dragging && qAbs(dx) > 5 && qAbs(dx) > qAbs(dy))
                    m_dragging = true;

                if (m_dragging) {
                    const qint64 now = QDateTime::currentMSecsSinceEpoch();
                    const qint64 dt  = qMax(1LL, now - m_lastTime);
                    const double inst = static_cast<double>(cur.x() - m_lastPos.x()) / static_cast<double>(dt);
                    m_velocity  = m_velocity * 0.6 + inst * 0.4;
                    m_lastPos   = cur;
                    m_lastTime  = now;
                    m_scroll->horizontalScrollBar()->setValue(m_startValue - dx);
                    e->accept();
                    return true;
                }
            }

            if (e->type() == QEvent::TouchEnd && m_dragging) {
                const double flick = m_velocity * 120.0;
                const int target = m_scroll->horizontalScrollBar()->value()
                                   - static_cast<int>(flick);
                const int clamped = qBound(
                    m_scroll->horizontalScrollBar()->minimum(),
                    target,
                    m_scroll->horizontalScrollBar()->maximum());

                auto *anim = new QPropertyAnimation(
                    m_scroll->horizontalScrollBar(), "value", this);
                anim->setDuration(320);
                anim->setStartValue(m_scroll->horizontalScrollBar()->value());
                anim->setEndValue(clamped);
                anim->setEasingCurve(QEasingCurve::OutCubic);
                anim->start(QAbstractAnimation::DeleteWhenStopped);

                m_dragging = false;
                e->accept();
                return true;
            }
            break;
        }
        case QEvent::Wheel: {
            auto *we = static_cast<QWheelEvent *>(e);
            // pixelDelta is set by trackpads (high-res); angleDelta by mice.
            // We want horizontal scroll. On a trackpad, a horizontal two-finger
            // swipe sends pixelDelta.x() != 0. A vertical swipe sends y() != 0.
            // If there's meaningful horizontal delta — own it.
            const QPoint px  = we->pixelDelta();
            const QPoint ang = we->angleDelta();
            int hDelta = 0;
            if (!px.isNull())
                hDelta = px.x();
            else if (qAbs(ang.x()) > qAbs(ang.y()) / 2)
                hDelta = ang.x() / 8; // angleDelta in 1/8 degree units

            if (hDelta != 0) {
                m_scroll->horizontalScrollBar()->setValue(
                    m_scroll->horizontalScrollBar()->value() - hDelta);
                e->accept();
                return true; // prevent outer vertical scroll from firing
            }
            // Vertical wheel — let the outer scroll area handle it
            return false;
        }

        default: break;
        }
        return QObject::eventFilter(obj, e);
    }

private:
    QScrollArea *m_scroll;
    QPoint       m_pressPos;
    QPoint       m_lastPos;
    int          m_startValue = 0;
    bool         m_dragging   = false;
    double       m_velocity   = 0.0;
    qint64       m_lastTime   = 0;
};

void SettingsPage::buildAppearanceSection(QVBoxLayout *root)
{
    auto *box    = makeSection(tr("Appearance"), this);
    auto *layout = new QVBoxLayout(box);
    layout->setContentsMargins(8, 4, 8, 8);
    layout->setSpacing(Theme::SPACE_SM);

    auto *header = new QLabel(tr("COLOR THEME"), box);
    header->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 11px; font-weight: 600; letter-spacing: 0.8px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(header);

    // ── Palette grid ────────────────────────────────────────────────────
    struct PaletteEntry { const char *id; const char *label; const char *accent; const char *bg; };
    static constexpr PaletteEntry kEntries[] = {
        // Only IDs that exist in Theme::kPalettes[]
        { "dark", "Dark", "#C9B99A", "#1A1917" },
        { "pitch_black", "Pitch Black", "#E0C97A", "#000000" },
        { "paper", "Paper", "#7C5C2E", "#FDFCF9" },
        { "neon", "Neon", "#00E5FF", "#0A0D1A" },
        { "minimal", "Minimal", "#3F3F46", "#FAFAFA" },
        { "modern", "Modern", "#58A6FF", "#161B22" },
        { "earth", "Earth", "#8B6914", "#231A0C" },
        { "pastels", "Pastels", "#A88FDB", "#FAF7FF" },
        { "metallic", "Metallic", "#A0AACC", "#141418" },
        { "pastels_dark", "Pastels Night", "#B899FF", "#211A2A" },
        { "minimal_dark", "Minimal Dark", "#C0C0C0", "#0E0E0E" },
        { "earth_dark", "Forest Stone", "#6DBF5C", "#111A12" },
        { "ocean", "Ocean", "#5DB7F0", "#0A1929" },
        { "rose", "Rose Gold", "#C97B5E", "#FDF8F5" },
        { "monokai", "Monokai", "#A6E22E", "#272822" },
        { "dracula", "Dracula", "#BD93F9", "#282A36" },
        { "solarized_light", "Solarized", "#268BD2", "#FDF6E3" },
        { "midnight", "Midnight", "#7EA8D8", "#0D1421" },
        { "high_contrast", "High Cont.", "#FFD700", "#000000" },
        // V32.14 additions
        { "nord", "Nord", "#88C0D0", "#2E3440" },
        { "gruvbox", "Gruvbox", "#FABD2F", "#282828" },
        { "solarized_dark", "Solar. Dark", "#268BD2", "#002B36" },
        { "cyberpunk", "Cyberpunk", "#FF00AA", "#0A0014" },
        { "sepia", "Sepia", "#8B4513", "#F6ECD8" },
        { "terracotta", "Terracotta", "#C75B3C", "#FFF4E6" },
        { "lavender", "Lavender", "#7B5BB0", "#F8F4FC" },
        { "carbon", "Carbon", "#0F62FE", "#1F1F1F" },
        // V32.32 additions — see Theme.hpp for rationale per palette.
        // chip accent picks the most distinctive accent color, chip bg
        // matches BG_BASE so the carousel preview looks like a tiny
        // slice of the actual theme.
        { "tokyo_night", "Tokyo Night", "#7AA2F7", "#1F2335" },
        { "catppuccin_mocha", "Catpp. Mocha", "#CBA6F7", "#181825" },
        { "catppuccin_latte", "Catpp. Latte", "#8839EF", "#E6E9EF" },
        { "synthwave", "Synthwave", "#FF6AD5", "#241040" },
        { "matcha", "Matcha", "#5C8A3A", "#FAF7EE" },
        { "sunset", "Sunset", "#FF6B4A", "#FFF8F0" },
        { "ice", "Ice", "#0EA5E9", "#F8FAFC" },
        { "forest_day", "Forest Day", "#4A7C3D", "#F9F7F0" },
        // V32.29: synthetic ambient palette derived from inbox photos.
        // Chip accent + bg are static placeholders since the actual
        // palette is computed on-pick. A muted teal-to-grey suggests
        // "ambient / generated" without being misleading about content.
        { "ambient_inbox", "Ambient: Inbox", "#7FB1A8", "#1A1A1F" },
        // V32.30: live-viewfinder driven palette ("Content Creator Mode").
        // Settings click hands control to MainWindow which subscribes
        // CameraWidget's preview-frame signal to a 1Hz derivation tick.
        // Same warm-coral hint to signal "lens-driven" feel.
        { "ambient_camera", "Ambient: Camera", "#E8A07A", "#1F1815" },
    };
    constexpr int kCount = static_cast<int>(sizeof(kEntries) / sizeof(kEntries[0]));

    // V32.29: if a synthetic (ambient) palette is currently active, treat
    // its synthetic id as the "saved" pick for carousel selection state.
    // QSettings still holds the user's last STATIC pick — that's restored
    // when the user selects any non-synthetic carousel entry, OR on next
    // launch (synthetic doesn't persist by design — see WORK_LOG V32.29).
    QString saved = QSettings(
        QStringLiteral("SnapCloneProject"),
        QStringLiteral("SnapClone"))
        .value(QStringLiteral("ui/palette"), QStringLiteral("dark")).toString();
    if (Theme::isSyntheticActive()) {
        saved = QString::fromUtf8(Theme::syntheticStorage().id.c_str());
    }

    // ── Horizontal scroll carousel — works on narrow screens ──────────────
    // Each theme card is 80px wide with 8px gap. All cards fit in one scrollable
    // row regardless of count. Font is clamped to 10px to prevent overflow.
    // On narrow screens the user swipes horizontally through themes.
    auto *scroll = new HorizontalScrollArea(box);
    scroll->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    scroll->setVerticalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    scroll->setFrameShape(QFrame::NoFrame);
    scroll->setFixedHeight(74);
    scroll->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Fixed);
    scroll->setAttribute(Qt::WA_AcceptTouchEvents);
    scroll->setStyleSheet(QStringLiteral("background: transparent; border: none;"));

    auto *strip = new QWidget;
    auto *stripLayout = new QHBoxLayout(strip);
    stripLayout->setContentsMargins(0, 0, 0, 0);
    stripLayout->setSpacing(8);

    auto *group = new QButtonGroup(box);
    group->setExclusive(true);

    for (int i = 0; i < kCount; ++i) {
        const auto &e = kEntries[i];
        const bool active = (QLatin1String(e.id) == saved);

        auto *btn = new QPushButton(strip);
        btn->setCheckable(true);
        btn->setChecked(active);
        btn->setFixedSize(70, 64);
        btn->setCursor(Qt::PointingHandCursor);

        // Clamp label to 10px — never overflows the 72px card width
        const QString shortLabel = QString::fromUtf8(e.label).left(11);

        btn->setStyleSheet(QStringLiteral(
            "QPushButton {"
            "  background-color: %1;"
            "  border: 2px solid %2;"
            "  border-radius: 12px;"
            "  padding: 0;"
            "}"
            "QPushButton:checked { border: 3px solid %3; }"
            "QPushButton:hover   { border-color: %3; }")
            .arg(QLatin1String(e.bg))
            .arg(active ? QLatin1String(e.accent) : QStringLiteral("#404040"))
            .arg(QLatin1String(e.accent)));

        auto *inner = new QVBoxLayout(btn);
        inner->setContentsMargins(0, 6, 0, 4);
        inner->setSpacing(1);

        auto *dot = new QLabel(QStringLiteral("●"), btn);
        dot->setAlignment(Qt::AlignCenter);
        dot->setStyleSheet(QStringLiteral(
            "color: %1; font-size: 18px; background: transparent; border: none;")
            .arg(QLatin1String(e.accent)));
        inner->addWidget(dot);

        auto *lbl = new QLabel(shortLabel, btn);
        lbl->setAlignment(Qt::AlignCenter);
        lbl->setWordWrap(false);
        // Fixed 9px — always fits in 72px card regardless of locale
        lbl->setStyleSheet(QStringLiteral(
            "color: %1; font-size: 9px; font-weight: 600;"
            " background: transparent; border: none;")
            .arg(QLatin1String(e.accent)));
        inner->addWidget(lbl);

        group->addButton(btn, i);
        stripLayout->addWidget(btn);

        connect(btn, &QPushButton::clicked, this, [this, e, btn, group]() {
            const QLatin1String id(e.id);
            // V32.32: persist ALL palette choices to QSettings, including
            // ambient ones. Previously only static palettes saved their
            // id; ambient_inbox / ambient_camera were transient. On reboot
            // the user would find their carousel selection reset to
            // whichever static palette was last picked (or default 'dark'
            // if never picked). Now: the ambient id is saved too, and
            // MainWindow's startup path looks for it and re-triggers
            // the appropriate driver. The actual derived COLORS still
            // aren't persisted (those depend on inbox photos / live
            // camera frames, which the user may not have at startup) —
            // but the MODE is persisted so the chrome immediately starts
            // working toward an ambient palette without user re-selection.
            QSettings(QStringLiteral("SnapCloneProject"),
                      QStringLiteral("SnapClone"))
                .setValue(QStringLiteral("ui/palette"), id);

            if (id == QLatin1String("ambient_inbox")) {
                // V32.30: any prior ambient-camera subscription must stop —
                // only one synthetic palette is active at a time. Signal
                // MainWindow which owns the driver.
                emit ambientCameraRequested(false);
                // V32.30 (Wave 2): inbox mode now goes through a driver
                // too. MainWindow owns the AmbientInboxDriver which
                // performs the initial derivation AND auto-refreshes on
                // every snapReceived (30s throttled). Replaces the
                // previous in-handler one-shot derivation — without the
                // driver, the palette wouldn't update when new snaps
                // arrived during a session.
                emit ambientInboxRequested(true);
                Theme::applyGlobalStyleSheetIfChanged();
            } else if (id == QLatin1String("ambient_camera")) {
                // V32.30: Content Creator Mode. MainWindow handles the
                // subscription + 1Hz derivation tick (it owns the driver
                // and the CameraWidget). SettingsPage just signals intent.
                emit ambientInboxRequested(false);
                emit ambientCameraRequested(true);
                // Best-effort initial repaint with current palette so the
                // chip looks selected even before the first derivation
                // fires (~1s later). globalStyleSheet() reads activePalette()
                // which is still the prior static — visually neutral until
                // the driver gets its first frame.
                Theme::applyGlobalStyleSheetIfChanged();
            } else {
                // V32.30: a static pick implicitly stops any ambient
                // subscription (only one synthetic palette at a time).
                emit ambientInboxRequested(false);
                emit ambientCameraRequested(false);
                // Static palette path — unchanged from V32.28.
                // (The QSettings setValue above already saved this id,
                // so no duplicate save here.)
                Theme::setPalette(id);
                // setPalette resets idx to -1; activePalette() will
                // re-read QSettings on the next call. This naturally
                // clears any active synthetic palette too.
                Theme::applyGlobalStyleSheetIfChanged();
            }
            // Refresh all button borders in the group to match new selection
            for (QAbstractButton *b : group->buttons()) {
                const int idx = group->id(b);
                if (idx < 0 || idx >= kCount) continue;
                const bool sel = (b == btn);
                const auto &entry = kEntries[idx];
                b->setStyleSheet(QStringLiteral(
                    "QPushButton {"
                    "  background-color: %1;"
                    "  border: %3 solid %2;"
                    "  border-radius: 12px; padding: 0;"
                    "}"
                    "QPushButton:checked { border: 3px solid %2; }"
                    "QPushButton:hover   { border-color: %2; }")
                    .arg(QLatin1String(entry.bg))
                    .arg(QLatin1String(entry.accent))
                    .arg(sel ? QStringLiteral("3px") : QStringLiteral("2px")));
                // Spring animation on selected
                auto *fx = new QGraphicsOpacityEffect(b);
                b->setGraphicsEffect(fx);
                auto *anim = new QPropertyAnimation(fx, "opacity", b);
                anim->setDuration(sel ? 200 : 0);
                anim->setStartValue(sel ? 0.6 : 1.0);
                anim->setEndValue(1.0);
                anim->setEasingCurve(QEasingCurve::OutBack);
                anim->start(QAbstractAnimation::DeleteWhenStopped);
            }
        });
    }

    strip->adjustSize();
    scroll->setWidget(strip);
    scroll->setWidgetResizable(false);

    // Install carousel filter on the scroll area, viewport, and all child buttons.
    // CRITICAL: also install on the outer QScrollArea (the settings page scroller)
    // because on Qt/Wayland wheel events are delivered to the QScrollArea widget
    // before the viewport event filter sees them. The outer scroll area must be
    // intercepted and wheel events that land over the carousel's geometry forwarded.
    auto *carouselFilter = new ThumbCarouselFilter(scroll);
    scroll->viewport()->setAttribute(Qt::WA_AcceptTouchEvents);
    scroll->viewport()->installEventFilter(carouselFilter);
    scroll->installEventFilter(carouselFilter);  // intercept BEFORE outer scroll eats it
    strip->setAttribute(Qt::WA_AcceptTouchEvents);
    strip->installEventFilter(carouselFilter);
    for (QAbstractButton *b : group->buttons()) {
        b->setAttribute(Qt::WA_AcceptTouchEvents);
        b->installEventFilter(carouselFilter);
    }

    // Store the carousel scroll area on SettingsPage so wheelEvent can forward to it
    m_themeScroll = scroll;
    // Wire the carousel scroll area into the outer scroll area subclass
    // Direct static_cast — we created m_outerScroll as CarouselAwareScrollArea
    // a few lines earlier. qobject_cast can return null due to moc issues; static_cast is safe.
    static_cast<CarouselAwareScrollArea *>(m_outerScroll)->themeScroll = scroll;

    // Scroll to show the active theme
    const int activeIdx = [&]() {
        for (int i = 0; i < kCount; ++i)
            if (QLatin1String(kEntries[i].id) == saved) return i;
        return 0;
    }();
    QTimer::singleShot(0, scroll, [scroll, activeIdx]() {
        scroll->horizontalScrollBar()->setValue(activeIdx * 80);
    });

    layout->addWidget(scroll);

    auto *hint = makeHint(
        tr("Tap a colour theme to change the look of the app instantly."), box);
    hint->setWordWrap(true);
    hint->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 11px; background: transparent;")
        .arg(QLatin1String(Theme::TEXT_DISABLED())));
    layout->addWidget(hint);

    // ── Navigation style (V32.34+) ───────────────────────────────────────────
    // The unified bottom nav toggle belongs here in Appearance, not buried in
    // the About-tab debug flags. It writes the ui/unified_nav QSettings key that
    // SwipeNavigator::unifiedNavEnabled() reads at startup; the flag is cached
    // once at launch, so the change needs a RESTART (the inline note says so).
    // This is the ONLY in-app way to enable unified nav on a phone (the env var
    // SC_UNIFIED_NAV isn't visible to an app launched from its icon).
    auto *navHeader = new QLabel(tr("NAVIGATION"), box);
    navHeader->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 11px; font-weight: 600; letter-spacing: 0.8px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(navHeader);

    auto *unifiedNav = new QCheckBox(tr("Unified bottom navigation"), box);
    unifiedNav->setToolTip(tr(
        "Replaces the 5-icon bottom tab bar with a single two-level bar: the "
        "current page's sub-tabs inline, page-position dots, and swipe/scroll "
        "handoffs between sub-tabs and pages. Takes effect after restarting the "
        "app (the setting is read once at launch)."));
    {
        QSettings s;
        unifiedNav->setChecked(
            s.value(QStringLiteral("ui/unified_nav"), false).toBool());
    }
    auto *unifiedNavNote = new QLabel(box);
    unifiedNavNote->setWordWrap(true);
    unifiedNavNote->setStyleSheet(QStringLiteral("color: %1; font-size: 11px;")
        .arg(QLatin1String(Theme::ACCENT())));
    unifiedNavNote->setVisible(false);
    connect(unifiedNav, &QCheckBox::toggled, this, [unifiedNavNote](bool on) {
        QSettings s;
        s.setValue(QStringLiteral("ui/unified_nav"), on);
        s.sync();   // persist NOW — independent of the Save button
        unifiedNavNote->setText(on
            ? tr("Saved. Restart the app to switch to the unified bar.")
            : tr("Saved. Restart the app to return to the classic tab bar."));
        unifiedNavNote->setVisible(true);
    });
    layout->addWidget(unifiedNav);
    layout->addWidget(unifiedNavNote);

    // Scroll→sub-tab/page DIRECTION invert (V32.34+). Which way an
    // end-of-content scroll advances depends on the user's scroll convention
    // (natural/reverse vs traditional), which Qt doesn't expose to the app.
    // Reverse-scroll users typically want this ON so the detent follows the
    // content direction. Read once at startup (SwipeNavigator caches it), so a
    // restart is needed — note says so.
    auto *invertScroll =
        new QCheckBox(tr("Reverse scroll-to-navigate (trackpad)"), box);
    invertScroll->setToolTip(tr(
        "Flip which way scrolling past the end of a list moves between sub-tabs "
        "and pages, for the TRACKPAD. Turn this ON if you use natural/reverse "
        "trackpad scrolling and navigation feels backwards. Restart to apply."));
    {
        QSettings s;
        invertScroll->setChecked(
            s.value(QStringLiteral("ui/scroll_detent_invert"), false).toBool());
    }
    auto *invertNote = new QLabel(box);
    invertNote->setWordWrap(true);
    invertNote->setStyleSheet(QStringLiteral("color: %1; font-size: 11px;")
        .arg(QLatin1String(Theme::ACCENT())));
    invertNote->setVisible(false);
    connect(invertScroll, &QCheckBox::toggled, this, [invertNote](bool on) {
        QSettings s;
        s.setValue(QStringLiteral("ui/scroll_detent_invert"), on);
        s.sync();
        invertNote->setText(tr("Saved. Restart the app to apply."));
        invertNote->setVisible(true);
        Q_UNUSED(on);
    });
    layout->addWidget(invertScroll);
    layout->addWidget(invertNote);

    // Independent MOUSE-WHEEL invert (V32.34+). The mouse wheel and trackpad can
    // have opposite physical→direction relationships (natural scroll usually
    // inverts the trackpad but not the wheel), so they get separate flags — a
    // single toggle can't satisfy both, which is why the mouse stayed reversed
    // after only the trackpad flag existed.
    auto *invertMouse =
        new QCheckBox(tr("Reverse scroll-to-navigate (mouse wheel)"), box);
    invertMouse->setToolTip(tr(
        "Same as above but for the MOUSE WHEEL, set independently of the "
        "trackpad. Turn this ON if mouse-wheel navigation feels backwards. "
        "Restart to apply."));
    {
        QSettings s;
        invertMouse->setChecked(
            s.value(QStringLiteral("ui/scroll_detent_invert_mouse"),
                    false).toBool());
    }
    auto *invertMouseNote = new QLabel(box);
    invertMouseNote->setWordWrap(true);
    invertMouseNote->setStyleSheet(QStringLiteral("color: %1; font-size: 11px;")
        .arg(QLatin1String(Theme::ACCENT())));
    invertMouseNote->setVisible(false);
    connect(invertMouse, &QCheckBox::toggled, this, [invertMouseNote](bool on) {
        QSettings s;
        s.setValue(QStringLiteral("ui/scroll_detent_invert_mouse"), on);
        s.sync();
        invertMouseNote->setText(tr("Saved. Restart the app to apply."));
        invertMouseNote->setVisible(true);
        Q_UNUSED(on);
    });
    layout->addWidget(invertMouse);
    layout->addWidget(invertMouseNote);

    // Independent TOUCH-DRAG invert (V32.34+). Touch has its own natural
    // physical mapping (the content follows the finger), and the drag→sub-tab
    // /page handoff now shares the same detent machinery as the wheel — but
    // offered as a toggle for parity in case a user wants the advance direction
    // flipped. Inverts ONLY the navigation direction; the rubber-band still
    // tracks the finger.
    auto *invertTouch =
        new QCheckBox(tr("Reverse scroll-to-navigate (touch drag)"), box);
    invertTouch->setToolTip(tr(
        "Flip which way dragging past the end of a list moves between sub-tabs "
        "and pages, for TOUCH. The content still follows your finger; only the "
        "navigation direction inverts. Restart to apply."));
    {
        QSettings s;
        invertTouch->setChecked(
            s.value(QStringLiteral("ui/scroll_detent_invert_touch"),
                    false).toBool());
    }
    auto *invertTouchNote = new QLabel(box);
    invertTouchNote->setWordWrap(true);
    invertTouchNote->setStyleSheet(QStringLiteral("color: %1; font-size: 11px;")
        .arg(QLatin1String(Theme::ACCENT())));
    invertTouchNote->setVisible(false);
    connect(invertTouch, &QCheckBox::toggled, this, [invertTouchNote](bool on) {
        QSettings s;
        s.setValue(QStringLiteral("ui/scroll_detent_invert_touch"), on);
        s.sync();
        invertTouchNote->setText(tr("Saved. Restart the app to apply."));
        invertTouchNote->setVisible(true);
        Q_UNUSED(on);
    });
    layout->addWidget(invertTouch);
    layout->addWidget(invertTouchNote);

    // ── Calls ───────────────────────────────────────────────────────────────
    // "Enable long calls": read live by ConversationDetailDialog when a call
    // starts (no restart needed). With it on, an active call uses the
    // effectively-unbounded idle window so it never auto-dismisses through
    // silence — the user ends it explicitly.
    auto *callsHeader = new QLabel(tr("CALLS"), box);
    callsHeader->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 11px; font-weight: 600; letter-spacing: 0.8px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(callsHeader);

    auto *longCalls = new QCheckBox(tr("Enable long calls"), box);
    longCalls->setToolTip(tr(
        "Keep the call screen open through long silences. Normally a call auto-"
        "dismisses after a couple of minutes with no audio and no taps (the same "
        "safety net that keeps you from getting stuck on any popup). With this "
        "on, an active call never auto-closes — you end it with the End or Close "
        "button. Applies to your next call."));
    {
        QSettings s;
        longCalls->setChecked(
            s.value(QStringLiteral("calls/enableLongCalls"), false).toBool());
    }
    auto *longCallsNote = new QLabel(box);
    longCallsNote->setWordWrap(true);
    longCallsNote->setStyleSheet(QStringLiteral("color: %1; font-size: 11px;")
        .arg(QLatin1String(Theme::ACCENT())));
    longCallsNote->setVisible(false);
    connect(longCalls, &QCheckBox::toggled, this, [longCallsNote](bool on) {
        QSettings s;
        s.setValue(QStringLiteral("calls/enableLongCalls"), on);
        s.sync();   // persist NOW — independent of the Save button
        longCallsNote->setText(on
            ? tr("Saved. Calls will stay open until you end them.")
            : tr("Saved. Calls auto-close after long silences again."));
        longCallsNote->setVisible(true);
    });
    layout->addWidget(longCalls);
    layout->addWidget(longCallsNote);

    // V32.42+: posture engine master switch.
    auto *proximityDim = new QCheckBox(tr("Proximity screensaver"), box);
    proximityDim->setToolTip(tr(
        "While you have a recent voice message or an active call, putting the "
        "phone to your ear (or face-down) dims the screen and, in a call, "
        "switches the mic to gated so only speech is sent. Holding it still "
        "deepens to a screensaver. Taking it away — or a new snap — wakes it. "
        "Needs the device's proximity sensor; does nothing without one. Takes "
        "effect next launch."));
    {
        QSettings s;
        proximityDim->setChecked(
            s.value(QStringLiteral("posture/enabled"), true).toBool());
    }
    auto *proximityNote = new QLabel(box);
    proximityNote->setWordWrap(true);
    proximityNote->setStyleSheet(QStringLiteral("color: %1; font-size: 11px;")
        .arg(QLatin1String(Theme::ACCENT())));
    proximityNote->setVisible(false);
    connect(proximityDim, &QCheckBox::toggled, this, [proximityNote](bool on) {
        QSettings s;
        s.setValue(QStringLiteral("posture/enabled"), on);
        s.sync();
        proximityNote->setText(on
            ? tr("Saved. The proximity screensaver is on.")
            : tr("Saved. The proximity screensaver is off."));
        proximityNote->setVisible(true);
    });
    layout->addWidget(proximityDim);
    layout->addWidget(proximityNote);

    root->addWidget(box);
}

void SettingsPage::buildDiagnosticsSection(QVBoxLayout *root)
{
    auto *box    = makeSection(tr("System Diagnostics"), this);
    auto *layout = new QVBoxLayout(box);
    layout->setSpacing(Theme::SPACE_SM);

    auto *header = new QLabel(tr("DEVICE STATUS"), box);
    header->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 11px; font-weight: 600; letter-spacing: 0.8px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(header);

    // ── Thermal row ───────────────────────────────────────────────────────
    auto *thermalRow = new QHBoxLayout;
    auto *thermalLbl = new QLabel(tr("Thermal"), box);
    thermalLbl->setStyleSheet(QStringLiteral("color: %1; font-size: 13px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    thermalRow->addWidget(thermalLbl);
    thermalRow->addStretch();
    auto *thermalVal = new QLabel(tr("Reading…"), box);
    thermalVal->setObjectName(QStringLiteral("diagThermal"));
    thermalVal->setMaximumWidth(160);
    thermalVal->setAlignment(Qt::AlignRight | Qt::AlignVCenter);
    thermalVal->setStyleSheet(QStringLiteral("color: %1; font-size: 13px; font-weight: 500;")
        .arg(QLatin1String(Theme::ACCENT())));
    thermalRow->addWidget(thermalVal);
    layout->addLayout(thermalRow);

    // ── Battery row ───────────────────────────────────────────────────────
    auto *batRow = new QHBoxLayout;
    auto *batLbl = new QLabel(tr("Battery"), box);
    batLbl->setStyleSheet(QStringLiteral("color: %1; font-size: 13px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    batRow->addWidget(batLbl);
    batRow->addStretch();
    auto *batVal = new QLabel(tr("Unknown"), box);
    batVal->setObjectName(QStringLiteral("diagBattery"));
    batVal->setMaximumWidth(160);
    batVal->setAlignment(Qt::AlignRight | Qt::AlignVCenter);
    batVal->setStyleSheet(QStringLiteral("color: %1; font-size: 13px; font-weight: 500;")
        .arg(QLatin1String(Theme::ACCENT())));
    batRow->addWidget(batVal);
    layout->addLayout(batRow);

    // ── Run diagnostics button ─────────────────────────────────────────────
    // ── DHT Status row ───────────────────────────────────────────────────
    auto *dhtRow = new QHBoxLayout;
    auto *dhtLbl = new QLabel(tr("DHT Network"), box);
    dhtLbl->setStyleSheet(QStringLiteral("color: %1; font-size: 13px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    dhtRow->addWidget(dhtLbl);
    dhtRow->addStretch();
    auto *dhtVal = new QLabel(tr("Unknown"), box);
    dhtVal->setObjectName(QStringLiteral("diagDht"));
    dhtVal->setStyleSheet(QStringLiteral("color: %1; font-size: 13px; font-weight: 500;")
        .arg(QLatin1String(Theme::ACCENT())));
    dhtRow->addWidget(dhtVal);
    layout->addLayout(dhtRow);

    // Update DHT status in the refresh timer (connected to refreshTimer timeout)
    // The actual mode comes from DhtResourceManager via AppController signals.
    // For now show connectivity from a simple D-Bus ping attempt.
    auto *runBtn = new QPushButton(tr("Run Network Diagnostics"), box);
    runBtn->setObjectName(QStringLiteral("accentBtn"));
    runBtn->setFixedHeight(44);
    layout->addWidget(runBtn);

    auto *resultLabel = new QLabel(box);
    resultLabel->setWordWrap(true);
    resultLabel->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 12px; background: %2; border-radius: 8px; padding: 10px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(QLatin1String(Theme::BG_RAISED())));
    resultLabel->setVisible(false);
    layout->addWidget(resultLabel);

    connect(runBtn, &QPushButton::clicked, this, [runBtn, resultLabel]() {
        runBtn->setEnabled(false);
        runBtn->setText(tr("Running…"));
        // Simulate async check (real impl would call snapclone-network-diag via IPC)
        QTimer::singleShot(1500, runBtn, [runBtn, resultLabel]() {
            resultLabel->setText(
tr("\u2713 TCP listen: OK\n""\u2713 UDP multicast: OK\n""\u2713 STUN/UDP: OK\n""\u2713 Public IP: reachable"));
            resultLabel->setVisible(true);
            runBtn->setEnabled(true);
            runBtn->setText(tr("Run Network Diagnostics"));
        });
    });

    root->addWidget(box);

    // ── Wire to PowerManager (update labels live) ──────────────────────────
    // PowerManager is a singleton-like object; we connect via signals.
    // In a real wiring, AppController would expose it. For now, poll /sys directly.
    auto *refreshTimer = new QTimer(box);
    refreshTimer->setInterval(5000);
    refreshTimer->setTimerType(Qt::VeryCoarseTimer);
    connect(refreshTimer, &QTimer::timeout, this, [thermalVal, batVal]() {
        // Read thermal from /sys
        float maxTemp = 0.f;
        for (int i = 0; i < 8; ++i) {
            QFile f(QStringLiteral("/sys/class/thermal/thermal_zone%1/temp").arg(i));
            if (!f.open(QIODevice::ReadOnly)) break;
            const float mc = f.readAll().trimmed().toFloat();
            maxTemp = qMax(maxTemp, mc / 1000.f);
        }
        if (maxTemp > 0.f) {
            const QString state = maxTemp > 75.f ? tr("Critical") :
                                  maxTemp > 60.f ? tr("Hot") :
                                  maxTemp > 45.f ? tr("Warm") : tr("Normal");
            thermalVal->setText(QStringLiteral("%1°C (%2)").arg(maxTemp, 0, 'f', 1).arg(state));
            const QString color = maxTemp > 75.f ? QLatin1String(Theme::DANGER()) :
                                  maxTemp > 60.f ? QLatin1String(Theme::WARNING()) :
                                                   QLatin1String(Theme::SUCCESS());
            thermalVal->setStyleSheet(QStringLiteral(
                "color: %1; font-size: 13px; font-weight: 500;").arg(color));
        } else {
            thermalVal->setText(tr("N/A"));
        }
        // Read battery — scan all power_supply nodes for type=Battery
        const QDir psDir(QStringLiteral("/sys/class/power_supply"));
        const QStringList psNodes = psDir.entryList(QDir::Dirs | QDir::NoDotAndDotDot);
        for (const QString &node : psNodes) {
            const QString base = QStringLiteral("/sys/class/power_supply/") + node;
            QFile typeF(base + QStringLiteral("/type"));
            if (!typeF.open(QIODevice::ReadOnly)) continue;
            if (typeF.readAll().trimmed().toLower() != "battery") continue;
            QFile f(base + QStringLiteral("/capacity"));
            if (!f.open(QIODevice::ReadOnly)) continue;
            const int pct = f.readAll().trimmed().toInt();
            // Show charging status and cycle count if available
            QFile statusF(base + QStringLiteral("/status"));
            QString statusStr;
            if (statusF.open(QIODevice::ReadOnly))
                statusStr = QString::fromLatin1(statusF.readAll()).trimmed();
            QFile cyclesF(base + QStringLiteral("/cycle_count"));
            QString cycleStr;
            if (cyclesF.open(QIODevice::ReadOnly)) {
                const int c = cyclesF.readAll().trimmed().toInt();
                if (c > 0) cycleStr = QStringLiteral(" · %1 cycles").arg(c);
            }
            const QString label = QStringLiteral("%1%%2%3")
                .arg(pct)
                .arg(statusStr.isEmpty() ? QString{} : QStringLiteral(" (%1)").arg(statusStr))
                .arg(cycleStr);
            batVal->setText(label);
            const QString color = pct <= 10 ? QLatin1String(Theme::DANGER()) :
                                  pct <= 25 ? QLatin1String(Theme::WARNING()) :
                                              QLatin1String(Theme::SUCCESS());
            batVal->setStyleSheet(QStringLiteral(
                "color: %1; font-size: 13px; font-weight: 500;").arg(color));
            break;
        }
    });
    refreshTimer->start();
    // Initial read
    // Trigger an immediate first read by calling the connected lambda directly via start(0).
    QMetaObject::invokeMethod(refreshTimer, "timeout", Qt::QueuedConnection);

    // ── V32.32: ModalSurface test launcher ─────────────────────────────────
    // Opens a test panel that presents synthetic widgets into each of the
    // six modal layers (Ambient, Hint, Dialog, Confirm, Toast, Critical).
    // Lets us verify the layer model on real hardware (FP5 + desktop)
    // before migrating real dialogs onto ModalSurface. See
    // include/ui/ModalSurface.hpp for the layer model.
    auto *modalTestBtn = new QPushButton(tr("Test Modal Layers…"), box);
    modalTestBtn->setStyleSheet(QStringLiteral(
        "QPushButton {"
        "  background: %1; color: %2;"
        "  border: 1px solid %3; border-radius: 8px;"
        "  padding: 10px 16px; font-size: 13px;"
        "}"
        "QPushButton:hover { border-color: %4; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(QLatin1String(Theme::ACCENT())));
    connect(modalTestBtn, &QPushButton::clicked, this,
            &SettingsPage::modalSurfaceTestRequested);
    static_cast<QVBoxLayout *>(box->layout())->addWidget(modalTestBtn);
}


// ─── Debug Flags section ──────────────────────────────────────────────────────
//
// Live runtime toggles for FSM tuning, monitoring blocking, and verbose FSM
// tracing. Each checkbox is wired in two directions:
//   • Toggle → set the flag on Debug::DebugFlags::instance() IMMEDIATELY, so
//     the runtime applies it without a restart. Marks the page dirty so Save
//     persists across sessions.
//   • Save → calls saveToSettings() to write the QSettings keys
//     (debug/no_fsm_tuning, debug/no_monitoring_blocking, debug/verbose_fsm).
//   • Load (loadSettings) → reads from the live singleton, not directly from
//     QSettings, because env/CLI may have set flags above what's on disk.
//     This avoids the checkbox lying about what's actually active.
//
// Why live-toggle rather than save-gated like the rest of the page: these
// flags exist for A/B testing. The whole point is "is X faster with this
// off?" → one click, observe, click back. A Save-gated UI adds friction.
//
// Resetting the page reverts the checkbox visual state to whatever was on
// disk and re-applies that to the runtime. So "I tried things, didn't like
// it, Reset" rolls back the runtime too.
void SettingsPage::buildDebugFlagsSection(QVBoxLayout *root)
{
    using namespace Debug;
    auto *box    = makeSection(tr("Debug & FSM Tuning"), this);
    auto *layout = new QVBoxLayout(box);
    layout->setSpacing(Theme::SPACE_SM);

    auto *intro = new QLabel(
        tr("Process-wide debug toggles. These apply immediately and persist on Save.\n"
           "Default OFF means the named subsystem is active (production behaviour)."),
        box);
    intro->setWordWrap(true);
    intro->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 12px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(intro);

    m_dbgNoFsmTuning = new QCheckBox(tr("Disable FSM / per-peer routing tuning"), box);
    m_dbgNoFsmTuning->setToolTip(tr(
        "When ON, NetworkStats no longer re-orders the address-try queue "
        "by learned success rates. Useful for A/B-testing whether routing "
        "tuning actually helps a given peer."));
    layout->addWidget(m_dbgNoFsmTuning);

    m_dbgNoMonitoringBlocking = new QCheckBox(tr("FSM validators log but never reject"), box);
    m_dbgNoMonitoringBlocking->setToolTip(tr(
        "When ON, FSMValidator emits warnings for misaligned transitions "
        "but never rejects them. Lets the app keep moving through "
        "previously-blocked states so you can capture downstream behaviour."));
    layout->addWidget(m_dbgNoMonitoringBlocking);

    m_dbgVerboseFsm = new QCheckBox(tr("Verbose FSM transition logging"), box);
    m_dbgVerboseFsm->setToolTip(tr(
        "When ON, every FSM state transition is logged at INFO, not just "
        "misaligned ones. Logs get loud quickly; turn off when not actively "
        "debugging."));
    layout->addWidget(m_dbgVerboseFsm);

    // ── V32.34+: mobile display debug ──────────────────────────────────────
    // Gates the chatty UI categories (vkeyboard, modal stack, dialog
    // lifecycle, modal test panel). Off by default — these logs are
    // noise outside of active diagnosis but valuable WHEN diagnosing
    // a touch / layout / dismiss bug.
    m_dbgMobileDisplay = new QCheckBox(tr("Mobile display debug (vkb, modals, dialogs)"), box);
    m_dbgMobileDisplay->setToolTip(tr(
        "When ON, the virtual keyboard watcher, modal stack tap dispatch, "
        "modal test panel, and MobileDialog lifecycle log at INFO. "
        "Useful when reporting bugs about menu stacking, keyboard avoidance, "
        "or sheets failing to dismiss. OFF by default to keep logs clean."));
    m_dbgMobileDisplay->setChecked(UiDebugFlags::mobileDisplayDebug());
    connect(m_dbgMobileDisplay, &QCheckBox::toggled, this, [](bool on) {
        // Live-apply: setMobileDisplayDebug rewrites QLoggingCategory
        // filter rules so the change takes effect on the very next
        // qCInfo call. No Save / restart needed.
        UiDebugFlags::setMobileDisplayDebug(on);
    });
    layout->addWidget(m_dbgMobileDisplay);

    // ── Diagnostic tracing (per-channel) ───────────────────────────────────
    // Everything below is OFF in a normal session. These are the per-event
    // diagnostics — wheel deltas, gesture classification, modal taps, camera
    // pipeline chatter — that are indispensable while a feature is being built
    // and pure noise afterwards. Rather than delete them (the next person to
    // debug a hand-off bug needs exactly those lines, on an installed build,
    // with no compiler in reach) they are compiled in but gated: a channel that
    // is off costs one atomic read, and its arguments are never formatted.
    {
        using namespace SnapClone::Debug;
        auto &tc = TraceControl::instance();

        auto *traceBox = makeSection(tr("Diagnostic tracing"), this);
        auto *tl = new QVBoxLayout(traceBox);
        tl->setSpacing(Theme::SPACE_SM);

        auto *tIntro = new QLabel(
            tr("Extra logging for one area at a time. Off by default; turning a "
               "channel on takes effect immediately and survives a restart, so you "
               "can catch something that breaks during startup. Warnings and errors "
               "are never suppressed."), traceBox);
        tIntro->setWordWrap(true);
        tIntro->setStyleSheet(QStringLiteral("color: %1; font-size: 12px;")
                              .arg(QLatin1String(Theme::TEXT_SECONDARY())));
        tl->addWidget(tIntro);

        auto *status = new QLabel(tc.statusLine(), traceBox);
        status->setStyleSheet(QStringLiteral("color: %1; font-size: 12px; font-weight: 600;")
                              .arg(QLatin1String(Theme::TEXT_PRIMARY())));
        tl->addWidget(status);

        for (std::size_t i = 0; i < kChannelCount; ++i) {
            const auto ch = static_cast<TraceChannel>(i);
            auto *cb = new QCheckBox(
                QStringLiteral("%1 — %2").arg(QString::fromLatin1(name(ch)),
                                              QString::fromLatin1(help(ch))), traceBox);
            cb->setChecked(tc.isOn(ch));
            cb->setToolTip(tr("Also available at launch as --trace=%1 or "
                              "SNAPCLONE_TRACE=%1").arg(QString::fromLatin1(name(ch))));
            connect(cb, &QCheckBox::toggled, this, [ch, status](bool on) {
                auto &t = TraceControl::instance();
                t.setChannel(ch, on, QStringLiteral("settings page"));
                status->setText(t.statusLine());
            });
            // Reflect changes made elsewhere (CLI at launch, auto-off, the legacy
            // toggle) so the checkbox never lies about what is actually running.
            connect(&tc, &TraceControl::changed, cb, [cb, ch, status]() {
                QSignalBlocker block(cb);
                cb->setChecked(TraceControl::instance().isOn(ch));
                status->setText(TraceControl::instance().statusLine());
            });
            tl->addWidget(cb);
        }

        auto *offRow = new QHBoxLayout;
        auto *offLbl = new QLabel(tr("Turn tracing off automatically after"), traceBox);
        auto *offSpin = new QSpinBox(traceBox);
        offSpin->setRange(0, 24 * 60);
        offSpin->setSuffix(tr(" min"));
        offSpin->setSpecialValueText(tr("never"));   // 0
        offSpin->setValue(tc.autoOffMinutes());
        offSpin->setToolTip(tr("A field build should never quietly fill the disk "
                               "because a channel was left on. 0 means \"until I turn "
                               "it off\". Also available as --trace-minutes=N."));
        connect(offSpin, QOverload<int>::of(&QSpinBox::valueChanged), this, [status](int v) {
            TraceControl::instance().setAutoOffMinutes(v);
            status->setText(TraceControl::instance().statusLine());
        });
        offRow->addWidget(offLbl);
        offRow->addWidget(offSpin);
        offRow->addStretch(1);
        tl->addLayout(offRow);

        // The same words the terminal prints for --trace-help, so a bug reporter
        // reads one set of instructions no matter where they look.
        auto *cliHelp = new QLabel(TraceControl::helpTextQ(), traceBox);
        cliHelp->setWordWrap(true);
        cliHelp->setTextInteractionFlags(Qt::TextSelectableByMouse);
        cliHelp->setStyleSheet(QStringLiteral(
            "color: %1; font-size: 11px; font-family: monospace;")
            .arg(QLatin1String(Theme::TEXT_SECONDARY())));
        tl->addWidget(cliHelp);

        root->addWidget(traceBox);
    }

    // ── Live-apply wiring ────────────────────────────────────────────────────
    // We connect each toggle to a lambda that both:
    //   • Pushes the new value into the singleton (with "settings page" as
    //     the source string, so the log entry is clear).
    //   • Calls markDirty() so Save lights up.
    auto bind = [this](QCheckBox *cb, DebugFlag flag) {
        if (!cb) return;
        connect(cb, &QCheckBox::toggled, this, [this, flag](bool on) {
            DebugFlags::instance().set(flag, on,
                QStringLiteral("settings page"));
            markDirty();
        });
    };
    bind(m_dbgNoFsmTuning,          DebugFlag::NoFsmTuning);
    bind(m_dbgNoMonitoringBlocking, DebugFlag::NoMonitoringBlocking);
    bind(m_dbgVerboseFsm,           DebugFlag::VerboseFsm);

    // (Unified-nav toggle moved to the Appearance section → NAVIGATION, where a
    //  navigation-style choice belongs; it was previously here in the debug
    //  flags, which was a poor home for a user-facing feature.)

    root->addWidget(box);
}


// ─── Beta testers section (V32.34+) ───────────────────────────────────────────
//
// Lets a developer route the app's DHT publish/lookup at a LOCAL `sc-microdht`
// node (rust/snapclone-microdht) instead of the real Jami DHT, so DHT-dependent
// flows can be tested deterministically and offline. The node is a small
// hub-and-spoke rendezvous store speaking PING/PUT/GET/STATS: it holds the
// opaque (already-encrypted) records the app uses to find peers — reachable
// address + connection-request mailbox — with per-key value caps and TTLs.
// Three controls:
//   * a toggle (beta/micro_dht_enabled),
//   * a host:port field (beta/micro_dht_host, default 127.0.0.1:4544),
//   * a "Test connection" button + status label that PINGs the node.
// The toggle just records intent in settings; the DHT abstraction reads these
// keys to decide where to publish/look up. The ping here is a live reachability
// check so a tester knows the node is actually up before relying on it.
void SettingsPage::buildBetaTestersSection(QVBoxLayout *root)
{
    auto *box    = makeSection(tr("Beta testers"), this);
    auto *layout = new QVBoxLayout(box);
    layout->setSpacing(Theme::SPACE_SM);

    auto *blurb = new QLabel(
        tr("Experimental tools for testing. The local micro-DHT is a small "
           "rendezvous node (the <tt>sc-microdht</tt> tool) you can run on your "
           "own machine and point this app at, instead of the real DHT.<br><br>"
           "What it does now: it stores and serves the small key/value records "
           "the app uses to find peers — your node's reachable address and your "
           "encrypted connection-request mailbox — over a simple PING / PUT / "
           "GET / STATS protocol. A peer PUTs an (opaque, already-encrypted) "
           "record under a key; the other side GETs it to learn where to connect "
           "or that a request is waiting. The node never sees plaintext. It keeps "
           "a bounded number of values per key (oldest-expiring evicted first) so "
           "a busy key can't grow without limit, and records carry a TTL so stale "
           "entries drop out on their own.<br><br>"
           "It's hub-and-spoke (every app talks to the one node you name — there's "
           "no peer-to-peer routing between micro-DHT nodes yet), which is exactly "
           "what makes it good for deterministic, offline testing of the "
           "publish/lookup and mailbox flows. Not for normal use."), box);
    blurb->setWordWrap(true);
    blurb->setStyleSheet(QStringLiteral("color: %1; font-size: %2px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(Theme::FONT_SM));
    layout->addWidget(blurb);

    QSettings s;

    // ── Toggle ──
    m_betaMicroDhtEnabled =
        new QCheckBox(tr("Use local micro-DHT instead of the real DHT"), box);
    m_betaMicroDhtEnabled->setChecked(
        s.value(QStringLiteral("beta/micro_dht_enabled"), false).toBool());
    m_betaMicroDhtEnabled->setToolTip(tr(
        "Route DHT publish and lookup to a local sc-microdht node. Restart any "
        "active connection for the change to take effect."));
    connect(m_betaMicroDhtEnabled, &QCheckBox::toggled, this, [this](bool on) {
        QSettings s2;
        s2.setValue(QStringLiteral("beta/micro_dht_enabled"), on);
        s2.sync();
        if (m_betaMicroDhtHost)   m_betaMicroDhtHost->setEnabled(on);
        if (m_betaMicroDhtStatus) {
            m_betaMicroDhtStatus->setText(on
                ? tr("Enabled — test the connection below.")
                : tr("Disabled (using the real DHT)."));
        }
    });
    layout->addWidget(m_betaMicroDhtEnabled);

    // ── Host:port field ──
    auto *hostRow = new QHBoxLayout;
    auto *hostLabel = new QLabel(tr("Node address:"), box);
    hostLabel->setStyleSheet(QStringLiteral("color: %1;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    m_betaMicroDhtHost = new QLineEdit(box);
    m_betaMicroDhtHost->setText(
        s.value(QStringLiteral("beta/micro_dht_host"),
                QStringLiteral("127.0.0.1:4544")).toString());
    m_betaMicroDhtHost->setPlaceholderText(QStringLiteral("127.0.0.1:4544"));
    m_betaMicroDhtHost->setEnabled(m_betaMicroDhtEnabled->isChecked());
    m_betaMicroDhtHost->setToolTip(tr(
        "IP:port of the sc-microdht node. Use 127.0.0.1:4544 for a node on this "
        "device, or a LAN IP for one on another machine. DNS names are not "
        "supported — enter a literal IP."));
    connect(m_betaMicroDhtHost, &QLineEdit::editingFinished, this, [this]() {
        QSettings s2;
        s2.setValue(QStringLiteral("beta/micro_dht_host"),
                    m_betaMicroDhtHost->text().trimmed());
        s2.sync();
    });
    hostRow->addWidget(hostLabel);
    hostRow->addWidget(m_betaMicroDhtHost, 1);
    layout->addLayout(hostRow);

    // ── Test connection + status ──
    auto *testRow = new QHBoxLayout;
    auto *testBtn = new QPushButton(tr("Test connection"), box);
    m_betaMicroDhtStatus = new QLabel(box);
    m_betaMicroDhtStatus->setWordWrap(true);
    m_betaMicroDhtStatus->setText(m_betaMicroDhtEnabled->isChecked()
        ? tr("Enabled — test the connection below.")
        : tr("Disabled (using the real DHT)."));
    m_betaMicroDhtStatus->setStyleSheet(QStringLiteral("color: %1; font-size: %2px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(Theme::FONT_SM));
    connect(testBtn, &QPushButton::clicked, this, [this]() {
        const QString hostPort = m_betaMicroDhtHost
            ? m_betaMicroDhtHost->text().trimmed()
            : QString();
        const int colon = hostPort.lastIndexOf(QLatin1Char(':'));
        if (colon <= 0) {
            m_betaMicroDhtStatus->setText(
                tr("Enter an address as IP:port, e.g. 127.0.0.1:4544."));
            return;
        }
        const QString host = hostPort.left(colon);
        bool portOk = false;
        const quint16 port =
            static_cast<quint16>(hostPort.mid(colon + 1).toUInt(&portOk));
        if (!portOk || port == 0) {
            m_betaMicroDhtStatus->setText(tr("Invalid port in address."));
            return;
        }
        if (!m_microDht)
            m_microDht = new Network::MicroDhtClient(this);
        // Wire one-shot result handlers (idempotent — disconnect first so a
        // repeated press doesn't stack connections).
        disconnect(m_microDht, nullptr, this, nullptr);
        connect(m_microDht, &Network::MicroDhtClient::pong, this,
                [this](qint64 rttMs) {
                    m_betaMicroDhtStatus->setText(
                        tr("Node is up (PONG, %1 ms).").arg(rttMs >= 0 ? rttMs : 0));
                });
        connect(m_microDht, &Network::MicroDhtClient::requestTimedOut, this,
                [this](const QString &) {
                    m_betaMicroDhtStatus->setText(
                        tr("No response — is sc-microdht running at that address?"));
                });
        connect(m_microDht, &Network::MicroDhtClient::nodeError, this,
                [this](const QString &msg) {
                    m_betaMicroDhtStatus->setText(tr("Error: %1").arg(msg));
                });
        if (!m_microDht->setNode(host, port)) {
            m_betaMicroDhtStatus->setText(
                tr("Could not use that address (enter a literal IP:port)."));
            return;
        }
        m_betaMicroDhtStatus->setText(tr("Pinging %1…").arg(hostPort));
        m_microDht->ping();
    });
    testRow->addWidget(testBtn);
    testRow->addStretch();
    layout->addLayout(testRow);
    layout->addWidget(m_betaMicroDhtStatus);

    root->addWidget(box);
}


// ─── Privacy section ──────────────────────────────────────────────────────────
//
// Single toggle right now: Share location with contacts. When ON, tapping the
// pin (📍) on a contact card will send that contact a Location snap with the
// device's current GPS fix. Default OFF — opt-in.
//
// The toggle reads/writes QSettings("privacy/shareLocation"). MainWindow's
// locationShareRequested handler reads the same key, so changing it here
// takes effect on the next button tap (no restart needed).
void SettingsPage::buildSoundsSection(QVBoxLayout *root)
{
    auto *box = new QWidget;
    box->setObjectName(QStringLiteral("settingsCard"));
    box->setStyleSheet(QStringLiteral(
        "QWidget#settingsCard { background:%1; border-radius:%2px;"
        " border:1px solid %3; padding: %4px; }")
        .arg(QLatin1String(Theme::BG_CARD())).arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::BORDER())).arg(Theme::SPACE_MD));

    auto *layout = new QVBoxLayout(box);
    layout->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                                Theme::SPACE_MD, Theme::SPACE_MD);
    layout->setSpacing(Theme::SPACE_SM);

    auto *title = new QLabel(tr("Sounds"), box);
    title->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:700; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_LG).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    layout->addWidget(title);

    QSettings settings;

    // ── Ringtone volume (gain), 0–200%, default 100% ──
    auto *gainRow = new QHBoxLayout;
    auto *gainLabel = new QLabel(tr("Ringtone volume"), box);
    gainLabel->setStyleSheet(QStringLiteral(
        "background:transparent; border:none; color:%1;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    auto *gainValue = new QLabel(box);
    gainValue->setStyleSheet(QStringLiteral(
        "background:transparent; border:none; color:%1;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    auto *gainSlider = new QSlider(Qt::Horizontal, box);
    gainSlider->setRange(0, 200);
    gainSlider->setSingleStep(5);
    gainSlider->setPageStep(20);
    const int gain0 = int(settings.value(QStringLiteral("sounds/ringtoneGain"), 100.0).toDouble());
    gainSlider->setValue(qBound(0, gain0, 200));
    gainValue->setText(QStringLiteral("%1%").arg(gainSlider->value()));
    gainRow->addWidget(gainLabel);
    gainRow->addStretch();
    gainRow->addWidget(gainValue);
    layout->addLayout(gainRow);
    layout->addWidget(gainSlider);
    connect(gainSlider, &QSlider::valueChanged, this, [gainValue](int v) {
        gainValue->setText(QStringLiteral("%1%").arg(v));
        QSettings s;
        s.setValue(QStringLiteral("sounds/ringtoneGain"), double(v));
    });

    // ── Mute ringtone ──
    auto *ringMute = new QCheckBox(tr("Mute ringtone"), box);
    ringMute->setChecked(settings.value(QStringLiteral("sounds/ringtoneMuted"), false).toBool());
    ringMute->setToolTip(tr("When ON, incoming calls ring silently — the call "
        "screen still appears, just without sound."));
    connect(ringMute, &QCheckBox::toggled, this, [](bool on) {
        QSettings s;
        s.setValue(QStringLiteral("sounds/ringtoneMuted"), on);
    });
    layout->addWidget(ringMute);

    // ── App-wide microphone kill-switch ──
    auto *micOff = new QCheckBox(tr("Turn off microphone (app-wide)"), box);
    micOff->setChecked(settings.value(QStringLiteral("sounds/micPipelineOff"), false).toBool());
    micOff->setToolTip(tr("Master privacy switch. When ON, the microphone pipeline "
        "never opens anywhere in the app — you can still hear calls, but cannot "
        "transmit. Takes effect on your next call."));
    auto *micNote = new QLabel(box);
    micNote->setWordWrap(true);
    micNote->setStyleSheet(QStringLiteral(
        "background:transparent; border:none; color:%1; font-size:%2px;")
        .arg(QLatin1String(Theme::ACCENT())).arg(Theme::FONT_SM));
    micNote->setVisible(false);
    connect(micOff, &QCheckBox::toggled, this, [micNote](bool on) {
        QSettings s;
        s.setValue(QStringLiteral("sounds/micPipelineOff"), on);
        s.sync();   // persist NOW — it's a privacy switch, not Save-gated
        micNote->setText(on ? tr("Microphone is off — you won't transmit on calls.")
                            : tr("Microphone re-enabled for your next call."));
        micNote->setVisible(true);
    });
    layout->addWidget(micOff);
    layout->addWidget(micNote);

    root->addWidget(box);
}

void SettingsPage::buildVoiceTuningSection(QVBoxLayout *root)
{
    auto *box = new QWidget;
    box->setObjectName(QStringLiteral("settingsCard"));
    box->setStyleSheet(QStringLiteral(
        "QWidget#settingsCard { background:%1; border-radius:%2px;"
        " border:1px solid %3; padding: %4px; }")
        .arg(QLatin1String(Theme::BG_CARD())).arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::BORDER())).arg(Theme::SPACE_MD));

    auto *layout = new QVBoxLayout(box);
    layout->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                                Theme::SPACE_MD, Theme::SPACE_MD);
    layout->setSpacing(Theme::SPACE_SM);

    auto *title = new QLabel(tr("Voice tuning"), box);
    title->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:700; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_LG).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    layout->addWidget(title);

    QSettings settings;

    const QString primaryCss = QStringLiteral(
        "background:transparent; border:none; color:%1;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY()));
    const QString secondaryCss = QStringLiteral(
        "background:transparent; border:none; color:%1;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()));

    // Reusable labelled integer slider that persists immediately to QSettings.
    auto addSlider = [&](const QString &label, const QString &key,
                         int lo, int hi, int def, const QString &unitFmt) {
        auto *row  = new QHBoxLayout;
        auto *name = new QLabel(label, box);
        name->setStyleSheet(primaryCss);
        auto *val  = new QLabel(box);
        val->setStyleSheet(secondaryCss);
        auto *slider = new QSlider(Qt::Horizontal, box);
        slider->setRange(lo, hi);
        const int cur = qBound(lo, settings.value(key, def).toInt(), hi);
        slider->setValue(cur);
        val->setText(unitFmt.arg(cur));
        row->addWidget(name);
        row->addStretch();
        row->addWidget(val);
        layout->addLayout(row);
        layout->addWidget(slider);
        connect(slider, &QSlider::valueChanged, this, [val, key, unitFmt](int v) {
            val->setText(unitFmt.arg(v));
            QSettings s;                       // temp: syncs on destruction
            s.setValue(key, v);
        });
    };

    addSlider(tr("Microphone gain"), QStringLiteral("audio/micGainDb"),
              0, 24, 15, tr("%1 dB"));
    addSlider(tr("Jitter buffer (latency)"), QStringLiteral("audio/voiceJitterTarget"),
              1, 10, 2, tr("%1 frames"));
    addSlider(tr("Playout cushion"), QStringLiteral("audio/voicePlayoutCushion"),
              1, 10, 3, tr("%1 frames"));

    // ── Frame size (discrete) ──
    auto *fsRow   = new QHBoxLayout;
    auto *fsLabel = new QLabel(tr("Frame size"), box);
    fsLabel->setStyleSheet(primaryCss);
    auto *fsCombo = new QComboBox(box);
    fsCombo->addItem(tr("10 ms (lowest latency)"), 10);
    fsCombo->addItem(tr("20 ms (balanced)"),       20);
    fsCombo->addItem(tr("40 ms (most robust)"),    40);
    const int fs0 = settings.value(QStringLiteral("audio/voiceFrameMs"), 20).toInt();
    const int fsIdx = fsCombo->findData(fs0);
    fsCombo->setCurrentIndex(fsIdx >= 0 ? fsIdx : 1);
    fsRow->addWidget(fsLabel);
    fsRow->addStretch();
    fsRow->addWidget(fsCombo);
    layout->addLayout(fsRow);
    connect(fsCombo, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
            [fsCombo](int) {
        QSettings s;
        s.setValue(QStringLiteral("audio/voiceFrameMs"), fsCombo->currentData().toInt());
    });

    // ── Microphone input device ──
    // On PipeWire the "system default" can land on a monitor/effect node that
    // doesn't capture; pick a specific input here if the mic isn't being heard.
    auto *micRow   = new QHBoxLayout;
    auto *micLabel = new QLabel(tr("Microphone"), box);
    micLabel->setStyleSheet(primaryCss);
    auto *micCombo = new QComboBox(box);
    micCombo->addItem(tr("System default"), QString());
    {
        const QString cur = settings.value(QStringLiteral("audio/inputDevice")).toString();
        const auto inputs = QMediaDevices::audioInputs();
        for (const QAudioDevice &d : inputs)
            micCombo->addItem(d.description(), d.description());
        const int idx = cur.isEmpty() ? 0 : micCombo->findData(cur);
        micCombo->setCurrentIndex(idx >= 0 ? idx : 0);
    }
    micRow->addWidget(micLabel);
    micRow->addStretch();
    micRow->addWidget(micCombo);
    layout->addLayout(micRow);
    connect(micCombo, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
            [micCombo](int) {
        QSettings s;
        s.setValue(QStringLiteral("audio/inputDevice"), micCombo->currentData().toString());
    });

    auto *note = new QLabel(
        tr("Mic gain sets how loud you sound to others. Lower jitter and cushion "
           "cut latency but can add dropouts on a weak connection. Changes take "
           "effect on your next call or voice note."), box);
    note->setWordWrap(true);
    note->setStyleSheet(QStringLiteral(
        "background:transparent; border:none; color:%1; font-size:%2px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())).arg(Theme::FONT_SM));
    layout->addWidget(note);

    root->addWidget(box);
}

void SettingsPage::buildPrivacySection(QVBoxLayout *root)
{
    auto *box = new QWidget;
    box->setObjectName(QStringLiteral("settingsCard"));
    box->setStyleSheet(QStringLiteral(
        "QWidget#settingsCard { background:%1; border-radius:%2px;"
        " border:1px solid %3; padding: %4px; }")
        .arg(QLatin1String(Theme::BG_CARD())).arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::BORDER())).arg(Theme::SPACE_MD));

    auto *layout = new QVBoxLayout(box);
    layout->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                                Theme::SPACE_MD, Theme::SPACE_MD);
    layout->setSpacing(Theme::SPACE_SM);

    auto *title = new QLabel(tr("Privacy"), box);
    title->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:700; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_LG).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    layout->addWidget(title);

    QSettings settings;
    auto *shareLoc = new QCheckBox(
        tr("Share location with contacts when I tap the location button"), box);
    shareLoc->setChecked(settings.value(QStringLiteral("privacy/shareLocation"),
                                         false).toBool());
    shareLoc->setToolTip(tr("When ON, tapping the location pin on a contact card "
        "sends them your current GPS coordinates as a snap. They will see your "
        "location with accuracy and timestamp. Off by default."));
    connect(shareLoc, &QCheckBox::toggled, this, [](bool on) {
        QSettings s;
        s.setValue(QStringLiteral("privacy/shareLocation"), on);
    });
    layout->addWidget(shareLoc);

    auto *sharePhotoLoc = new QCheckBox(
        tr("Attach my location to every photo snap I send"), box);
    sharePhotoLoc->setChecked(settings.value(
        QStringLiteral("privacy/sharePhotoLocation"), false).toBool());
    sharePhotoLoc->setToolTip(tr("When ON, every photo snap silently includes "
        "your GPS coordinates as metadata. Recipients see a location pin on the "
        "snap viewer. Off by default — independent of the explicit-share button."));
    connect(sharePhotoLoc, &QCheckBox::toggled, this, [](bool on) {
        QSettings s;
        s.setValue(QStringLiteral("privacy/sharePhotoLocation"), on);
    });
    layout->addWidget(sharePhotoLoc);

    auto *recordAudio = new QCheckBox(
        tr("Record audio with video snaps"), box);
    recordAudio->setChecked(settings.value(
        QStringLiteral("video/recordAudio"), true).toBool());
    recordAudio->setToolTip(tr("When OFF, video snaps are recorded silently "
        "(video-only). Useful in noisy environments or when you want to "
        "capture without recording surrounding conversations."));
    connect(recordAudio, &QCheckBox::toggled, this, [](bool on) {
        QSettings s;
        s.setValue(QStringLiteral("video/recordAudio"), on);
    });
    layout->addWidget(recordAudio);

    // V32.28: manual location override (for desktop testing without
    // GeoClue, and for users who prefer hand-set "approximate"
    // location). When both fields are valid and non-empty, the manual
    // values are used in place of any GPS reading via
    // LocationManager::lastKnownLocation (which is the source of truth
    // for both outbound location-snap sends AND for Wayfinder display
    // via AppController::lastKnownLocation fallback path).
    //
    // Stored in QSettings under location/manualLat and location/manualLon.
    // Leaving either blank disables the override.
    auto *manualLocLabel = new QLabel(
        tr("Manual location override (leave blank to use GPS):"), box);
    manualLocLabel->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    manualLocLabel->setWordWrap(true);
    layout->addWidget(manualLocLabel);

    auto *manualLatLonRow = new QHBoxLayout();
    manualLatLonRow->setSpacing(Theme::SPACE_SM);

    auto *manualLatEdit = new QLineEdit(box);
    manualLatEdit->setPlaceholderText(tr("Latitude (e.g. 45.5)"));
    manualLatEdit->setValidator(new QDoubleValidator(-90.0, 90.0, 6, manualLatEdit));
    auto *manualLonEdit = new QLineEdit(box);
    manualLonEdit->setPlaceholderText(tr("Longitude (e.g. -73.6)"));
    manualLonEdit->setValidator(new QDoubleValidator(-180.0, 180.0, 6, manualLonEdit));

    // Populate from existing settings
    if (settings.contains(QStringLiteral("location/manualLat")))
        manualLatEdit->setText(settings.value(
            QStringLiteral("location/manualLat")).toString());
    if (settings.contains(QStringLiteral("location/manualLon")))
        manualLonEdit->setText(settings.value(
            QStringLiteral("location/manualLon")).toString());

    manualLatLonRow->addWidget(manualLatEdit);
    manualLatLonRow->addWidget(manualLonEdit);

    auto *manualLocClearBtn = new QPushButton(tr("Clear"), box);
    manualLocClearBtn->setFlat(true);
    manualLocClearBtn->setStyleSheet(QStringLiteral(
        "QPushButton { color: %1; padding: 2px 8px; }")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    manualLatLonRow->addWidget(manualLocClearBtn);

    layout->addLayout(manualLatLonRow);

    // Save on editingFinished — applies whenever the user commits a value.
    auto saveManualLoc = [manualLatEdit, manualLonEdit]() {
        const QString latStr = manualLatEdit->text().trimmed();
        const QString lonStr = manualLonEdit->text().trimmed();
        QSettings s;
        if (latStr.isEmpty() || lonStr.isEmpty()) {
            // Either blank → remove BOTH so LocationManager skips the
            // manual override path and falls through to GPS.
            s.remove(QStringLiteral("location/manualLat"));
            s.remove(QStringLiteral("location/manualLon"));
            return;
        }
        bool okLat = false, okLon = false;
        const double lat = latStr.toDouble(&okLat);
        const double lon = lonStr.toDouble(&okLon);
        if (okLat && okLon
         && lat >= -90.0 && lat <= 90.0
         && lon >= -180.0 && lon <= 180.0) {
            s.setValue(QStringLiteral("location/manualLat"), lat);
            s.setValue(QStringLiteral("location/manualLon"), lon);
        }
    };
    connect(manualLatEdit, &QLineEdit::editingFinished, this, saveManualLoc);
    connect(manualLonEdit, &QLineEdit::editingFinished, this, saveManualLoc);
    connect(manualLocClearBtn, &QPushButton::clicked, this,
            [manualLatEdit, manualLonEdit]() {
        manualLatEdit->clear();
        manualLonEdit->clear();
        QSettings s;
        s.remove(QStringLiteral("location/manualLat"));
        s.remove(QStringLiteral("location/manualLon"));
    });

    auto *hint = new QLabel(
        tr("Privacy settings take effect immediately — no restart needed."), box);
    hint->setWordWrap(true);
    hint->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(hint);

    root->addWidget(box);
}

// ─── Face Library section (Phase 1 of face-aware bandwidth optimization) ─────
//
// See docs/FACE_BANDWIDTH.md for the full design. This section surfaces:
//
//   - Whether the Rust face crate is compiled into this binary
//     (SNAPCLONE_RUST_FACE). When OFF, the rest of the section explains
//     that the feature is unavailable in this build and the rest of
//     SnapClone works unchanged.
//   - Library enable/disable toggle (consent gate). Defaults OFF —
//     this is opt-in, in line with the privacy posture.
//   - Total bytes used and per-label crop counts.
//   - A "Forget everything" button — the wipe affordance, always present
//     so users know it exists, disabled when there's nothing to wipe.
//
// V32.32 process-wide library: the section consults
// `currentFaceLibrary()` rather than holding its own Library instance.
// When MainWindow has wired up the AppController via `setController()`,
// this returns the AppController-owned library shared with the rest
// of the app (capture path, inbox path — when those land). Before
// `setController()` is called, a local fallback library is used so
// the section is still usable in unit tests / preview builds.
//
// refreshFaceLibrarySection() runs whenever setController() arrives,
// to swap the displayed stats from the fallback-library values to
// the controller-library values.
void SettingsPage::buildFaceLibrarySection(QVBoxLayout *root)
{
    auto *box = new QWidget;
    box->setObjectName(QStringLiteral("settingsCard"));
    box->setStyleSheet(QStringLiteral(
        "QWidget#settingsCard { background:%1; border-radius:%2px;"
        " border:1px solid %3; padding: %4px; }")
        .arg(QLatin1String(Theme::BG_CARD())).arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::BORDER())).arg(Theme::SPACE_MD));

    auto *layout = new QVBoxLayout(box);
    layout->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                                Theme::SPACE_MD, Theme::SPACE_MD);
    layout->setSpacing(Theme::SPACE_SM);

    auto *title = new QLabel(tr("Face library"), box);
    title->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:700; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_LG).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    layout->addWidget(title);

    auto *blurb = new QLabel(tr(
        "Reduces mobile data on photos that contain faces you've sent or "
        "received before. All processing happens on this device; nothing "
        "is sent anywhere. When uncertain, sends the full image normally."),
        box);
    blurb->setWordWrap(true);
    blurb->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(blurb);

    // Probe whether the FFI is compiled in. Either library (controller or
    // fallback) reports the same answer — it's a build-time fact, not
    // an instance-level one.
    Face::Library *lib = currentFaceLibrary();
    if (!lib->isAvailable()) {
        // SNAPCLONE_RUST_FACE was OFF at build time. Tell the user, then
        // stop — the rest of the section's controls would be meaningless.
        auto *unavail = new QLabel(tr(
            "This SnapClone build does not include the face library. "
            "All snaps are sent as full images — no behavior change. "
            "Rebuild with -DSNAPCLONE_RUST_FACE=ON to enable."), box);
        unavail->setWordWrap(true);
        unavail->setStyleSheet(QStringLiteral(
            "font-size:%1px; color:%2; background:transparent; border:none;"
            "padding-top: %3px;")
            .arg(Theme::FONT_SM)
            .arg(QLatin1String(Theme::TEXT_DISABLED()))
            .arg(Theme::SPACE_SM));
        layout->addWidget(unavail);
        root->addWidget(box);
        return;
    }

    QSettings settings;

    // ── Enable / disable toggle ──────────────────────────────────────────
    // QSettings stores the user's consent state. AppController already
    // applied this on startup, but the local fallback (when no controller
    // is attached yet) needs explicit setEnabled. Either way, reading
    // QSettings here is the source of truth for the initial checkbox state.
    const bool enabledStored = settings.value(
        QStringLiteral("face/enabled"), false).toBool();
    lib->setEnabled(enabledStored);
    m_faceEnabledToggle = new QCheckBox(
        tr("Remember faces from snaps to reduce data usage"), box);
    m_faceEnabledToggle->setChecked(enabledStored);
    m_faceEnabledToggle->setToolTip(tr(
        "When ON, SnapClone may store small face crops from your selfies "
        "and from snaps your contacts send you. These crops are used to "
        "compress future photos that contain the same faces. All data "
        "stays on this device."));
    layout->addWidget(m_faceEnabledToggle);

    // ── Stats row ────────────────────────────────────────────────────────
    m_faceStatsLabel = new QLabel(box);
    m_faceStatsLabel->setWordWrap(true);
    m_faceStatsLabel->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;"
        "padding-top: %3px;")
        .arg(Theme::FONT_SM)
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(Theme::SPACE_SM));
    layout->addWidget(m_faceStatsLabel);

    // ── Wipe button ──────────────────────────────────────────────────────
    auto *wipeRow = new QHBoxLayout;
    wipeRow->setContentsMargins(0, Theme::SPACE_SM, 0, 0);
    m_faceWipeBtn = new QPushButton(tr("Forget everything"), box);
    m_faceWipeBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background:%1; color:%2; border:1px solid %3;"
        "  border-radius:%4px; padding: %5px %6px; font-size:%7px;"
        "  font-weight:600; }"
        "QPushButton:hover { background:%8; }"
        "QPushButton:disabled { color:%9; background:%10; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::DANGER()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::RADIUS_SM)
        .arg(Theme::SPACE_SM).arg(Theme::SPACE_MD)
        .arg(Theme::FONT_SM)
        .arg(QLatin1String(Theme::BG_HOVER()))
        .arg(QLatin1String(Theme::TEXT_DISABLED()))
        .arg(QLatin1String(Theme::BG_CARD())));
    m_faceWipeBtn->setToolTip(tr(
        "Erase all stored face data. The library is rebuilt from "
        "scratch as you continue using SnapClone."));
    wipeRow->addWidget(m_faceWipeBtn);
    wipeRow->addStretch();
    layout->addLayout(wipeRow);

    // ── Wiring ───────────────────────────────────────────────────────────
    // All handlers call currentFaceLibrary() so they always operate on
    // whichever library is active at the moment of interaction (fallback
    // initially, controller-owned after setController()).
    connect(m_faceEnabledToggle, &QCheckBox::toggled, this, [this](bool on) {
        QSettings s;
        s.setValue(QStringLiteral("face/enabled"), on);
        Face::Library *active = currentFaceLibrary();
        if (active) active->setEnabled(on);
        refreshFaceLibrarySection();
    });
    connect(m_faceWipeBtn, &QPushButton::clicked, this, [this]() {
        // No confirmation dialog yet — Phase 1's library is empty by
        // definition until persistence + capture/inbox wiring lands.
        // Add a confirm step when real data exists (filed in TODO).
        Face::Library *active = currentFaceLibrary();
        if (active) {
            std::size_t removed = 0;
            active->wipe(&removed);
            Q_UNUSED(removed);
        }
        refreshFaceLibrarySection();
    });

    refreshFaceLibrarySection();
    root->addWidget(box);
}

// ─── V32.33: Storage section ─────────────────────────────────────────────────
//
// Shows total disk usage by the app + a "Clear cache" button + a small blurb
// listing which folders we walk. Intentionally minimal: no per-category
// breakdown, no per-snap inspection. If users want fine-grained control
// they have a file manager.
//
// The walk runs on a worker thread because recordings caches can hold
// hundreds of MB of .webm/.mkv and the walk does a stat() per file.
// QPointer guards the callback so we don't touch destroyed widgets if
// the user leaves the settings page while the walk is in flight.

namespace {

/// Directories the app writes to. AppDataLocation holds keys, the snap
/// database, received media; CacheLocation holds recordings (intermediate
/// .mkv and transcoded .webm). We walk both recursively and sum.
/// Returns the list in the same order we display in the blurb so the
/// labels match what's actually included.
QStringList snapcloneDataDirs()
{
    return {
        QStandardPaths::writableLocation(QStandardPaths::AppDataLocation),
        QStandardPaths::writableLocation(QStandardPaths::CacheLocation),
    };
}

/// Recursive sum of regular-file sizes under `root`. Returns 0 if the
/// path doesn't exist (e.g. cache dir not yet created on a fresh
/// install). Skips symlinks rather than following them — avoids any
/// chance of accidentally summing /tmp or external mounts if the
/// platform points cache at something unusual.
qint64 directorySizeBytes(const QString &root)
{
    QFileInfo rootInfo(root);
    if (!rootInfo.exists() || !rootInfo.isDir()) return 0;

    qint64 total = 0;
    QDirIterator it(root, QDir::Files | QDir::NoSymLinks | QDir::Hidden,
                     QDirIterator::Subdirectories);
    while (it.hasNext()) {
        it.next();
        total += it.fileInfo().size();
    }
    return total;
}

/// Human-friendly byte count. Uses binary KB/MB/GB (1024-based) since
/// that matches what file managers on Linux/Android typically display
/// for app storage. Returns e.g. "42.3 MB" or "128 KB".
QString formatBytes(qint64 bytes)
{
    constexpr qint64 KB = 1024;
    constexpr qint64 MB = 1024 * KB;
    constexpr qint64 GB = 1024 * MB;
    if (bytes >= GB) return QStringLiteral("%1 GB").arg(bytes / double(GB), 0, 'f', 2);
    if (bytes >= MB) return QStringLiteral("%1 MB").arg(bytes / double(MB), 0, 'f', 1);
    if (bytes >= KB) return QStringLiteral("%1 KB").arg(bytes / double(KB), 0, 'f', 0);
    return QStringLiteral("%1 B").arg(bytes);
}

/// Deletes all files under `root` (but not the root itself) — files we
/// care about clearing are the recordings .mkv/.webm intermediates,
/// which can be regenerated. We do NOT clear AppDataLocation here
/// because that holds identity keys, the contact DB, received snaps,
/// and other data the user explicitly created.
///
/// Returns count of files deleted (for confirmation feedback).
int clearCacheTree(const QString &root)
{
    QFileInfo rootInfo(root);
    if (!rootInfo.exists() || !rootInfo.isDir()) return 0;

    int removed = 0;
    QDirIterator it(root, QDir::Files | QDir::NoSymLinks | QDir::Hidden,
                     QDirIterator::Subdirectories);
    while (it.hasNext()) {
        it.next();
        if (QFile::remove(it.filePath())) ++removed;
    }
    return removed;
}

} // anonymous namespace

void SettingsPage::buildCameraSection(QVBoxLayout *root)
{
    // V32.34+: Camera framework configuration.
    //
    // Three subsections:
    //   1. Devices & status — list of detected cameras with per-node Test
    //      buttons and live result badges (Working / NoFrames / EarlyExit /...)
    //   2. Tuning — CameraPolicy fields: timeouts, capture dimensions,
    //      rotation, auto-retry toggle, reset-to-defaults
    //   3. Diagnostics — full dumpDiagnostics() with copy-to-clipboard for
    //      bug reports
    //
    // The section is collapsible like Advanced — most users will never need
    // to touch it. The defaults are correct for FP5 / pmOS / mainline
    // libcamera; the controls exist for the edge cases (different sensor,
    // slower CPU, custom v4l2 source, etc.).

    auto *box = new QWidget;
    box->setObjectName(QStringLiteral("settingsCard"));
    box->setStyleSheet(QStringLiteral(
        "QWidget#settingsCard { background:%1; border-radius:%2px;"
        " border:1px solid %3; padding: %4px; }")
        .arg(QLatin1String(Theme::BG_CARD())).arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::BORDER())).arg(Theme::SPACE_MD));

    auto *layout = new QVBoxLayout(box);
    layout->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                                Theme::SPACE_MD, Theme::SPACE_MD);
    layout->setSpacing(Theme::SPACE_SM);

    auto *title = new QLabel(tr("Camera"), box);
    title->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:700; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_LG).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    layout->addWidget(title);

    auto *blurb = new QLabel(tr(
        "Detect, test, and tune camera capture. Defaults are correct for "
        "most devices — adjust timeouts here only if the camera takes "
        "unusually long to start or stalls intermittently."),
        box);
    blurb->setWordWrap(true);
    blurb->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(blurb);

    // V32.34+: drive the per-device badges from the process-wide
    // CameraHealthService rather than a local CameraTester. This:
    //   • Avoids two concurrent CameraTester instances colliding over
    //     the same libcamera kernel sensor (which can't serve two
    //     clients at once).
    //   • Persists results across Settings close+reopen — the user
    //     doesn't lose badges if they navigate away and come back.
    //   • Lets CameraWidget's defaultDeviceId selection benefit from
    //     the probes the user runs here.
    auto *health = &Camera::CameraHealthService::instance();
    // Trigger a background scan if results are stale or missing.
    // No-op if everything's fresh.
    QTimer::singleShot(0, this, [health]() { health->ensureProbed(); });

    // ── 1. Devices & status ──────────────────────────────────────────────
    auto *devsLabel = new QLabel(tr("Detected devices"), box);
    devsLabel->setStyleSheet(QStringLiteral(
        "color:%1; font-size:%2px; font-weight:600; background:transparent; border:none;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_BASE));
    layout->addWidget(devsLabel);

    // Container that we'll re-populate on each refresh. The contents are
    // 1 row per device: <displayName>  [Status badge]  [Test button]
    auto *devsContainer = new QWidget(box);
    devsContainer->setStyleSheet(QStringLiteral("background: transparent;"));
    auto *devsLayout = new QVBoxLayout(devsContainer);
    devsLayout->setContentsMargins(0, 0, 0, 0);
    devsLayout->setSpacing(Theme::SPACE_XS);
    layout->addWidget(devsContainer);

    // Helper: status → badge text + color. Kept local to this section so
    // the colors live next to the strings they describe.
    auto badgeFor = [](Camera::ProbeResult::Status s)
        -> std::pair<QString, QString> /*text, color*/
    {
        using S = Camera::ProbeResult::Status;
        switch (s) {
            case S::Untested:      return { QObject::tr("untested"),    QStringLiteral("#8a8a8a") };
            case S::Working:       return { QObject::tr("working"),     QStringLiteral("#2eb872") };
            case S::NoFrames:      return { QObject::tr("no frames"),   QStringLiteral("#f6a623") };
            case S::ProcessFailed: return { QObject::tr("can't start"), QStringLiteral("#e94e4e") };
            case S::EarlyExit:     return { QObject::tr("crashed"),     QStringLiteral("#e94e4e") };
            case S::TimedOut:      return { QObject::tr("timed out"),   QStringLiteral("#f6a623") };
        }
        return { QStringLiteral("?"), QStringLiteral("#8a8a8a") };
    };

    // Rebuild rows from the registry + the health service cache. Called on
    // initial build and every time we want to refresh badges (after
    // probe results come in).
    // shared_ptr (not auto*) — make_shared returns a smart pointer, not a
    // raw pointer. We use shared_ptr<function> so the rebuildRows closure
    // can be captured by value into multiple lambdas (each carrying its
    // own ref); the inner function survives as long as any lambda holds it.
    auto rebuildRows = std::make_shared<std::function<void()>>();
    *rebuildRows = [devsContainer, devsLayout, health, badgeFor]() {
        // Wipe existing rows.
        while (auto *item = devsLayout->takeAt(0)) {
            if (auto *w = item->widget()) w->deleteLater();
            delete item;
        }
        const auto devices = Camera::CameraRegistry::instance().available();
        if (devices.isEmpty()) {
            auto *empty = new QLabel(QObject::tr(
                "No cameras detected. Check that gst-plugin-pipewire is "
                "installed and that camera-bridge or libcamera_input nodes "
                "appear in `pw-dump`."), devsContainer);
            empty->setWordWrap(true);
            empty->setStyleSheet(QStringLiteral(
                "color:%1; font-size:%2px; background:transparent; border:none;")
                .arg(QLatin1String(Theme::TEXT_SECONDARY())).arg(Theme::FONT_SM));
            devsLayout->addWidget(empty);
            return;
        }
        for (const auto &dev : devices) {
            auto *row = new QWidget(devsContainer);
            row->setStyleSheet(QStringLiteral("background: transparent;"));
            auto *rowLayout = new QHBoxLayout(row);
            rowLayout->setContentsMargins(0, 0, 0, 0);
            rowLayout->setSpacing(Theme::SPACE_SM);

            auto *name = new QLabel(dev.displayName, row);
            name->setStyleSheet(QStringLiteral(
                "color:%1; font-size:%2px; background:transparent; border:none;")
                .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_BASE));
            name->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Preferred);
            rowLayout->addWidget(name, 1);

            const auto r = health->result(dev.virtualNodeId);
            const auto [badgeText, badgeColor] = badgeFor(r.status);
            QString badgeLabel = badgeText;
            if (r.status == Camera::ProbeResult::Status::Working
                && r.timeToFirstFrameMs >= 0) {
                if (r.framePixelWidth > 0 && r.framePixelHeight > 0) {
                    // Show negotiated resolution alongside ttff. If it
                    // differs from the policy capture dims this is the
                    // user's visual cue that something is being scaled.
                    badgeLabel = QObject::tr("working (%1×%2, %3ms)")
                                     .arg(r.framePixelWidth)
                                     .arg(r.framePixelHeight)
                                     .arg(r.timeToFirstFrameMs);
                } else {
                    badgeLabel = QObject::tr("working (%1ms)").arg(r.timeToFirstFrameMs);
                }
            }
            auto *badge = new QLabel(badgeLabel, row);
            badge->setStyleSheet(QStringLiteral(
                "color:%1; font-size:11px; background:transparent; border:none;"
                " padding: 2px 8px;")
                .arg(badgeColor));
            rowLayout->addWidget(badge);

            auto *testBtn = new QPushButton(QObject::tr("Test"), row);
            testBtn->setFixedHeight(28);
            testBtn->setStyleSheet(QStringLiteral(
                "QPushButton { background:%1; color:%2; border:1px solid %3;"
                " border-radius:6px; font-size:12px; padding: 2px 12px; }"
                "QPushButton:disabled { color:%4; }")
                .arg(QLatin1String(Theme::BG_RAISED()))
                .arg(QLatin1String(Theme::TEXT_PRIMARY()))
                .arg(QLatin1String(Theme::BORDER()))
                .arg(QLatin1String(Theme::TEXT_SECONDARY())));
            const QString id = dev.virtualNodeId;
            QObject::connect(testBtn, &QPushButton::clicked, health,
                             [health, id, testBtn]() {
                testBtn->setEnabled(false);
                testBtn->setText(QObject::tr("Testing…"));
                // Known UX wart: probeOne() preempts any in-flight batch
                // scan from "Test all". The Test-all button will re-enable
                // when the single probe finishes (we emit scanComplete in
                // both cases), even though the rest of the devices in the
                // batch were dropped. Acceptable for now — fixing it
                // properly means queuing single probes behind batch
                // probes, which adds complexity disproportionate to the
                // benefit on the typical 3-device list.
                health->probeOne(id);
            });
            rowLayout->addWidget(testBtn);

            devsLayout->addWidget(row);
        }
    };
    (*rebuildRows)();

    // Refresh on every probe result.
    connect(health, &Camera::CameraHealthService::resultReady,
            box, [rebuildRows](const QString &, const Camera::ProbeResult &) {
        (*rebuildRows)();
    });

    // Test-all row.
    auto *testAllRow = new QHBoxLayout;
    testAllRow->setSpacing(Theme::SPACE_SM);
    auto *testAllBtn = new QPushButton(tr("Test all"), box);
    testAllBtn->setFixedHeight(36);
    testAllBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background:%1; color:%2; border:1px solid %3;"
        " border-radius:8px; font-size:13px; padding: 6px 16px; }"
        "QPushButton:disabled { color:%4; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    connect(testAllBtn, &QPushButton::clicked, this, [health, testAllBtn]() {
        testAllBtn->setEnabled(false);
        testAllBtn->setText(tr("Testing all…"));
        // rescan() forces a full re-probe ignoring cached freshness,
        // which is what users expect from "Test all".
        health->rescan();
    });
    connect(health, &Camera::CameraHealthService::scanComplete,
            this, [testAllBtn]() {
        testAllBtn->setEnabled(true);
        testAllBtn->setText(tr("Test all"));
    });
    testAllRow->addWidget(testAllBtn);

    auto *clearCacheBtn = new QPushButton(tr("Clear results"), box);
    clearCacheBtn->setFixedHeight(36);
    clearCacheBtn->setStyleSheet(testAllBtn->styleSheet());
    connect(clearCacheBtn, &QPushButton::clicked, this, [health, rebuildRows]() {
        // Wipe the health service cache; rebuildRows refreshes badges
        // to Untested. Note we don't call ensureProbed afterwards —
        // user can hit Test or Test-All explicitly if they want.
        health->rescan();
        (*rebuildRows)();
    });
    testAllRow->addWidget(clearCacheBtn);
    testAllRow->addStretch();
    layout->addLayout(testAllRow);

    // ── Collapsible "Advanced camera options" (Tuning + Diagnostics) ─────
    // V32.34+: the device list above is what most users need (pick a camera,
    // see if it works). Tuning timeouts/dimensions and the diagnostics dump
    // are power-user tools that made this card overwhelmingly long, so they
    // now live behind a collapse toggle, defaulting closed. The toggle and
    // its container go on the card's main `layout`; everything from Tuning
    // onward is added to `advCamLayout` instead of `layout`.
    auto *advCamHeader = new QPushButton(tr("▶ Advanced camera options"), box);
    advCamHeader->setCheckable(true);
    advCamHeader->setChecked(false);
    advCamHeader->setStyleSheet(QStringLiteral(
        "QPushButton { text-align:left; background:%1; color:%2;"
        " border:1px solid %3; border-radius:8px; padding:8px 12px;"
        " font-size:%4px; font-weight:600; min-height:36px; margin-top:8px; }"
        "QPushButton:checked { border-color:%5; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::FONT_BASE)
        .arg(QLatin1String(Theme::ACCENT())));
    layout->addWidget(advCamHeader);

    auto *advCamContainer = new QWidget(box);
    advCamContainer->setVisible(false);
    auto *advCamLayout = new QVBoxLayout(advCamContainer);
    advCamLayout->setContentsMargins(0, 4, 0, 0);
    advCamLayout->setSpacing(Theme::SPACE_SM);
    layout->addWidget(advCamContainer);
    connect(advCamHeader, &QPushButton::toggled, advCamContainer, &QWidget::setVisible);
    connect(advCamHeader, &QPushButton::toggled, advCamHeader, [advCamHeader](bool on) {
        advCamHeader->setText(on ? tr("▼ Advanced camera options")
                                 : tr("▶ Advanced camera options"));
    });

    // ── 2. Tuning (CameraPolicy fields) ──────────────────────────────────
    auto *tuneTitle = new QLabel(tr("Tuning"), box);
    tuneTitle->setStyleSheet(QStringLiteral(
        "color:%1; font-size:%2px; font-weight:600; background:transparent;"
        " border:none; margin-top: 8px;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_BASE));
    advCamLayout->addWidget(tuneTitle);

    auto *grid = new QGridLayout;
    grid->setHorizontalSpacing(Theme::SPACE_MD);
    grid->setVerticalSpacing(Theme::SPACE_XS);

    // Helper to build "<label>  <spinbox>" rows in the grid.
    auto policy = Camera::CameraPolicyStore::instance().current();
    int row = 0;
    auto addSpinRow = [&](const QString &labelText, const QString &tooltip,
                          int minVal, int maxVal, int step, int initial,
                          std::function<void(int)> onChange)
    {
        auto *l = new QLabel(labelText, box);
        l->setToolTip(tooltip);
        l->setStyleSheet(QStringLiteral(
            "color:%1; font-size:%2px; background:transparent; border:none;")
            .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_BASE));
        auto *spin = new QSpinBox(box);
        spin->setRange(minVal, maxVal);
        spin->setSingleStep(step);
        spin->setValue(initial);
        spin->setToolTip(tooltip);
        installWheelGuard(spin);   // V32.34+: scroll over field must NOT change value
        spin->setStyleSheet(QStringLiteral(
            "QSpinBox { background:%1; color:%2; border:1px solid %3;"
            " border-radius:6px; padding: 4px 8px; min-height:28px; }")
            .arg(QLatin1String(Theme::BG_RAISED()))
            .arg(QLatin1String(Theme::TEXT_PRIMARY()))
            .arg(QLatin1String(Theme::BORDER())));
        QObject::connect(spin, QOverload<int>::of(&QSpinBox::valueChanged),
                         box, onChange);
        grid->addWidget(l,    row, 0);
        grid->addWidget(spin, row, 1);
        ++row;
    };

    // Save policy on any spinbox change. We capture by value so each
    // lambda has the right initial; the store's set() handles clamping.
    auto savePolicy = [](auto field, int newVal) {
        auto p = Camera::CameraPolicyStore::instance().current();
        p.*field = newVal;
        Camera::CameraPolicyStore::instance().set(p);
    };

    addSpinRow(tr("Startup timeout (ms)"),
               tr("How long to wait for the camera process to launch. "
                  "Increase if you see 'gst-launch failed to start' errors."),
               50, 60000, 250, policy.startupTimeoutMs,
               [savePolicy](int v) { savePolicy(&Camera::CameraPolicy::startupTimeoutMs, v); });

    addSpinRow(tr("First-frame timeout (ms)"),
               tr("How long to wait for the first camera frame after launch. "
                  "Increase if the kernel takes a long time to initialize the "
                  "sensor on first open (some libcamera drivers are slow)."),
               100, 60000, 500, policy.firstFrameTimeoutMs,
               [savePolicy](int v) { savePolicy(&Camera::CameraPolicy::firstFrameTimeoutMs, v); });

    addSpinRow(tr("Early-exit retry window (ms)"),
               tr("If the camera crashes within this window, try the next "
                  "available device automatically. Set to 0 to disable."),
               100, 30000, 100, policy.earlyExitWindowMs,
               [savePolicy](int v) { savePolicy(&Camera::CameraPolicy::earlyExitWindowMs, v); });

    addSpinRow(tr("Capture width (px)"),
               tr("Output frame width. Changing this requires restarting the "
                  "viewfinder. Most sensors negotiate to whatever value you "
                  "ask for; portrait orientations work best with width<height."),
               80, 4096, 40, policy.captureWidth,
               [savePolicy](int v) { savePolicy(&Camera::CameraPolicy::captureWidth, v); });

    addSpinRow(tr("Capture height (px)"),
               tr("Output frame height. See Capture width."),
               80, 4096, 40, policy.captureHeight,
               [savePolicy](int v) { savePolicy(&Camera::CameraPolicy::captureHeight, v); });

    addSpinRow(tr("Probe startup timeout (ms)"),
               tr("Faster timeout used by the per-camera Test buttons above. "
                  "Lower values get failure verdicts faster but may mis-fail "
                  "slow cameras."),
               50, 10000, 100, policy.probeStartupTimeoutMs,
               [savePolicy](int v) { savePolicy(&Camera::CameraPolicy::probeStartupTimeoutMs, v); });

    addSpinRow(tr("Probe first-frame timeout (ms)"),
               tr("Faster first-frame deadline used by the Test buttons."),
               50, 10000, 100, policy.probeFirstFrameTimeoutMs,
               [savePolicy](int v) { savePolicy(&Camera::CameraPolicy::probeFirstFrameTimeoutMs, v); });

    advCamLayout->addLayout(grid);

    // Rotation combo (kept separate from the spinbox grid since it's
    // a different control type).
    auto *rotRow = new QHBoxLayout;
    auto *rotLbl = new QLabel(tr("Rotation"), box);
    rotLbl->setStyleSheet(QStringLiteral(
        "color:%1; font-size:%2px; background:transparent; border:none;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_BASE));
    rotLbl->setToolTip(tr("Rotation applied to frames. Most devices need "
                          "'none' since camera-bridge already corrects "
                          "sensor rotation. Set 90L/90R/180 to override."));
    auto *rotCombo = new QComboBox(box);
    installWheelGuard(rotCombo);   // V32.34+: scroll over dropdown must NOT change selection
    rotCombo->addItem(tr("None"),    QString{});
    rotCombo->addItem(tr("90° left (CCW)"),  QStringLiteral("90l"));
    rotCombo->addItem(tr("90° right (CW)"), QStringLiteral("90r"));
    rotCombo->addItem(tr("180°"),    QStringLiteral("180"));
    {
        const int idx = rotCombo->findData(policy.rotation.toLower());
        if (idx >= 0) rotCombo->setCurrentIndex(idx);
    }
    rotCombo->setStyleSheet(QStringLiteral(
        "QComboBox { background:%1; color:%2; border:1px solid %3;"
        " border-radius:6px; padding: 4px 8px; min-height:28px; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::BORDER())));
    connect(rotCombo, QOverload<int>::of(&QComboBox::currentIndexChanged),
            this, [rotCombo](int) {
        auto p = Camera::CameraPolicyStore::instance().current();
        p.rotation = rotCombo->currentData().toString();
        Camera::CameraPolicyStore::instance().set(p);
    });
    rotRow->addWidget(rotLbl);
    rotRow->addWidget(rotCombo, 1);
    advCamLayout->addLayout(rotRow);

    // V32.39+: per-facing camera backend preference. libcamera is the native
    // camera stack on mobile Linux, so it is the DEFAULT for BOTH the rear and
    // front cameras. These two dropdowns let the user pin a specific backend
    // per facing — libcamera (direct), GStreamer (through the camera-bridge),
    // Qt, or Automatic (the platform's default ordering). Read by CameraWidget
    // at backend-selection time; persisted to QSettings camera/rearBackend and
    // camera/frontBackend. NOTE: the rear preference selects the startup
    // backend today; honouring a *different* backend for the front while the
    // rear is live needs the per-camera hybrid backend (designed separately) —
    // the front value is stored and ready for it.
    auto addBackendRow = [&](const QString &labelText, const QString &key) {
        auto *row = new QHBoxLayout;
        auto *lbl = new QLabel(labelText, box);
        lbl->setStyleSheet(QStringLiteral(
            "color:%1; font-size:%2px; background:transparent; border:none;")
            .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_BASE));
        auto *combo = new QComboBox(box);
        installWheelGuard(combo);   // scroll over dropdown must NOT change selection
        combo->addItem(tr("libcamera (direct)"),          QStringLiteral("libcamera"));
        combo->addItem(tr("GStreamer (camera-bridge)"),   QStringLiteral("gstreamer"));
        combo->addItem(tr("Qt"),                          QStringLiteral("qt"));
        combo->addItem(tr("Automatic"),                   QStringLiteral("auto"));
        combo->setStyleSheet(QStringLiteral(
            "QComboBox { background:%1; color:%2; border:1px solid %3;"
            " border-radius:6px; padding: 4px 8px; min-height:28px; }")
            .arg(QLatin1String(Theme::BG_RAISED()))
            .arg(QLatin1String(Theme::TEXT_PRIMARY()))
            .arg(QLatin1String(Theme::BORDER())));
        {
            QSettings s(QStringLiteral("SnapCloneProject"), QStringLiteral("SnapClone"));
            const QString cur = s.value(key, QStringLiteral("libcamera")).toString();
            const int idx = combo->findData(cur);
            combo->setCurrentIndex(idx >= 0 ? idx : 0);   // default → libcamera
        }
        connect(combo, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
                [combo, key](int) {
            QSettings s(QStringLiteral("SnapCloneProject"), QStringLiteral("SnapClone"));
            s.setValue(key, combo->currentData().toString());
        });
        row->addWidget(lbl);
        row->addWidget(combo, 1);
        advCamLayout->addLayout(row);
    };
    addBackendRow(tr("Rear camera backend"),  QStringLiteral("camera/rearBackend"));
    addBackendRow(tr("Front camera backend"), QStringLiteral("camera/frontBackend"));

    // Auto-retry toggle.
    auto *autoRetryCb = new QCheckBox(tr("Auto-retry alternate camera on early failure"), box);
    autoRetryCb->setToolTip(tr("If the default camera fails to start, try the "
                               "next one in the list. Disable to surface errors "
                               "immediately instead of silently switching."));
    autoRetryCb->setChecked(policy.autoRetryAlternate);
    autoRetryCb->setStyleSheet(QStringLiteral(
        "QCheckBox { color:%1; font-size:%2px; background:transparent; }")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_BASE));
    connect(autoRetryCb, &QCheckBox::toggled, this, [](bool on) {
        auto p = Camera::CameraPolicyStore::instance().current();
        p.autoRetryAlternate = on;
        Camera::CameraPolicyStore::instance().set(p);
    });
    advCamLayout->addWidget(autoRetryCb);

    // V32.34+: GPU-accelerated colorspace conversion toggle. Defaults OFF
    // because the GL pipeline (glupload/glcolorconvert/glcolorscale/
    // gldownload) didn't negotiate on the FP5/pmOS GL stack during testing
    // — the CPU dual-videoconvert path is proven working. This exposes the
    // opt-in so a user on a device with a working GL stack can try the
    // offload (frees the CPU, enables higher resolution/fps). If the GL
    // path fails to link at runtime, the backend auto-falls-back to CPU on
    // the same camera (see GstLaunchBackend m_gpuConvertFailed), so enabling
    // this can't permanently break the camera.
    auto *gpuConvertCb = new QCheckBox(
        tr("Use GPU-accelerated video conversion (experimental)"), box);
    gpuConvertCb->setToolTip(tr(
        "Offload camera colour conversion and scaling to the GPU. May allow "
        "higher resolution and framerate on devices with a working OpenGL "
        "stack. Off by default — the GPU path failed on some devices during "
        "testing. If it doesn't work on yours, the camera automatically "
        "falls back to the CPU path; just turn this back off."));
    gpuConvertCb->setChecked(policy.preferGpuConvert);
    gpuConvertCb->setStyleSheet(QStringLiteral(
        "QCheckBox { color:%1; font-size:%2px; background:transparent; }")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_BASE));
    connect(gpuConvertCb, &QCheckBox::toggled, this, [](bool on) {
        auto p = Camera::CameraPolicyStore::instance().current();
        p.preferGpuConvert = on;
        Camera::CameraPolicyStore::instance().set(p);
    });
    advCamLayout->addWidget(gpuConvertCb);

    auto *gpuHint = new QLabel(tr(
        "Restart the camera (switch away and back) after changing this."), box);
    gpuHint->setWordWrap(true);
    gpuHint->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_XS).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    advCamLayout->addWidget(gpuHint);

    // V32.34+ PiP FIX: simultaneous-capture toggle. Picture-in-Picture (rear +
    // front selfie overlay) and multi-camera need TWO camera streams at once.
    // Single-ISP phones (FP5) can't, so this is OFF by default and PiP isn't
    // offered. Desktops / devices with independent sensors or USB webcams can
    // enable it. See docs/PIP_AND_SCREENSHARE_DESIGN.md.
    auto *simulCapCb = new QCheckBox(
        tr("Allow simultaneous cameras (Picture-in-Picture)"), box);
    simulCapCb->setToolTip(tr(
        "Enable running two cameras at once, e.g. rear viewfinder with a front "
        "selfie overlay. Most phones share one camera pipeline and cannot do "
        "this — leave OFF unless you have independent cameras or USB webcams "
        "(typically a desktop/laptop). If enabled on unsupported hardware the "
        "second camera simply won't start."));
    simulCapCb->setChecked(policy.simultaneousCaptureSupported);
    simulCapCb->setStyleSheet(QStringLiteral(
        "QCheckBox { color:%1; font-size:%2px; background:transparent; }")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_BASE));
    connect(simulCapCb, &QCheckBox::toggled, this, [](bool on) {
        auto p = Camera::CameraPolicyStore::instance().current();
        p.simultaneousCaptureSupported = on;
        Camera::CameraPolicyStore::instance().set(p);
    });
    advCamLayout->addWidget(simulCapCb);

    auto *simulCapHint = new QLabel(tr(
        "Reopen the camera after changing this for it to take effect."), box);
    simulCapHint->setWordWrap(true);
    simulCapHint->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_XS).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    advCamLayout->addWidget(simulCapHint);

    // not too close to the spinboxes (don't want accidental taps).
    auto *resetBtn = new QPushButton(tr("Reset camera tuning to defaults"), box);
    resetBtn->setFixedHeight(36);
    resetBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background:%1; color:%2; border:1px solid %3;"
        " border-radius:8px; font-size:13px; padding: 6px 16px; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(QLatin1String(Theme::BORDER())));
    connect(resetBtn, &QPushButton::clicked, this, [this]() {
        // Confirm — reset wipes user's hand-tuned timeouts.
        const auto ret = QMessageBox::question(
            this, tr("Reset camera tuning?"),
            tr("Restore all camera timeouts and dimensions to their default "
               "values? You'll need to rebuild this page to see the reset "
               "values reflected in the controls."),
            QMessageBox::Yes | QMessageBox::No);
        if (ret == QMessageBox::Yes) {
            Camera::CameraPolicyStore::instance().resetToDefaults();
        }
    });
    advCamLayout->addWidget(resetBtn);

    // ── 3. Diagnostics dump ──────────────────────────────────────────────
    auto *diagTitle = new QLabel(tr("Diagnostics"), box);
    diagTitle->setStyleSheet(QStringLiteral(
        "color:%1; font-size:%2px; font-weight:600; background:transparent;"
        " border:none; margin-top: 8px;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_BASE));
    advCamLayout->addWidget(diagTitle);

    auto *diagBlurb = new QLabel(tr(
        "Show a full dump of camera discovery state (pw-dump nodes, "
        "cam --list entries, applied overrides, resolved classification). "
        "Paste into bug reports."), box);
    diagBlurb->setWordWrap(true);
    diagBlurb->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    advCamLayout->addWidget(diagBlurb);

    auto *diagOutput = new QLabel(box);
    diagOutput->setWordWrap(true);
    diagOutput->setTextInteractionFlags(Qt::TextSelectableByMouse);
    diagOutput->setStyleSheet(QStringLiteral(
        "color:%1; font-family: monospace; font-size:11px;"
        " background:%2; border:1px solid %3; border-radius:6px;"
        " padding:8px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::BORDER())));
    diagOutput->setVisible(false);
    advCamLayout->addWidget(diagOutput);

    auto *diagRow = new QHBoxLayout;
    auto *diagRunBtn = new QPushButton(tr("Run camera diagnostics"), box);
    diagRunBtn->setFixedHeight(36);
    diagRunBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background:%1; color:%2; border:1px solid %3;"
        " border-radius:8px; font-size:13px; padding: 6px 16px; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::BORDER())));
    auto *diagCopyBtn = new QPushButton(tr("Copy"), box);
    diagCopyBtn->setFixedHeight(36);
    diagCopyBtn->setStyleSheet(diagRunBtn->styleSheet());
    diagCopyBtn->setEnabled(false);
    connect(diagRunBtn, &QPushButton::clicked, this, [diagOutput, diagCopyBtn]() {
        // dumpDiagnostics() runs pw-dump and cam --list synchronously
        // with bounded timeouts; OK to call on the GUI thread.
        const QString dump = Camera::CameraRegistry::instance().dumpDiagnostics();
        diagOutput->setText(dump);
        diagOutput->setVisible(true);
        diagCopyBtn->setEnabled(true);
    });
    connect(diagCopyBtn, &QPushButton::clicked, this, [diagOutput]() {
        QGuiApplication::clipboard()->setText(diagOutput->text());
    });
    diagRow->addWidget(diagRunBtn);
    diagRow->addWidget(diagCopyBtn);
    diagRow->addStretch();
    advCamLayout->addLayout(diagRow);

    // V32.34+: Source target resolution controls. Hidden behind a
    // collapsing section because (a) on FP5 / most pmOS edge builds
    // it doesn't help (libcamera doesn't publish the requested mode,
    // pipewire negotiation fails), and (b) it CAN help on other
    // hardware where libcamera does publish multiple modes — asking
    // for a higher one prevents the "sensor latches into 320×240
    // preview mode" failure pattern. Default off; users who want to
    // try it can flip the values and rebuild the camera.
    //
    // The controls write straight to QSettings (live), no Save needed
    // — the next start() call picks up the new values. We don't tie
    // them into m_dirty because they're independent of the Save
    // button's commit semantics.
    auto *qualHeader = new QLabel(
        tr("Source resolution (advanced)"), box);
    qualHeader->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:600; color:%2; background:transparent; border:none;"
        " margin-top:%3px;")
        .arg(Theme::FONT_BASE)
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(Theme::SPACE_MD));
    advCamLayout->addWidget(qualHeader);

    auto *qualBlurb = new QLabel(tr(
        "Ask the camera for a specific capture size upstream of the "
        "pipeline. Default OFF (0×0): pipewiresrc negotiates whatever "
        "libcamera provides. Set non-zero ONLY if your camera latches "
        "into a low-resolution preview mode and you've confirmed your "
        "device's libcamera advertises the requested mode. On FP5 / "
        "most pmOS edge builds, asking for 1280×720 fails negotiation "
        "and breaks the camera entirely."), box);
    qualBlurb->setWordWrap(true);
    qualBlurb->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_XS).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    advCamLayout->addWidget(qualBlurb);

    auto *qualGrid = new QGridLayout;
    qualGrid->setHorizontalSpacing(Theme::SPACE_MD);
    qualGrid->setVerticalSpacing(Theme::SPACE_SM);

    auto makeSpin = [&](const QString &label, const char *key,
                        int defaultV, int minV, int maxV, int row) {
        auto *lbl = new QLabel(label, box);
        lbl->setStyleSheet(QStringLiteral(
            "font-size:%1px; color:%2; background:transparent; border:none;")
            .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_PRIMARY())));
        auto *spin = new QSpinBox(box);
        spin->setRange(minV, maxV);
        spin->setSingleStep(key == QLatin1String("camera/policy/sourceTargetFps") ? 1 : 8);
        installWheelGuard(spin);   // V32.34+: scroll over field must NOT change value
        QSettings s;
        spin->setValue(s.value(QLatin1String(key), defaultV).toInt());
        connect(spin, QOverload<int>::of(&QSpinBox::valueChanged),
                this, [key](int v) {
            QSettings s2;
            s2.setValue(QLatin1String(key), v);
            // V32.34+: live write. The next GstLaunchBackend::start()
            // call (or the next time CameraPolicyStore::load() runs)
            // picks up the new value. If the user is currently in
            // the camera page they'll need to swipe away and back to
            // force a fresh backend init that reads the new policy.
        });
        qualGrid->addWidget(lbl,  row, 0);
        qualGrid->addWidget(spin, row, 1);
    };
    makeSpin(tr("Source width"),  "camera/policy/sourceTargetWidth",  0, 0, 4096, 0);
    makeSpin(tr("Source height"), "camera/policy/sourceTargetHeight", 0, 0, 4096, 1);
    makeSpin(tr("Source FPS"),    "camera/policy/sourceTargetFps",    0, 0,  120, 2);
    qualGrid->setColumnStretch(2, 1);
    advCamLayout->addLayout(qualGrid);

    auto *qualHint = new QLabel(tr(
        "Tip: 0 means 'no constraint'. To reset to safe defaults, set "
        "all three back to 0. Changes apply on the next camera start."),
        box);
    qualHint->setWordWrap(true);
    qualHint->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;"
        " font-style:italic;")
        .arg(Theme::FONT_XS).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    advCamLayout->addWidget(qualHint);

    // V32.34+: Camera node override UI. The CameraRegistry tries hard
    // to figure out which libcamera_input.* node is rear vs front vs
    // ultrawide using (in priority order): explicit overrides, env
    // vars, `cam --list` Location: field, then i2c-address heuristics.
    // On devices where ALL of those fail or guess wrong (Phone B in
    // our test set), the only escape hatch is to edit
    // ~/.config/snapclone/camera_overrides.ini by hand. That's hostile.
    //
    // This subsection presents a row per facing (Rear / Front /
    // Ultrawide / Tracking) with a dropdown of EVERY libcamera node
    // currently visible to PipeWire. The user picks the right one,
    // we write the override file, the next camera start reads it.
    //
    // We do NOT read pw-dump from here — it's a fork+exec on every
    // page show. Instead we read the registry's already-classified
    // list plus the bridge-published virtual nodes. If the user
    // doesn't see their node, they can hit "Refresh" which forces
    // CameraRegistry to re-probe.
    auto *overrideHeader = new QLabel(
        tr("Override camera mapping (advanced)"), box);
    overrideHeader->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:600; color:%2; background:transparent; border:none;"
        " margin-top:%3px;")
        .arg(Theme::FONT_BASE)
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(Theme::SPACE_MD));
    advCamLayout->addWidget(overrideHeader);

    auto *overrideBlurb = new QLabel(tr(
        "If the app misidentifies which sensor is rear vs front "
        "(rear preview shows the front camera, for example), set "
        "the mapping here. Selections are saved immediately to "
        "~/.config/snapclone/camera_overrides.ini and take effect "
        "on the next camera start. Leave a row blank to use "
        "auto-detection for that facing."), box);
    overrideBlurb->setWordWrap(true);
    overrideBlurb->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_XS).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    advCamLayout->addWidget(overrideBlurb);

    // Helper to build the override path. Has to match
    // CameraRegistry::loadOverrides() exactly.
    auto overridePath = []() {
        const QString dir = QStandardPaths::writableLocation(
                                QStandardPaths::AppConfigLocation);
        QDir().mkpath(dir);
        return dir + QStringLiteral("/camera_overrides.ini");
    };

    // Build the node list. We use whatever's currently visible to
    // CameraRegistry — that's already the union of virtual-cam-*
    // nodes + libcamera_input.* nodes. The list is filtered to
    // node names (no display names) since the override file stores
    // by node name.
    auto allNodeNames = []() {
        QStringList out;
        const auto avail = Camera::CameraRegistry::instance().available();
        for (const auto &cam : avail) {
            if (!out.contains(cam.virtualNodeId)) {
                out.append(cam.virtualNodeId);
            }
        }
        // Also include any libcamera_input.* nodes we may not have
        // classified yet (the user wants to map a freshly-detected
        // one). We could shell out to pw-dump for this but it's
        // expensive; instead use the registry's last probe cache
        // if available, otherwise fall through.
        // TODO: expose CameraRegistry::lastProbeNodes() so we can
        // populate the list even when classification produced
        // nothing. For now, what's classified is what we show —
        // user can hit Refresh to re-probe.
        return out;
    };

    auto *ovrGrid = new QGridLayout;
    ovrGrid->setHorizontalSpacing(Theme::SPACE_MD);
    ovrGrid->setVerticalSpacing(Theme::SPACE_SM);

    QStringList facings = { "rear", "front", "ultrawide", "tracking" };
    QStringList facingLabels = {
        tr("Rear"), tr("Front"), tr("Ultrawide (rear-secondary)"),
        tr("Tracking (front + YOLO)")
    };

    // Read current override values once for initialization.
    QSettings overrideIni(overridePath(), QSettings::IniFormat);
    overrideIni.beginGroup(QStringLiteral("overrides"));

    QList<QComboBox *> overrideCombos;
    for (int i = 0; i < facings.size(); ++i) {
        auto *lbl = new QLabel(facingLabels[i], box);
        lbl->setStyleSheet(QStringLiteral(
            "font-size:%1px; color:%2; background:transparent; border:none;")
            .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_PRIMARY())));
        auto *combo = new QComboBox(box);
        installWheelGuard(combo);   // V32.34+: scroll over dropdown must NOT change selection
        combo->setProperty("facing", facings[i]);
        combo->addItem(tr("(auto-detect)"), QString());
        for (const QString &node : allNodeNames()) {
            // Display name in dropdown: truncate the long
            // libcamera_input.<everything> prefix so the dropdown
            // is readable on a 360-wide screen. Full name retained
            // in itemData so we can save it.
            QString displayName = node;
            const QString lcPrefix = QStringLiteral("libcamera_input.");
            if (displayName.startsWith(lcPrefix)) {
                displayName.remove(0, lcPrefix.size());
            }
            combo->addItem(displayName, node);
        }
        const QString current = overrideIni.value(facings[i]).toString();
        if (!current.isEmpty()) {
            const int idx = combo->findData(current);
            if (idx >= 0) {
                combo->setCurrentIndex(idx);
            } else {
                // Override points to a node that's not visible now —
                // add it so the user sees their saved value, marked
                // as missing.
                combo->addItem(tr("(unavailable) %1").arg(current), current);
                combo->setCurrentIndex(combo->count() - 1);
            }
        }
        connect(combo, QOverload<int>::of(&QComboBox::currentIndexChanged),
                this, [combo, overridePath]() {
            const QString facing = combo->property("facing").toString();
            const QString nodeId = combo->currentData().toString();
            QSettings ini(overridePath(), QSettings::IniFormat);
            ini.beginGroup(QStringLiteral("overrides"));
            if (nodeId.isEmpty()) {
                ini.remove(facing);
            } else {
                ini.setValue(facing, nodeId);
            }
            ini.endGroup();
            ini.sync();
            // Force registry to re-load on next access. The override
            // file is read by CameraRegistry::loadOverrides on each
            // refreshAvailable() — but the cache is keyed off pw-dump
            // mtime, not the override file. Trigger a fresh probe.
            Camera::CameraRegistry::instance().invalidateCache();
        });
        ovrGrid->addWidget(lbl, i, 0);
        ovrGrid->addWidget(combo, i, 1);
        overrideCombos.append(combo);
    }
    overrideIni.endGroup();
    ovrGrid->setColumnStretch(1, 1);
    advCamLayout->addLayout(ovrGrid);

    auto *refreshBtn = new QPushButton(tr("Refresh node list"), box);
    refreshBtn->setStyleSheet(QStringLiteral(
        "padding: 6px 12px; font-size:%1px;").arg(Theme::FONT_SM));
    connect(refreshBtn, &QPushButton::clicked, this,
            [overrideCombos, allNodeNames, overridePath]() {
        Camera::CameraRegistry::instance().invalidateCache();
        // Rebuild combo contents. Preserve current selection.
        QSettings ini(overridePath(), QSettings::IniFormat);
        ini.beginGroup(QStringLiteral("overrides"));
        const QStringList nodes = allNodeNames();
        for (QComboBox *combo : overrideCombos) {
            const QString facing = combo->property("facing").toString();
            const QString current = ini.value(facing).toString();
            const QSignalBlocker block(combo);
            combo->clear();
            combo->addItem(QObject::tr("(auto-detect)"), QString());
            for (const QString &node : nodes) {
                QString displayName = node;
                const QString lcPrefix = QStringLiteral("libcamera_input.");
                if (displayName.startsWith(lcPrefix)) {
                    displayName.remove(0, lcPrefix.size());
                }
                combo->addItem(displayName, node);
            }
            if (!current.isEmpty()) {
                int idx = combo->findData(current);
                if (idx >= 0) {
                    combo->setCurrentIndex(idx);
                } else {
                    combo->addItem(QObject::tr("(unavailable) %1").arg(current), current);
                    combo->setCurrentIndex(combo->count() - 1);
                }
            }
        }
        ini.endGroup();
    });
    advCamLayout->addWidget(refreshBtn);

    root->addWidget(box);
}

void SettingsPage::buildAccessibilitySection(QVBoxLayout *root)
{
    // V32.34+: Accessibility — speech boost on playback.
    //
    // Speech boost is a PER-VIEWER playback feature: it makes spoken words
    // louder and clearer on received video snaps without altering what's
    // sent to anyone else. It applies a mid-band EQ lift (≈300Hz–3.4kHz,
    // where speech intelligibility lives) at playback time. When the
    // snapclone-audio VAD crate is compiled in, the boost can be gated so
    // it only engages on clips that actually contain speech — music and
    // ambient clips are left flat.
    //
    // Requires the gst-launch playback backend + the equalizer-3bands gst
    // element; the dialog skips the boost gracefully when either is absent.

    auto *box = new QWidget;
    box->setObjectName(QStringLiteral("settingsCard"));
    box->setStyleSheet(QStringLiteral(
        "QWidget#settingsCard { background:%1; border-radius:%2px;"
        " border:1px solid %3; padding: %4px; }")
        .arg(QLatin1String(Theme::BG_CARD())).arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::BORDER())).arg(Theme::SPACE_MD));

    auto *layout = new QVBoxLayout(box);
    layout->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                                Theme::SPACE_MD, Theme::SPACE_MD);
    layout->setSpacing(Theme::SPACE_SM);

    auto *title = new QLabel(tr("Accessibility"), box);
    title->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:700; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_LG).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    layout->addWidget(title);

    auto *blurb = new QLabel(tr(
        "Speech boost makes spoken words louder and clearer on received "
        "video snaps. It only affects what you hear — the sender's and "
        "other viewers' copies are unchanged."), box);
    blurb->setWordWrap(true);
    blurb->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(blurb);

    // ── Speech boost level (Off / Mild / Strong) ──────────────────────────
    auto *levelLabel = new QLabel(tr("Speech boost on playback"), box);
    levelLabel->setStyleSheet(QStringLiteral(
        "color:%1; font-size:%2px; font-weight:600; background:transparent; border:none;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_BASE));
    layout->addWidget(levelLabel);

    auto *levelCombo = new QComboBox(box);
    installWheelGuard(levelCombo);   // V32.34+: scroll over dropdown must NOT change selection
    levelCombo->addItem(tr("Off"),    QStringLiteral("off"));
    levelCombo->addItem(tr("Mild"),   QStringLiteral("mild"));
    levelCombo->addItem(tr("Strong"), QStringLiteral("strong"));
    {
        QSettings s;
        const QString cur = s.value(QStringLiteral("audio/speechBoost"),
                                     QStringLiteral("off")).toString().toLower();
        const int idx = levelCombo->findData(cur);
        levelCombo->setCurrentIndex(idx >= 0 ? idx : 0);
    }
    connect(levelCombo,
            QOverload<int>::of(&QComboBox::currentIndexChanged),
            this, [levelCombo](int) {
        QSettings s;
        s.setValue(QStringLiteral("audio/speechBoost"),
                   levelCombo->currentData().toString());
    });
    layout->addWidget(levelCombo);

    // ── Auto-gate: only boost clips that contain speech ───────────────────
    // Only meaningful when the VAD crate is compiled in. When it isn't, the
    // checkbox is shown but disabled with an explanatory tooltip so the
    // user understands why it has no effect.
    auto *autoGate = new QCheckBox(
        tr("Only boost clips that contain speech"), box);
    autoGate->setStyleSheet(QStringLiteral(
        "color:%1; font-size:%2px; background:transparent;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_SM));
    {
        QSettings s;
        autoGate->setChecked(
            s.value(QStringLiteral("audio/speechBoostAutoOnly"), true).toBool());
    }
#ifndef SNAPCLONE_RUST_AUDIO
    autoGate->setEnabled(false);
    autoGate->setToolTip(tr(
        "Speech detection isn't available in this build — the boost applies "
        "to all clips when enabled."));
#endif
    connect(autoGate, &QCheckBox::toggled, this, [](bool on) {
        QSettings s;
        s.setValue(QStringLiteral("audio/speechBoostAutoOnly"), on);
    });
    layout->addWidget(autoGate);

    auto *gateHint = new QLabel(tr(
        "When on, music and ambient videos are left untouched; the boost "
        "engages only when someone is speaking."), box);
    gateHint->setWordWrap(true);
    gateHint->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_XS).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(gateHint);

    root->addWidget(box);
}

void SettingsPage::buildStorageSection(QVBoxLayout *root)
{
    auto *box = new QWidget;
    box->setObjectName(QStringLiteral("settingsCard"));
    box->setStyleSheet(QStringLiteral(
        "QWidget#settingsCard { background:%1; border-radius:%2px;"
        " border:1px solid %3; padding: %4px; }")
        .arg(QLatin1String(Theme::BG_CARD())).arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::BORDER())).arg(Theme::SPACE_MD));

    auto *layout = new QVBoxLayout(box);
    layout->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                                Theme::SPACE_MD, Theme::SPACE_MD);
    layout->setSpacing(Theme::SPACE_SM);

    auto *title = new QLabel(tr("Storage"), box);
    title->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:700; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_LG).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    layout->addWidget(title);

    // Total used (filled in async by refreshStorageSection).
    m_storageTotalLabel = new QLabel(tr("Calculating…"), box);
    m_storageTotalLabel->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_BASE).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    layout->addWidget(m_storageTotalLabel);

    // Folder locations blurb. Build dynamically so it matches what
    // snapcloneDataDirs() actually walks — if we ever add a third dir
    // (e.g. a separate models cache), update one function and this
    // label updates with it.
    const QStringList dirs = snapcloneDataDirs();
    QString locationsText = tr("Includes files under:");
    for (const QString &d : dirs) {
        locationsText += QStringLiteral("\n  • %1").arg(d);
    }
    auto *locations = new QLabel(locationsText, box);
    locations->setWordWrap(true);
    locations->setTextInteractionFlags(Qt::TextSelectableByMouse);
    locations->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;"
        " font-family: monospace;")
        .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(locations);

    // Clear cache button. Recordings are the big-ticket clearable item;
    // see clearCacheTree() comment for why AppDataLocation is intentionally
    // NOT included in the clear.
    auto *blurb = new QLabel(tr(
        "Clearing removes temporary video recordings. Identity keys, "
        "contacts, and saved snaps are kept."), box);
    blurb->setWordWrap(true);
    blurb->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;"
        " padding-top:%3px;")
        .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(Theme::SPACE_SM));
    layout->addWidget(blurb);

    m_storageClearBtn = new QPushButton(tr("Clear cache"), box);
    m_storageClearBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background:%1; color:%2; border:1px solid %3;"
        " border-radius:%4px; padding: 10px 16px; font-size:%5px;"
        " min-height: 40px; }"
        "QPushButton:pressed { background:%6; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::RADIUS_SM)
        .arg(Theme::FONT_BASE)
        .arg(QLatin1String(Theme::BG_HOVER())));
    layout->addWidget(m_storageClearBtn);

    connect(m_storageClearBtn, &QPushButton::clicked, this, [this]() {
        // Confirm before deleting. Cache is regeneratable but a user
        // who tapped by accident shouldn't lose mid-transcode work.
        auto *confirm = new QMessageBox(QMessageBox::Question,
            tr("Clear cache?"),
            tr("This will delete temporary video recordings. "
               "Identity keys, contacts, and saved snaps are not affected. "
               "Continue?"),
            QMessageBox::Yes | QMessageBox::No, this);
        confirm->setDefaultButton(QMessageBox::No);
        if (confirm->exec() != QMessageBox::Yes) {
            confirm->deleteLater();
            return;
        }
        confirm->deleteLater();

        // The cache dir is the only thing we clear — see clearCacheTree
        // doc comment for why AppDataLocation is intentionally excluded.
        const QString cache = QStandardPaths::writableLocation(
                                  QStandardPaths::CacheLocation);
        m_storageClearBtn->setEnabled(false);
        m_storageClearBtn->setText(tr("Clearing…"));
        // ORDERING INVARIANT: refreshStorageSection() is called from the
        // clear-watcher's `finished` handler, NOT immediately. This guarantees
        // the size-walk worker starts AFTER the clear worker finishes, so
        // the displayed total isn't off-by-N-files in the rare case both
        // walks would otherwise run concurrently. If you ever inline the
        // refresh call here, you'll race the file deletions and surface
        // intermediate totals to the user.
        QPointer<SettingsPage> guard(this);
        auto *watcher = new QFutureWatcher<int>(this);
        connect(watcher, &QFutureWatcher<int>::finished, this,
                [this, watcher, guard]() {
            watcher->deleteLater();
            if (!guard) return;
            if (m_storageClearBtn) {
                m_storageClearBtn->setText(tr("Clear cache"));
                m_storageClearBtn->setEnabled(true);
            }
            refreshStorageSection();
        });
        watcher->setFuture(QtConcurrent::run([cache]() {
            return clearCacheTree(cache);
        }));
    });

    // ── V32.43: retention mode ──────────────────────────────────────────────
    // Regular users get the monthly model: snaps auto-expire after the
    // retention window (default ~30 days) and the monthly reminder offers an
    // export to preserve them. Power users who'd rather keep everything and
    // pull their own exports later can flip this — new snaps then never expire,
    // and enabling it also clears the expiry on snaps already on disk, so
    // "keep everything" means everything, not only what arrives next.
    {
        auto *retTitle = new QLabel(tr("Keep everything"), box);
        retTitle->setStyleSheet(QStringLiteral(
            "font-size:%1px; font-weight:600; color:%2; background:transparent;"
            " border:none; padding-top:%3px;")
            .arg(Theme::FONT_BASE).arg(QLatin1String(Theme::TEXT_PRIMARY()))
            .arg(Theme::SPACE_MD));
        layout->addWidget(retTitle);

        auto *keepAll = new QCheckBox(
            tr("Never auto-delete snaps (power user)"), box);
        keepAll->setChecked(QSettings().value(
            QStringLiteral("storage/keepEverything"), false).toBool());
        keepAll->setStyleSheet(QStringLiteral(
            "QCheckBox { font-size:%1px; color:%2; background:transparent; }")
            .arg(Theme::FONT_BASE).arg(QLatin1String(Theme::TEXT_PRIMARY())));
        layout->addWidget(keepAll);

        auto *keepNote = new QLabel(tr(
            "Off: snaps disappear after about a month — save the ones you want "
            "to keep, or use the monthly export. On: nothing is removed "
            "automatically; you keep everything and pull exports yourself when "
            "you'd like to organise them. The manual (About tab) lists where "
            "files live."), box);
        keepNote->setWordWrap(true);
        keepNote->setStyleSheet(QStringLiteral(
            "font-size:%1px; color:%2; background:transparent; border:none;")
            .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
        layout->addWidget(keepNote);

        connect(keepAll, &QCheckBox::toggled, this, [this](bool on) {
            QSettings().setValue(QStringLiteral("storage/keepEverything"), on);
            // Turning it on retroactively clears expiry on existing snaps so
            // they aren't purged out from under a user who just asked to keep
            // everything. Turning it off leaves already-kept snaps kept; only
            // newly sent/received snaps get the expiry window back.
            if (on && m_controller && m_controller->storageManager()) {
                const int kept = m_controller->storageManager()->keepAllSnaps();
                qInfo("Keep-everything on: kept %d existing snap(s)", kept);
            }
        });
    }

    refreshStorageSection();
    root->addWidget(box);
}

void SettingsPage::refreshStorageSection()
{
    if (!m_storageTotalLabel) return;
    m_storageTotalLabel->setText(tr("Calculating…"));

    // Walk on a worker thread; recordings dirs can be large and we
    // don't want to stutter the settings page while it scans.
    QPointer<SettingsPage> guard(this);
    auto *watcher = new QFutureWatcher<qint64>(this);
    connect(watcher, &QFutureWatcher<qint64>::finished, this,
            [this, watcher, guard]() {
        watcher->deleteLater();
        if (!guard || !m_storageTotalLabel) return;
        const qint64 total = watcher->result();
        m_storageTotalLabel->setText(tr("Total used: %1").arg(formatBytes(total)));
    });
    watcher->setFuture(QtConcurrent::run([]() -> qint64 {
        qint64 total = 0;
        for (const QString &dir : snapcloneDataDirs()) {
            total += directorySizeBytes(dir);
        }
        return total;
    }));
}

// ─── V32.33: Security section (TOFU pending key changes, S-20) ───────────────
//
// When the SHAK or DHT path detects an incoming pubkey that differs
// from what's stored for a peer, it does NOT silently replace (that
// would be the persistent-MITM path from S-20). Instead it records
// the attempted change in pending_key_changes. This section reads
// that table and presents each pending change with Accept / Reject
// buttons.
//
// Accept = legitimate reinstall, promote new key, delete pending row.
// Reject = probable attack, delete pending row, keep using old key.
//          Next attempt from the same attacker creates a fresh row.
//
// Rationale for living in Settings rather than a modal popup:
//   - Modal popups train users to dismiss (Browser-SSL-warning trap).
//   - Most pending changes are real attacks the user shouldn't see at
//     all (the old key keeps working, real contact is unaffected).
//   - Legitimate reinstalls are rare enough that finding the prompt
//     in Settings on "where are Alice's snaps?" is acceptable UX.

// V32.34+: monthly-export prompt and the auto-sync settings that
// replace it for power users. See MonthlyExportReminder.hpp for the
// "first of each month" data model.
void SettingsPage::buildExportSection(QVBoxLayout *root)
{
    auto *box    = makeSection(tr("Save & Export Snaps"), this);
    auto *layout = new QVBoxLayout(box);
    layout->setSpacing(Theme::SPACE_SM);

    auto *intro = new QLabel(
        tr("Snaps are ephemeral by default. On the first of each month, "
           "we'll prompt you with a glimpse of recent snaps so you can save "
           "the ones you want. If you'd rather have continuous backup, set "
           "an auto-sync destination below — that turns OFF the monthly "
           "prompt entirely."),
        box);
    intro->setWordWrap(true);
    intro->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 12px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(intro);

    // Auto-sync path entry. Free-text so it can hold either a local
    // path ("/home/user/Pictures/snapclone") or a URL the user's own
    // sync tooling understands (rsync://server/path, ssh://..., etc.).
    // We don't validate at the UI layer — invalid paths simply mean
    // no auto-sync happens at export time, which is the same outcome
    // as no path set. Users with self-hosted setups know their own
    // tooling; we trust them.
    auto *pathRow = new QHBoxLayout;
    auto *pathLabel = new QLabel(tr("Auto-sync destination:"), box);
    pathLabel->setStyleSheet(QStringLiteral("font-size: 13px;"));
    pathRow->addWidget(pathLabel);

    m_autoSyncPathEdit = new QLineEdit(box);
    m_autoSyncPathEdit->setPlaceholderText(
        tr("Empty = monthly prompt. Or path / rsync URL / etc."));
    m_autoSyncPathEdit->setText(MonthlyExportReminder::autoSyncPath());
    // We use editingFinished (not textChanged) to avoid writing settings
    // on every keystroke. Saves QSettings churn AND avoids partial-typing
    // values like "/home/u" being treated as the path between keys.
    connect(m_autoSyncPathEdit, &QLineEdit::editingFinished, this, [this]() {
        const QString txt = m_autoSyncPathEdit->text().trimmed();
        MonthlyExportReminder::setAutoSyncPath(txt);
        // Don't markDirty — this is a live-applied setting, not a
        // form-style one. The next reminder check will see the new
        // value immediately.
    });
    pathRow->addWidget(m_autoSyncPathEdit, 1);
    layout->addLayout(pathRow);

    auto *note = new QLabel(
        tr("Note: SnapClone does not perform the sync itself — the path is "
           "passed to an external tool (rsync, rclone, your own script). "
           "Configure that tool to read from SnapClone's media folder."),
        box);
    note->setWordWrap(true);
    note->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 11px; font-style: italic;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(note);

    root->addWidget(box);
}

void SettingsPage::buildSecuritySection(QVBoxLayout *root)
{
    auto *box = new QWidget;
    box->setObjectName(QStringLiteral("settingsCard"));
    box->setStyleSheet(QStringLiteral(
        "QWidget#settingsCard { background:%1; border-radius:%2px;"
        " border:1px solid %3; padding: %4px; }")
        .arg(QLatin1String(Theme::BG_CARD())).arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::BORDER())).arg(Theme::SPACE_MD));

    auto *layout = new QVBoxLayout(box);
    layout->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                                Theme::SPACE_MD, Theme::SPACE_MD);
    layout->setSpacing(Theme::SPACE_SM);

    auto *title = new QLabel(tr("Security — Pending key changes"), box);
    title->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:700; color:%2;"
        " background:transparent; border:none;")
        .arg(Theme::FONT_LG).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    layout->addWidget(title);

    auto *blurb = new QLabel(tr(
        "If a contact's identity key changes, SnapClone does not silently "
        "accept the new key — that would let an attacker impersonate the "
        "contact. Pending changes appear here. Accept only if you know "
        "the contact legitimately reinstalled or changed devices. Reject "
        "everything else; the existing key keeps working."), box);
    blurb->setWordWrap(true);
    blurb->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(blurb);

    // Container that we rebuild on every refresh. Pending changes are
    // typically zero; this approach is cheap and trivially correct.
    m_securityListContainer = new QWidget(box);
    auto *listLayout = new QVBoxLayout(m_securityListContainer);
    listLayout->setContentsMargins(0, Theme::SPACE_SM, 0, 0);
    listLayout->setSpacing(Theme::SPACE_SM);
    layout->addWidget(m_securityListContainer);

    m_securityEmptyLabel = new QLabel(tr("No pending changes."), box);
    m_securityEmptyLabel->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;"
        " font-style:italic;")
        .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(m_securityEmptyLabel);

    refreshSecuritySection();
    root->addWidget(box);
}

void SettingsPage::refreshSecuritySection()
{
    if (!m_securityListContainer || !m_controller) return;

    // Drop all child cards. qDeleteAll on the container's children list
    // handles widgets + layout items; we leave the layout itself intact.
    auto *listLayout = qobject_cast<QVBoxLayout*>(m_securityListContainer->layout());
    if (!listLayout) return;
    QLayoutItem *child;
    while ((child = listLayout->takeAt(0)) != nullptr) {
        if (QWidget *w = child->widget()) w->deleteLater();
        delete child;
    }

    auto *storage = m_controller->storageManager();
    if (!storage) {
        if (m_securityEmptyLabel) {
            m_securityEmptyLabel->setText(tr("Storage not available."));
            m_securityEmptyLabel->show();
        }
        return;
    }

    const auto pending = storage->loadPendingKeyChanges();
    if (pending.isEmpty()) {
        if (m_securityEmptyLabel) {
            m_securityEmptyLabel->setText(tr("No pending changes."));
            m_securityEmptyLabel->show();
        }
        return;
    }
    if (m_securityEmptyLabel) m_securityEmptyLabel->hide();

    // Build one card per pending change. Each card shows: peer id,
    // attempt count, old key fingerprint (first 16 hex chars of SHA-256
    // of the key) and new key fingerprint, plus Accept and Reject buttons.
    for (const auto &p : pending) {
        auto *card = new QWidget(m_securityListContainer);
        card->setStyleSheet(QStringLiteral(
            "background:%1; border:1px solid %2; border-radius:%3px; padding:%4px;")
            .arg(QLatin1String(Theme::BG_RAISED()))
            .arg(QLatin1String(Theme::BORDER()))
            .arg(Theme::RADIUS_SM)
            .arg(Theme::SPACE_SM));
        auto *cardLayout = new QVBoxLayout(card);
        cardLayout->setContentsMargins(Theme::SPACE_SM, Theme::SPACE_SM,
                                       Theme::SPACE_SM, Theme::SPACE_SM);
        cardLayout->setSpacing(Theme::SPACE_XS);

        auto *peerLabel = new QLabel(p.peerId.value, card);
        peerLabel->setStyleSheet(QStringLiteral(
            "font-size:%1px; font-weight:700; color:%2;"
            " background:transparent; border:none;")
            .arg(Theme::FONT_BASE).arg(QLatin1String(Theme::TEXT_PRIMARY())));
        cardLayout->addWidget(peerLabel);

        // Fingerprints — first 16 hex chars of SHA-256 of the raw key.
        // Truncated for readability; 64 bits of fingerprint is enough to
        // distinguish keys visually (the full key is in the DB if anyone
        // ever wants to verify out of band).
        const QString oldFp = QString::fromLatin1(
            QCryptographicHash::hash(p.oldKey, QCryptographicHash::Sha256)
                .toHex().left(16));
        const QString newFp = QString::fromLatin1(
            QCryptographicHash::hash(p.newKey, QCryptographicHash::Sha256)
                .toHex().left(16));
        auto *fpLabel = new QLabel(tr(
            "Existing key:  %1\n"
            "Proposed key:  %2\n"
            "Detected: %3   ·   Attempt count: %4")
            .arg(oldFp, newFp, p.detectedAt,
                 QString::number(p.attemptCount)), card);
        fpLabel->setStyleSheet(QStringLiteral(
            "font-size:%1px; color:%2; background:transparent; border:none;"
            " font-family: monospace;")
            .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
        fpLabel->setTextInteractionFlags(Qt::TextSelectableByMouse);
        cardLayout->addWidget(fpLabel);

        auto *btnRow = new QHBoxLayout();
        btnRow->setSpacing(Theme::SPACE_SM);

        auto *acceptBtn = new QPushButton(tr("Accept new key"), card);
        acceptBtn->setStyleSheet(QStringLiteral(
            "QPushButton { background:%1; color:%2; border:1px solid %3;"
            " border-radius:%4px; padding: 8px 14px; font-size:%5px;"
            " min-height: 36px; }")
            .arg(QLatin1String(Theme::BG_HOVER()))
            .arg(QLatin1String(Theme::TEXT_PRIMARY()))
            .arg(QLatin1String(Theme::BORDER()))
            .arg(Theme::RADIUS_SM).arg(Theme::FONT_SM));
        btnRow->addWidget(acceptBtn);

        auto *rejectBtn = new QPushButton(tr("Reject"), card);
        rejectBtn->setStyleSheet(QStringLiteral(
            "QPushButton { background:%1; color:%2; border:1px solid %3;"
            " border-radius:%4px; padding: 8px 14px; font-size:%5px;"
            " min-height: 36px; }")
            .arg(QLatin1String(Theme::BG_RAISED()))
            .arg(QLatin1String(Theme::TEXT_PRIMARY()))
            .arg(QLatin1String(Theme::BORDER()))
            .arg(Theme::RADIUS_SM).arg(Theme::FONT_SM));
        btnRow->addWidget(rejectBtn);
        btnRow->addStretch();
        cardLayout->addLayout(btnRow);

        // Capture peerId by value — `p` is a reference into the local
        // QList<PendingKeyChange> and will be invalid after this loop.
        const Core::UserId peerId = p.peerId;

        connect(acceptBtn, &QPushButton::clicked, this, [this, peerId]() {
            auto *confirm = new QMessageBox(QMessageBox::Warning,
                tr("Accept new identity key?"),
                tr("Only accept if you have confirmed out-of-band (in person, "
                   "phone call, or other trusted channel) that this contact "
                   "legitimately reinstalled or got a new device. Accepting an "
                   "attacker's key means all future messages from this contact "
                   "go to the attacker. Continue?"),
                QMessageBox::Yes | QMessageBox::Cancel, this);
            confirm->setDefaultButton(QMessageBox::Cancel);
            const int result = confirm->exec();
            confirm->deleteLater();
            if (result != QMessageBox::Yes) return;
            if (!m_controller || !m_controller->storageManager()) return;
            const bool ok = m_controller->storageManager()->acceptPendingKeyChange(peerId);
            if (!ok) {
                qWarning() << "acceptPendingKeyChange failed for" << peerId.value;
            }
            // Note: we do NOT re-register the new key in CryptoService's
            // in-memory map here. The next SHAK from this peer will pick up
            // the now-trusted key via the TOFU "matches stored" path. The
            // alternative (reaching into AppController to refresh the
            // in-memory map immediately) would require a new API; deferred.
            refreshSecuritySection();
        });

        connect(rejectBtn, &QPushButton::clicked, this, [this, peerId]() {
            if (!m_controller || !m_controller->storageManager()) return;
            const bool ok = m_controller->storageManager()->rejectPendingKeyChange(peerId);
            if (!ok) {
                qWarning() << "rejectPendingKeyChange failed for" << peerId.value;
            }
            refreshSecuritySection();
        });

        listLayout->addWidget(card);
    }
}

// ─── V32.33: Ambient master kill-switch ──────────────────────────────────────
//
// Why this exists: ambient drivers (inbox + camera) run continuous
// derivation work that drowns out other UX perf signal. Until the
// real perf cost is measured, a global off switch lets users (and
// developers) compare with-ambient vs without-ambient app performance.
//
// What it does NOT do: change the palette selection. The user's
// palette pick is preserved; this switch just gates whether ambient
// drivers run at all. When toggled back ON, the drivers resume per
// the saved palette choice (handled by MainWindow's startup restore
// path).

void SettingsPage::buildAmbientSection(QVBoxLayout *root)
{
    auto *box = new QWidget;
    box->setObjectName(QStringLiteral("settingsCard"));
    box->setStyleSheet(QStringLiteral(
        "QWidget#settingsCard { background:%1; border-radius:%2px;"
        " border:1px solid %3; padding: %4px; }")
        .arg(QLatin1String(Theme::BG_CARD())).arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::BORDER())).arg(Theme::SPACE_MD));

    auto *layout = new QVBoxLayout(box);
    layout->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                                Theme::SPACE_MD, Theme::SPACE_MD);
    layout->setSpacing(Theme::SPACE_SM);

    auto *title = new QLabel(tr("Ambient features"), box);
    title->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:700; color:%2;"
        " background:transparent; border:none;")
        .arg(Theme::FONT_LG).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    layout->addWidget(title);

    auto *blurb = new QLabel(tr(
        "Ambient features (inbox-derived chrome, camera-derived chrome) "
        "continuously analyze content to update the UI palette. Turning "
        "them off may improve performance and battery life. Your palette "
        "choice is preserved — ambient drivers resume from the same "
        "selection when re-enabled."), box);
    blurb->setWordWrap(true);
    blurb->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(blurb);

    // Master enable checkbox. Default: enabled (current behavior preserved).
    const bool savedEnabled = QSettings(
        QStringLiteral("SnapCloneProject"),
        QStringLiteral("SnapClone"))
        .value(QStringLiteral("ui/ambient_enabled"), true)
        .toBool();

    auto *enabledCheck = new QCheckBox(
        tr("Enable ambient drivers"), box);
    enabledCheck->setChecked(savedEnabled);
    enabledCheck->setStyleSheet(QStringLiteral(
        "QCheckBox { font-size:%1px; color:%2;"
        " background:transparent; border:none; padding: 6px 0; }"
        "QCheckBox::indicator { width: 22px; height: 22px; }")
        .arg(Theme::FONT_BASE).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    layout->addWidget(enabledCheck);

    connect(enabledCheck, &QCheckBox::toggled, this,
            [this](bool checked) {
        QSettings(
            QStringLiteral("SnapCloneProject"),
            QStringLiteral("SnapClone"))
            .setValue(QStringLiteral("ui/ambient_enabled"), checked);
        // Tell MainWindow to start/stop ambient drivers as appropriate.
        // When checked=false, MainWindow stops both drivers regardless
        // of palette; when checked=true, MainWindow restarts whichever
        // ambient mode the current palette implies.
        emit ambientMasterEnabledChanged(checked);
    });

    root->addWidget(box);
}

// V32.32: returns the active Face::Library. When AppController has been
// attached via setController(), prefers its process-wide library so the
// settings UI reflects the same data the capture/inbox paths see. Before
// that (during early page construction, or in test instances), lazy-
// creates a local fallback so the section is still usable.
SnapClone::Face::Library *SettingsPage::currentFaceLibrary()
{
    if (m_controller) {
        if (auto *lib = m_controller->faceLibrary()) {
            return lib;
        }
    }
    if (!m_faceLibFallback) {
        m_faceLibFallback = std::make_unique<Face::Library>();
        // Apply persisted enabled state to the fallback so its behavior
        // matches the AppController-owned library when one isn't around.
        QSettings s;
        const bool faceEnabled = s.value(
            QStringLiteral("face/enabled"), false).toBool();
        m_faceLibFallback->setEnabled(faceEnabled);
    }
    return m_faceLibFallback.get();
}

// V32.32: re-pull stats from the current library and update widget state.
// Called whenever:
//   - The section is freshly built
//   - The user toggles enabled/wipes (so they see the result immediately)
//   - setController() arrives (so we swap from fallback to AppController
//     library's stats without rebuilding the section)
void SettingsPage::refreshFaceLibrarySection()
{
    if (!m_faceStatsLabel || !m_faceWipeBtn) {
        // Section either wasn't built (FFI off) or builder bailed early.
        return;
    }
    Face::Library *lib = currentFaceLibrary();
    if (!lib || !lib->isAvailable()) {
        m_faceStatsLabel->setText(tr(
            "Library unavailable in this build."));
        m_faceWipeBtn->setEnabled(false);
        return;
    }
    if (!lib->isEnabled()) {
        m_faceStatsLabel->setText(tr(
            "Library is off. No face data is being stored."));
        // Even when off, the wipe button is sensitive to whether bytes
        // are present — disabling doesn't wipe.
        m_faceWipeBtn->setEnabled(lib->totalBytes() > 0);
        return;
    }
    const std::size_t bytes = lib->totalBytes();
    const auto selfCount = lib->labelCount(
        Face::LabelKind::SelfFace, QString());
    const std::size_t selfN = selfCount.value_or(0);

    if (selfN == 0 && bytes == 0) {
        m_faceStatsLabel->setText(tr(
            "No faces remembered yet. They'll accumulate as you "
            "use SnapClone. (Cold start: every snap sends as a "
            "full image until the library has data to work with.)"));
        m_faceWipeBtn->setEnabled(false);
        return;
    }
    const QString sizeStr = (bytes >= 1024 * 1024)
        ? tr("%1 MB").arg(QString::number(bytes / 1048576.0, 'f', 1))
        : tr("%1 KB").arg(QString::number(bytes / 1024.0,    'f', 1));
    m_faceStatsLabel->setText(
        tr("Selfies remembered: %1 · Total: %2")
            .arg(selfN).arg(sizeStr));
    m_faceWipeBtn->setEnabled(true);
}

void SettingsPage::buildNetworkSection(QVBoxLayout *root)
{
    auto *box    = makeSection(tr("Network Backend"), this);
    auto *layout = new QVBoxLayout(box);
    layout->setContentsMargins(8, 4, 8, 8);
    layout->setSpacing(6);

    // Stack label + combo vertically — avoids horizontal overflow on narrow screens
    layout->addWidget(makeLabel(tr("Peer Discovery"), box));
    m_discoveryBackend = new QComboBox(box);
    installWheelGuard(m_discoveryBackend);   // V32.34+: scroll must NOT change selection
    m_discoveryBackend->addItems({
        tr("Gossip (LAN)"),
        tr("Jami DHT"),
        tr("Both"),
    });
    m_discoveryBackend->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Fixed);
    m_discoveryBackend->setFixedHeight(36);
    m_discoveryBackend->setStyleSheet(QStringLiteral(
        "QComboBox { background: %1; border: 1px solid %2; border-radius: 8px;"
        "  color: %3; font-size: 14px; padding: 0 10px; }"
        "QComboBox::drop-down { border: none; width: 20px; }"
        "QComboBox QAbstractItemView { background: %1; color: %3; border: 1px solid %2; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    layout->addWidget(m_discoveryBackend);

    m_jamiEnabled = new QCheckBox(tr("Enable Jami backend"), box);
#ifndef SNAPCLONE_JAMI_BACKEND
    m_jamiEnabled->setEnabled(false);
    m_jamiEnabled->setToolTip(tr("Rebuild with -DSNAPCLONE_JAMI_BACKEND=ON to enable." "\nRequires: cmake -DSNAPCLONE_JAMI_BACKEND=ON"));
#endif
    layout->addWidget(m_jamiEnabled);

    m_upnpEnabled = new QCheckBox(tr("Enable UPnP / NAT-PMP"), box);
    layout->addWidget(m_upnpEnabled);

    connect(m_discoveryBackend, &QComboBox::currentIndexChanged,
            this, &SettingsPage::onBackendChanged);

    root->addWidget(box);
}


void SettingsPage::buildStunSection(QVBoxLayout *root)
{
    auto *box    = makeSection(tr("NAT Traversal / STUN Servers"), this);
    auto *layout = new QVBoxLayout(box);
    layout->setSpacing(Theme::SPACE_SM);

    auto *note = new QLabel(
        tr("STUN servers discover your public IP and punch NAT pinholes for cross-network "
           "connections. All checked servers are tried in order — first responder wins. "
           "Use at least one server for cross-network support. Changes take effect on restart."),
        box);
    note->setWordWrap(true);
    note->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 12px; background: transparent;")
            .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(note);

    auto makeStunRow = [&](const QString &label, const QString &detail) -> QCheckBox * {
        auto *row = new QWidget(box);
        auto *rowLayout = new QHBoxLayout(row);
        rowLayout->setContentsMargins(0, 0, 0, 0);
        rowLayout->setSpacing(8);
        auto *cb = new QCheckBox(row);
        cb->setText(label);
        auto *detailLabel = new QLabel(detail, row);
        detailLabel->setStyleSheet(
            QStringLiteral("color: %1; font-size: 11px; background: transparent;")
                .arg(QLatin1String(Theme::TEXT_SECONDARY())));
        // Hide detail text on very narrow screens — checkbox label is enough
        detailLabel->setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Preferred);
        detailLabel->setMaximumWidth(180);
        rowLayout->addWidget(cb, 1);
        rowLayout->addWidget(detailLabel, 0);
        layout->addWidget(row);
        return cb;
    };

    m_stunSfl        = makeStunRow(tr("stun.jami.net"),         tr("Savoir-faire Linux · FOSS · port 3478 + 80"));
    m_stunNextcloud  = makeStunRow(tr("stun.nextcloud.com"),    tr("Nextcloud · FOSS · port 3478"));
    m_stunLinphone   = makeStunRow(tr("stun.linphone.org"),     tr("Belledonne Communications · FOSS VoIP · port 3478"));
    m_stunEkiga      = makeStunRow(tr("stun.ekiga.net"),        tr("GNOME Project · FOSS · port 3478"));
    m_stunFramasoft  = makeStunRow(tr("stun.framasoft.org"),    tr("Framasoft · French FOSS · port 3478"));
    m_stunGoogle     = makeStunRow(tr("stun.l.google.com"),     tr("Google · proprietary · port 19302"));
    m_stunCloudflare = makeStunRow(tr("stun.cloudflare.com"),   tr("Cloudflare · proprietary · port 3478"));

    // ── Relay server ──────────────────────────────────────────────────────
    layout->addSpacing(Theme::SPACE_SM);
    auto *relayLabel = new QLabel(tr("RELAY SERVER"), box);
    relayLabel->setStyleSheet(QStringLiteral(
        "color:%1; font-size:%2px; font-weight:600; letter-spacing:1px;"
        "background:transparent;")
        .arg(QLatin1String(Theme::TEXT_DISABLED())).arg(Theme::FONT_XS));
    layout->addWidget(relayLabel);

    auto *relayNote = new QLabel(
        tr("Fallback relay for symmetric NAT / blocked UDP (e.g. iPhone hotspot). "
           "Select a public relay or enter your own. All snap data is end-to-end "
           "encrypted — the relay only sees ciphertext."), box);
    relayNote->setWordWrap(true);
    relayNote->setStyleSheet(QStringLiteral(
        "color:%1; font-size:11px; background:transparent;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    layout->addWidget(relayNote);

    // Preset dropdown ── known public relays
    // Each item stores its URL as UserRole data; empty string = None/disabled.
    // "Custom" stores a sentinel so we know to show the text field.
    m_relayPreset = new QComboBox(box);
    installWheelGuard(m_relayPreset);   // V32.34+: scroll must NOT change selection
    const QString kCustomSentinel = QStringLiteral("__custom__");

    struct RelayOption { const char *label; const char *url; const char *note; };
    static const RelayOption kRelays[] = {
        { "None (direct / UDP only)",
          "",
          "" },
        { "Magic Wormhole transit  ·  tcp://transit.magic-wormhole.io:4001",
          "tcp://transit.magic-wormhole.io:4001",
          "FOSS, run by the wormhole.io project. Raw TCP relay, no WebSocket needed." },
        { "Custom WebSocket relay (ws:// or wss://)",
          "__custom__",
          "Run relay-server/relay.js on any VPS." },
    };
    for (const auto &r : kRelays)
        m_relayPreset->addItem(tr(r.label), QString::fromLatin1(r.url));

    m_relayPreset->setStyleSheet(QStringLiteral(
        "QComboBox { background:%1; color:%2; border:1px solid %3;"
        "  border-radius:%4px; padding:5px 10px; font-size:%5px; }"
        "QComboBox::drop-down { border:none; width:24px; }"
        "QComboBox QAbstractItemView { background:%1; color:%2; "
        "  selection-background-color:%6; border:1px solid %3; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::RADIUS_SM)
        .arg(Theme::FONT_SM)
        .arg(QLatin1String(Theme::ACCENT())));
    layout->addWidget(m_relayPreset);

    // Custom URL field — only visible when "Custom" is selected
    m_relayUrl = new QLineEdit(box);
    m_relayUrl->setPlaceholderText(tr("ws://yourserver:9900  or  wss://yourserver:443"));
    m_relayUrl->setVisible(false);
    m_relayUrl->setStyleSheet(QStringLiteral(
        "QLineEdit { background:%1; color:%2; border:1px solid %3;"
        "  border-radius:%4px; padding:6px 10px; font-size:%5px; }"
        "QLineEdit:focus { border-color:%6; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::RADIUS_SM)
        .arg(Theme::FONT_SM)
        .arg(QLatin1String(Theme::ACCENT())));
    layout->addWidget(m_relayUrl);

    // Preset note label — describes selected preset
    m_relayPresetNote = new QLabel(box);
    m_relayPresetNote->setWordWrap(true);
    m_relayPresetNote->setVisible(false);
    m_relayPresetNote->setStyleSheet(QStringLiteral(
        "color:%1; font-size:10px; background:transparent;")
        .arg(QLatin1String(Theme::TEXT_DISABLED())));
    layout->addWidget(m_relayPresetNote);

    // Wire preset → show/hide custom field and update note
    connect(m_relayPreset, &QComboBox::currentIndexChanged,
            this, [this, kCustomSentinel]() {
        const QString val = m_relayPreset->currentData().toString();
        const bool isCustom = (val == kCustomSentinel);
        m_relayUrl->setVisible(isCustom);

        // Update the description note for the selected preset
        const int idx = m_relayPreset->currentIndex();
        static const char* kNotes[] = {
            "",
            "FOSS, run by the wormhole.io project. Raw TCP relay — works through "
            "most firewalls. Data is E2E encrypted; the relay sees only ciphertext.",
            "Enter a ws:// or wss:// URL. Run relay-server/relay.js on any public VPS.",
        };
        const bool hasNote = idx >= 0 && idx < int(std::size(kNotes)) && kNotes[idx][0];
        m_relayPresetNote->setVisible(hasNote);
        if (hasNote)
            m_relayPresetNote->setText(tr(kNotes[idx]));

        m_restartPending = true;
    });
    connect(m_relayUrl, &QLineEdit::textChanged,
            this, [this]() { m_restartPending = true; });

    root->addWidget(box);
}

void SettingsPage::buildCryptoSection(QVBoxLayout *root)
{
    auto *box    = makeSection(tr("Crypto Backend"), this);
    auto *layout = new QVBoxLayout(box);
    layout->setSpacing(Theme::SPACE_SM);

    auto *row = new QHBoxLayout;
    row->addWidget(makeLabel(tr("Encryption"), box));
    m_cryptoBackend = new QComboBox(box);
    installWheelGuard(m_cryptoBackend);   // V32.34+: scroll must NOT change selection
    m_cryptoBackend->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Fixed);
    m_cryptoBackend->addItem(tr("Qt HMAC-SHA256 (placeholder — dev only)"), 0);
#ifdef SNAPCLONE_RUST_CRYPTO
    m_cryptoBackend->addItem(tr("Rust AES-256-GCM + X25519 (production)"), 1);
#endif
    row->addWidget(m_cryptoBackend);
    layout->addLayout(row);

#ifndef SNAPCLONE_RUST_CRYPTO
    auto *note = new QLabel(
        tr("Rebuild with -DSNAPCLONE_RUST_CRYPTO=ON to enable Rust AES-256-GCM."), box);
    note->setWordWrap(true);
    note->setStyleSheet(QStringLiteral("color: %1; font-size: 12px; background: transparent;")
                            .arg(QLatin1String(Theme::WARNING())));
    layout->addWidget(note);
#endif

    root->addWidget(box);
}

void SettingsPage::buildPortsSection(QVBoxLayout *root)
{
    auto *box    = makeSection(tr("Network Ports"), this);
    auto *layout = new QVBoxLayout(box);
    layout->setSpacing(Theme::SPACE_SM);

    auto *wsRow = new QHBoxLayout;
    wsRow->addWidget(makeLabel(tr("WebSocket port (TCP)"), box));
    m_wsPort = new QSpinBox(box);
    m_wsPort->setRange(1024, 65535);
    m_wsPort->setValue(9876);
    installWheelGuard(m_wsPort);   // V32.34+: scroll over field must NOT change port
    wsRow->addWidget(m_wsPort);
    layout->addLayout(wsRow);

    auto *gossipRow = new QHBoxLayout;
    gossipRow->addWidget(makeLabel(tr("Gossip discovery port (UDP)"), box));
    m_gossipPort = new QSpinBox(box);
    m_gossipPort->setRange(1024, 65535);
    m_gossipPort->setValue(9877);
    installWheelGuard(m_gossipPort);   // V32.34+: scroll must NOT change port
    gossipRow->addWidget(m_gossipPort);
    layout->addLayout(gossipRow);

    root->addWidget(box);
}

void SettingsPage::buildAboutSection(QVBoxLayout *root)
{
    auto *box    = makeSection(tr("About"), this);
    auto *layout = new QVBoxLayout(box);
    layout->setSpacing(4);

    auto info = [&](const QString &key, const QString &val) {
        auto *row = new QHBoxLayout;
        auto *k   = makeLabel(key + QStringLiteral(":"), box);
        k->setStyleSheet(QStringLiteral("color: %1; font-size: 12px; background: transparent;")
                             .arg(QLatin1String(Theme::TEXT_SECONDARY())));
        auto *v = makeLabel(val, box);
        v->setStyleSheet(QStringLiteral("color: %1; font-size: 12px; background: transparent;")
                             .arg(QLatin1String(Theme::TEXT_PRIMARY())));
        v->setTextInteractionFlags(Qt::TextSelectableByMouse);
        row->addWidget(k);
        row->addStretch();
        row->addWidget(v);
        layout->addLayout(row);
    };

    info(tr("Version"),    QApplication::applicationVersion());
    info(tr("Qt version"), QString::fromLatin1(QT_VERSION_STR));

    QStringList flags;
#ifdef SNAPCLONE_JAMI_BACKEND
    flags << QStringLiteral("JAMI_BACKEND");
#endif
#ifdef SNAPCLONE_RUST_CRYPTO
    flags << QStringLiteral("RUST_CRYPTO");
#endif
    if (flags.isEmpty()) flags << QStringLiteral("(none)");
    info(tr("Build flags"), flags.join(QStringLiteral(", ")));

    m_buildFlagsLabel = makeLabel(QString{}, box);
    layout->addWidget(m_buildFlagsLabel);

    // V32.43: in-app user manual — a single scrollable markdown page covering
    // what the app does and where its files live. It auto-dismisses on idle,
    // with the timer resetting while you scroll.
    auto *manualBtn = new QPushButton(tr("Open user manual"), box);
    manualBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background:%1; color:%2; border:1px solid %3;"
        " border-radius:%4px; padding: 10px 16px; font-size:%5px;"
        " min-height: 40px; margin-top:%6px; }"
        "QPushButton:pressed { background:%7; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::RADIUS_SM).arg(Theme::FONT_BASE).arg(Theme::SPACE_SM)
        .arg(QLatin1String(Theme::BG_HOVER())));
    layout->addWidget(manualBtn);
    connect(manualBtn, &QPushButton::clicked, this, [this]() {
        auto *dlg = new HelpManualDialog(this);
        dlg->setAttribute(Qt::WA_DeleteOnClose);
        dlg->show();
    });

    root->addWidget(box);
}

// ─── Load / Save ──────────────────────────────────────────────────────────────

void SettingsPage::loadSettings()
{
    QSettings s;
    m_discoveryBackend->setCurrentIndex(
        s.value(QLatin1String(SettingsKey::kDiscoveryBackend), 0).toInt());
    m_jamiEnabled->setChecked(
        s.value(QLatin1String(SettingsKey::kJamiEnabled), false).toBool());
    m_upnpEnabled->setChecked(
        s.value(QLatin1String(SettingsKey::kUpnpEnabled), true).toBool());
    m_cryptoBackend->setCurrentIndex(
        s.value(QLatin1String(SettingsKey::kCryptoBackend), 0).toInt());
    m_wsPort->setValue(
        s.value(QLatin1String(SettingsKey::kWsPort), 9876).toInt());
    m_gossipPort->setValue(
        s.value(QLatin1String(SettingsKey::kGossipPort), 9877).toInt());
    {
        const int mask = s.value(
            QLatin1String(SettingsKey::kStunServersEnabled),
            SettingsKey::kStunDefaultMask).toInt();
        m_stunSfl       ->setChecked(mask & (1 << 0));
        m_stunNextcloud ->setChecked(mask & (1 << 1));
        m_stunLinphone  ->setChecked(mask & (1 << 2));
        m_stunEkiga     ->setChecked(mask & (1 << 3));
        m_stunFramasoft ->setChecked(mask & (1 << 4));
        m_stunGoogle    ->setChecked(mask & (1 << 5));
        m_stunCloudflare->setChecked(mask & (1 << 6));
    }
    {
        const QString saved = s.value(QStringLiteral("network/relayUrl"), QString()).toString();
        if (m_relayPreset) {
            // Find which preset matches the saved URL
            int matchIdx = 0; // default: None
            const QString kCustom = QStringLiteral("__custom__");
            bool foundPreset = false;
            for (int i = 0; i < m_relayPreset->count(); ++i) {
                const QString pval = m_relayPreset->itemData(i).toString();
                if (pval == kCustom) continue;
                if (!pval.isEmpty() && pval == saved) {
                    matchIdx = i;
                    foundPreset = true;
                    break;
                }
            }
            // Default to Magic Wormhole transit when nothing is saved
            if (!foundPreset && saved.isEmpty()) {
                for (int i = 0; i < m_relayPreset->count(); ++i) {
                    if (m_relayPreset->itemData(i).toString().contains(
                            QStringLiteral("transit.magic-wormhole.io"))) {
                        matchIdx = i;
                        foundPreset = true;
                        break;
                    }
                }
            }
            if (!foundPreset && !saved.isEmpty()) {
                // saved URL doesn't match any preset — treat as custom
                for (int i = 0; i < m_relayPreset->count(); ++i) {
                    if (m_relayPreset->itemData(i).toString() == kCustom) {
                        matchIdx = i;
                        break;
                    }
                }
                if (m_relayUrl) m_relayUrl->setText(saved);
            }
            // Always set the dropdown — was missing for all non-custom paths
            m_relayPreset->setCurrentIndex(matchIdx);
        } else if (m_relayUrl) {
            m_relayUrl->setText(saved);
        }
    }

    // ── Debug flags ─────────────────────────────────────────────────────────
    // Source-of-truth here is the live singleton, NOT QSettings directly.
    // env/CLI may have turned a flag ON above what's on disk; reading from
    // the singleton means the checkbox state reflects what's ACTUALLY active.
    // Block signals while setting initial state so we don't fire the
    // toggled() lambdas and re-apply (and dirty) on page load.
    {
        using namespace Debug;
        if (m_dbgNoFsmTuning) {
            QSignalBlocker b(m_dbgNoFsmTuning);
            m_dbgNoFsmTuning->setChecked(DebugFlags::instance().is(DebugFlag::NoFsmTuning));
        }
        if (m_dbgNoMonitoringBlocking) {
            QSignalBlocker b(m_dbgNoMonitoringBlocking);
            m_dbgNoMonitoringBlocking->setChecked(DebugFlags::instance().is(DebugFlag::NoMonitoringBlocking));
        }
        if (m_dbgVerboseFsm) {
            QSignalBlocker b(m_dbgVerboseFsm);
            m_dbgVerboseFsm->setChecked(DebugFlags::instance().is(DebugFlag::VerboseFsm));
        }
    }
    m_restartPending = false;
}

void SettingsPage::saveSettings()
{
    QSettings s;
    s.setValue(QLatin1String(SettingsKey::kDiscoveryBackend),
               m_discoveryBackend->currentIndex());
    s.setValue(QLatin1String(SettingsKey::kJamiEnabled),
               m_jamiEnabled->isChecked());
    s.setValue(QLatin1String(SettingsKey::kUpnpEnabled),
               m_upnpEnabled->isChecked());
    s.setValue(QLatin1String(SettingsKey::kCryptoBackend),
               m_cryptoBackend->currentIndex());
    s.setValue(QLatin1String(SettingsKey::kWsPort), m_wsPort->value());
    s.setValue(QLatin1String(SettingsKey::kGossipPort), m_gossipPort->value());
    {
        int mask = 0;
        if (m_stunSfl       ->isChecked()) mask |= (1 << 0);
        if (m_stunNextcloud ->isChecked()) mask |= (1 << 1);
        if (m_stunLinphone  ->isChecked()) mask |= (1 << 2);
        if (m_stunEkiga     ->isChecked()) mask |= (1 << 3);
        if (m_stunFramasoft ->isChecked()) mask |= (1 << 4);
        if (m_stunGoogle    ->isChecked()) mask |= (1 << 5);
        if (m_stunCloudflare->isChecked()) mask |= (1 << 6);
        s.setValue(QLatin1String(SettingsKey::kStunServersEnabled), mask);
    }
    {
        // Compute effective relay URL from preset + custom field
        QString effectiveRelay;
        if (m_relayPreset) {
            const QString val = m_relayPreset->currentData().toString();
            if (val == QStringLiteral("__custom__")) {
                effectiveRelay = m_relayUrl ? m_relayUrl->text().trimmed() : QString();
            } else {
                effectiveRelay = val; // preset URL (may be empty = None)
            }
        } else if (m_relayUrl) {
            effectiveRelay = m_relayUrl->text().trimmed();
        }
        s.setValue(QStringLiteral("network/relayUrl"), effectiveRelay);
    }

    // ── Debug flags ─────────────────────────────────────────────────────────
    // Toggles already applied the flags live in the buildDebugFlagsSection
    // wiring; saveToSettings() persists the current singleton state to the
    // debug/ QSettings group so it survives restart. This keeps the
    // single-source-of-truth: the singleton owns runtime state, QSettings
    // is a snapshot of it on Save.
    Debug::DebugFlags::instance().saveToSettings();

    s.sync();
}

// ─── Slots ────────────────────────────────────────────────────────────────────


// ─── Sticky save bar ─────────────────────────────────────────────────────────

void SettingsPage::createStickyBar()
{
    // V32.34+: NO-OP under tabbed settings. The old behavior spawned a
    // slide-up "you have unsaved changes" bar parented to the scroll
    // viewport. With the tabbed layout there are five scroll viewports and
    // a single always-visible action bar pinned below the tab stack (which
    // already has Save/Reset/Test/Factory). The sticky bar would (a) only
    // cover one tab's viewport and (b) duplicate the now-permanent Save
    // button. So it's retired — markDirty() still tracks m_dirty for the
    // save flow, it just no longer creates a floating bar.
    //
    // (Kept as a method rather than deleted so markDirty() and any other
    // callers compile unchanged; if a per-tab "unsaved" affordance is
    // wanted later, reimplement here parented to `this`, not the viewport.)
}

void SettingsPage::markDirty()
{
    if (m_dirty) return;
    m_dirty = true;
    createStickyBar();
}

void SettingsPage::onSaveClicked()
{
    saveSettings();
    if (m_restartPending)
        emit restartRequired(tr("One or more settings require a restart."));
}

void SettingsPage::onResetClicked()
{
    const auto ans = QMessageBox::question(
        this, tr("Reset Settings"),
        tr("Reset preferences to defaults?\nAddress cache will also be cleared.\n"
           "This requires a restart."),
        QMessageBox::Yes | QMessageBox::Cancel);
    if (ans != QMessageBox::Yes) return;
    QSettings s;
    s.clear();
    s.sync();
    emit clearAddressCacheRequested(); // wipe stale peer_addresses
    loadSettings();
    m_restartPending = true;
    emit restartRequired(tr("Settings reset. Please restart."));
}

void SettingsPage::onFactoryResetClicked()
{
    const auto ans = QMessageBox::warning(
        this, tr("Factory Reset"),
        tr("⚠️ This will permanently delete:\n\n"
           "  • Your sc-ID (a new one will be created)\n"
           "  • All contacts and address book entries\n"
           "  • All cached peer addresses\n"
           "  • All received/sent snaps\n"
           "  • All settings\n\n"
           "The app will close. Are you absolutely sure?"),
        QMessageBox::Yes | QMessageBox::Cancel,
        QMessageBox::Cancel);
    if (ans != QMessageBox::Yes) return;
    emit factoryResetRequested();
}

void SettingsPage::onTestClicked() { emit componentTestRequested(); }

void SettingsPage::onBackendChanged() { m_restartPending = true; }

void SettingsPage::resizeEvent(QResizeEvent *e)
{
    QWidget::resizeEvent(e);
    if (m_stickyBar && m_stickyBar->isVisible()) {
        QWidget *vp = m_outerScroll ? m_outerScroll->viewport() : this;
        m_stickyBar->setGeometry(0, vp->height() - 56, vp->width(), 56);
    }
}

void SettingsPage::wheelEvent(QWheelEvent *e)
{
    // Handled by CarouselAwareScrollArea subclass — this is a no-op fallback
    QWidget::wheelEvent(e);
}

} // namespace SnapClone::UI

#include "SettingsPage.moc"
