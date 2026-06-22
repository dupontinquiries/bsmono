#include "ui/ContactsPage.hpp"
#include "core/Timings.hpp"
#include "ui/ContactEditorDialog.hpp"
#include "ui/ContactCard.hpp"   // V32.34+: expandable card w/ swipe-out submenu
#include "ui/Theme.hpp"
#include "ui/SwipeNavigator.hpp"   // V32.34+: unifiedNavEnabled() flag
#include "core/VCardIO.hpp"
#include "ui/UxGuide.hpp"
#include "ui/TouchFeedback.hpp"
#include "ui/ChevronConfirm.hpp"
#include "ui/SvgIcon.hpp"
#include "ui/SegmentedTabStrip.hpp" // V32.34+: contacts filter tabs
#include "ui/PressGesture.hpp"     // V32.30: robust long-press FSM
#include "ui/AddLandmarkDialog.hpp" // V32.31: landmark creation UI

#include <QLoggingCategory>         // V32.32: tap-to-edit diagnostics
#include <QStringList>
#include "core/AppController.hpp"   // V32.31: storage + self-id for landmarks
#include "storage/StorageManager.hpp" // V32.31: saveLandmark
#include "ui/QrScanDialog.hpp"

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QScrollArea>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <QFrame>
#include <QFileDialog>
#include <QFile>
#include <QTimer>
#include <QGroupBox>
#include <QGuiApplication>
#include <QClipboard>
#include <QMessageBox>
#include <QHideEvent>
#include <QMouseEvent>
#include <QTimer>
#include <QToolTip>
#include <QGraphicsOpacityEffect>
#include <QPropertyAnimation>
#include <algorithm>

namespace SnapClone::UI {

// ─── Helpers ─────────────────────────────────────────────────────────────────

static QColor avatarColorFor(const QString &id)
{
    // Deterministic warm palette from the sc-ID
    static const QColor kPalette[] = {
        QColor(0x7A,0x6E,0x60), QColor(0x5E,0x94,0x70),
        QColor(0x6E,0x7A,0x9A), QColor(0x9A,0x6E,0x6E),
        QColor(0x7A,0x8A,0x6E), QColor(0x8A,0x6E,0x8A),
        QColor(0x6E,0x8A,0x8A), QColor(0xC4,0x9A,0x3C),
    };
    uint h = 0;
    for (const QChar c : id) h = h * 31 + c.unicode();
    return kPalette[h % 8];
}

static QString initials(const Core::Contact &c)
{
    const QString lbl = c.label();
    if (lbl.isEmpty()) return QStringLiteral("?");
    const QStringList words = lbl.split(QLatin1Char(' '), Qt::SkipEmptyParts);
    if (words.size() >= 2)
        return QString(words[0][0]).toUpper() + QString(words[1][0]).toUpper();
    return QString(lbl[0]).toUpper();
}

// Human-readable relative timestamp: "just now", "3 min ago", "yesterday", etc.
static QString humanRelativeTime(const QDateTime &dt)
{
    if (!dt.isValid()) return QString();
    const qint64 secs = dt.secsTo(QDateTime::currentDateTimeUtc());
    if (secs < 0)    return QCoreApplication::translate("ContactsPage", "just now");
    if (secs < 60)   return QCoreApplication::translate("ContactsPage", "just now");
    if (secs < 3600) {
        const int m = static_cast<int>(secs / 60);
        return QCoreApplication::translate("ContactsPage", "%n min ago", "", m);
    }
    const qint64 hours = secs / 3600;
    if (hours < 24) {
        return hours == 1
            ? QCoreApplication::translate("ContactsPage", "1 hour ago")
            : QCoreApplication::translate("ContactsPage", "%1 hours ago").arg(hours);
    }
    const qint64 days = secs / 86400;
    if (days == 1) return QCoreApplication::translate("ContactsPage", "yesterday");
    if (days <  7) return QCoreApplication::translate("ContactsPage", "%n days ago", "", static_cast<int>(days));
    const qint64 weeks = days / 7;
    if (weeks < 5) return weeks == 1
        ? QCoreApplication::translate("ContactsPage", "1 week ago")
        : QCoreApplication::translate("ContactsPage", "%1 weeks ago").arg(weeks);
    const qint64 months = days / 30;
    return months == 1
        ? QCoreApplication::translate("ContactsPage", "1 month ago")
        : QCoreApplication::translate("ContactsPage", "%1 months ago").arg(months);
}

// ─── Construction ─────────────────────────────────────────────────────────────

ContactsPage::ContactsPage(QWidget *parent)
    : QWidget(parent)
{
    buildUi();

}

void ContactsPage::buildUi()
{
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);

    // ── Top bar: title + search ───────────────────────────────────────────
    auto *topBar = new QWidget(this);
    topBar->setStyleSheet(QStringLiteral(
        "background:%1; border-bottom:1px solid %2;")
        .arg(QLatin1String(Theme::BG_CARD()), QLatin1String(Theme::BORDER())));
    auto *topLayout = new QVBoxLayout(topBar);
    // V32.30: viewport-relative side margins (see Theme::pageSideMarginPx).
    topLayout->setContentsMargins(Theme::pageSideMarginPx(),
                                   Theme::pageTopMarginPx(),
                                   Theme::pageSideMarginPx(),
                                   Theme::SPACE_SM);
    topLayout->setSpacing(Theme::SPACE_SM);

    auto *titleRow = new QHBoxLayout;
    auto *title = new QLabel(tr("Contacts"), topBar);
    title->setObjectName(QStringLiteral("titleLabel"));
    titleRow->addWidget(title);
    titleRow->addStretch();

    auto *exportAllBtn = new QPushButton(topBar);
    exportAllBtn->setObjectName(QStringLiteral("iconBtn"));
    exportAllBtn->setToolTip(tr("Export all contacts as vCard"));
    exportAllBtn->setIcon(SvgIcon::secondary(QStringLiteral("export")));
    exportAllBtn->setIconSize(QSize(20, 20));
    exportAllBtn->setStyleSheet(QStringLiteral(
        "QPushButton { border-radius:%1px; font-size:18px;"
        " background:%2; border:1px solid %3; min-width:%4px; min-height:%4px; }"
        "QPushButton:pressed { background:%5; }")
        .arg(Theme::RADIUS_SM).arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::BORDER())).arg(Theme::TOUCH_MIN)
        .arg(QLatin1String(Theme::BG_HOVER())));
    connect(exportAllBtn, &QPushButton::clicked, this, [this, exportAllBtn]() {
        // Gate through ChevronConfirm — prevents accidental taps.
        // Familiar users (score >= 80) bypass it entirely.
        ChevronConfirm::present(exportAllBtn,
            tr("Export all contacts as a vCard file?"),
            QLatin1String(Feature::kExportVCard),
            [this]() {
                const QString path = QFileDialog::getSaveFileName(
                    this, tr("Export Contacts"),
                    QStringLiteral("contacts.vcf"),
                    tr("vCard (*.vcf)"));
                if (path.isEmpty()) return;
                QFile f(path);
                if (!f.open(QIODevice::WriteOnly | QIODevice::Text)) return;
                const QList<Core::Contact> list = m_contacts.values();
                f.write(Core::VCardIO::exportContacts(list).toUtf8());
                UxGuide::instance().recordUse(QLatin1String(Feature::kExportVCard));
            });
    });
    // Ripple on press — immediate touch feedback
    TouchFeedback::install(exportAllBtn, TouchFeedback::Effect::Ripple);
    titleRow->addWidget(exportAllBtn);

    // Import vCard button
    auto *importBtn = new QPushButton(topBar);
    importBtn->setObjectName(QStringLiteral("iconBtn"));
    importBtn->setToolTip(tr("Import contacts from vCard"));
    importBtn->setIcon(SvgIcon::secondary(QStringLiteral("import"), QSize(20,20)));
    importBtn->setIconSize(QSize(20, 20));
    importBtn->setStyleSheet(exportAllBtn->styleSheet());
    connect(importBtn, &QPushButton::clicked, this, &ContactsPage::importVCardRequested);
    titleRow->addWidget(importBtn);

    // QR scan button
    auto *qrScanBtn = new QPushButton(topBar);
    qrScanBtn->setObjectName(QStringLiteral("iconBtn"));
    qrScanBtn->setToolTip(tr("Scan QR code to add contact"));
    qrScanBtn->setIcon(SvgIcon::secondary(QStringLiteral("camera"), QSize(20,20)));
    qrScanBtn->setIconSize(QSize(20, 20));
    qrScanBtn->setStyleSheet(exportAllBtn->styleSheet());
    connect(qrScanBtn, &QPushButton::clicked, this, &ContactsPage::qrScanRequested);
    titleRow->addWidget(qrScanBtn);

    // V32.31: Add landmark button — primary entry point for the
    // landmarking feature (per design discussion). Uses the
    // map-pin SVG. The viewfinder swipe shortcut comes later;
    // this is the always-discoverable entry.
    auto *addLandmarkBtn = new QPushButton(topBar);
    addLandmarkBtn->setObjectName(QStringLiteral("iconBtn"));
    addLandmarkBtn->setToolTip(tr("Add landmark at current location"));
    addLandmarkBtn->setIcon(SvgIcon::secondary(QStringLiteral("landmark"),
                                                 QSize(20, 20)));
    addLandmarkBtn->setIconSize(QSize(20, 20));
    addLandmarkBtn->setStyleSheet(exportAllBtn->styleSheet());
    connect(addLandmarkBtn, &QPushButton::clicked,
            this, &ContactsPage::onAddLandmarkClicked);
    titleRow->addWidget(addLandmarkBtn);

    topLayout->addLayout(titleRow);

    m_filterEdit = new QLineEdit(topBar);
    m_filterEdit->setPlaceholderText(tr("Search contacts…"));
    m_filterEdit->setClearButtonEnabled(true);
    // Debounce: wait 120ms after last keystroke before rebuilding cards.
    // Prevents card deletion mid-touch when autocorrect fires textChanged.
    auto *filterTimer = new QTimer(this);
    filterTimer->setSingleShot(true);
    filterTimer->setInterval(120);
    connect(m_filterEdit, &QLineEdit::textChanged,
            filterTimer, qOverload<>(&QTimer::start));
    connect(filterTimer, &QTimer::timeout,
            this, [this]() { onFilterChanged(m_filterEdit->text()); });
    topLayout->addWidget(m_filterEdit);

    // ── Filter tabs (All / Online / Pending) ──────────────────────────────
    // V32.34+ contacts: shared in-page tab pattern (SegmentedTabStrip),
    // matching tabbed Settings + the mailbox All/Unread tabs. Composes with
    // the text search above — both narrow the card list. "Online" carries a
    // live count badge so the user sees how many peers are connected at a
    // glance.
    m_filterTabs = new SegmentedTabStrip(
        { tr("All"), tr("Online"), tr("Pending") }, topBar);
    connect(m_filterTabs, &SegmentedTabStrip::tabSelected, this,
            [this](int idx) { applyCardFilterIndex(idx); });
    topLayout->addWidget(m_filterTabs);
    // V32.34+ unified nav: bottom UnifiedNavBar renders these sub-tabs now —
    // hide the redundant top strip (kept constructed for its wiring; see
    // InboxWidget for the rationale on hide-not-delete).
    if (SwipeNavigator::unifiedNavEnabled() && m_filterTabs)
        m_filterTabs->hide();

    root->addWidget(topBar);

    // ── Scrollable card list ──────────────────────────────────────────────
    m_scroll = new QScrollArea(this);
    m_scroll->setWidgetResizable(true);
    m_scroll->setFrameShape(QFrame::NoFrame);
    m_scroll->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);

    m_cardContainer = new QWidget(m_scroll);
    m_cardLayout = new QVBoxLayout(m_cardContainer);
    m_cardLayout->setContentsMargins(Theme::SPACE_SM, Theme::SPACE_SM,
                                      Theme::SPACE_SM, Theme::SPACE_SM);
    m_cardLayout->setSpacing(Theme::SPACE_XS);

    m_emptyLabel = new QLabel(
        tr("No contacts yet.\n\nAdd a peer below using their sc-ID\nor IP address."),
        m_cardContainer);
    m_emptyLabel->setAlignment(Qt::AlignCenter);
    m_emptyLabel->setObjectName(QStringLiteral("subtitleLabel"));
    m_emptyLabel->setWordWrap(true);
    m_cardLayout->addWidget(m_emptyLabel);
    m_cardLayout->addStretch();

    m_scroll->setWidget(m_cardContainer);
    root->addWidget(m_scroll, 1);

    // 3-second timer: refresh all visible netstat labels from m_peerStats.
    // Each refresh fades the label in from opacity 0 → 1 over 400 ms.
    m_statTimer = new QTimer(this);
    m_statTimer->setInterval(Timings::kStatRefreshMs);
    // Only run while the page is visible — saves wakeups on other tabs
    connect(m_statTimer, &QTimer::timeout, this, [this]() {
        if (!isVisible() || !m_cardContainer) return;
        for (auto it = m_peerStats.constBegin(); it != m_peerStats.constEnd(); ++it) {
            const QString name = QStringLiteral("netstat_") + it.key();
            auto *lbl = m_cardContainer->findChild<QLabel *>(name);
            if (!lbl) continue;
            lbl->setText(it.value());
            lbl->setVisible(true);
            auto *effect = qobject_cast<QGraphicsOpacityEffect *>(lbl->graphicsEffect());
            if (!effect) continue;
            // Fade in: opacity 0 → 1 over 400 ms
            auto *anim = new QPropertyAnimation(effect, "opacity", lbl);
            anim->setDuration(400);
            anim->setStartValue(0.0);
            anim->setEndValue(1.0);
            anim->setEasingCurve(QEasingCurve::InOutQuad);
            anim->start(QAbstractAnimation::DeleteWhenStopped);
        }
    });
    m_statTimer->start();

    // ── Add Peer section (contains beacon expansion panel) ───────────────
    auto *addBox = new QGroupBox(tr("Add Peer"), this);
    auto *addLayout = new QVBoxLayout(addBox);
    addLayout->setContentsMargins(Theme::SPACE_SM, Theme::SPACE_SM,
                                   Theme::SPACE_SM, Theme::SPACE_SM);
    addLayout->setSpacing(Theme::SPACE_XS);

    // ── Beacon expansion panel — hidden until 📡 button is tapped ────────
    m_beaconPanel = new QWidget(addBox);
    m_beaconPanel->setVisible(false); // collapsed by default
    auto *beaconInnerLayout = new QVBoxLayout(m_beaconPanel);
    beaconInnerLayout->setContentsMargins(0, 0, 0, Theme::SPACE_XS);
    beaconInnerLayout->setSpacing(Theme::SPACE_XS);

    // Status line
    m_beaconStatus = new QLabel(tr("Tap Start to scan for nearby devices"), m_beaconPanel);
    m_beaconStatus->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent;")
        .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    beaconInnerLayout->addWidget(m_beaconStatus);

    // Start/Stop button
    m_beaconStartBtn = new QPushButton(tr("Start Scanning"), m_beaconPanel);
    m_beaconStartBtn->setObjectName(QStringLiteral("accentBtn"));
    m_beaconStartBtn->setFixedHeight(Theme::TOUCH_MIN);
    connect(m_beaconStartBtn, &QPushButton::clicked, this, [this]() {
        emit beaconToggleRequested(!m_beaconActive);
    });
    beaconInnerLayout->addWidget(m_beaconStartBtn);
    TouchFeedback::install(m_beaconStartBtn, TouchFeedback::Effect::Ripple);

    // Collapsible nearby-peer list lives here (populated by addNearbyPeer)
    m_beaconContent = new QWidget(m_beaconPanel);
    auto *cLayout = new QVBoxLayout(m_beaconContent);
    cLayout->setContentsMargins(0, 0, 0, 0);
    cLayout->setSpacing(Theme::SPACE_XS);
    beaconInnerLayout->addWidget(m_beaconContent);

    // Separator below beacon panel
    auto *beaconSep = new QFrame(addBox);
    beaconSep->setFrameShape(QFrame::HLine);
    beaconSep->setFixedHeight(1);
    beaconSep->setStyleSheet(QStringLiteral("border:none; background:%1;")
        .arg(QLatin1String(Theme::BORDER())));
    beaconSep->setVisible(false); // shown when beacon expands

    // Connect beacon panel visibility to separator
    // (use a lambda stored nowhere — beacon panel lifetime owns it)
    connect(m_beaconStartBtn, &QPushButton::clicked,
            beaconSep, [beaconSep, this]() {
        beaconSep->setVisible(m_beaconPanel->isVisible());
    });

    addLayout->addWidget(m_beaconPanel);
    addLayout->addWidget(beaconSep);

    // ── Input row: sc-ID + port ───────────────────────────────────────────
    auto *addrRow = new QHBoxLayout;
    m_hostEdit = new QLineEdit(addBox);
    m_hostEdit->setPlaceholderText(tr("sc-ID  or  IP address"));
    m_hostEdit->setInputMethodHints(Qt::ImhUrlCharactersOnly
                                    | Qt::ImhNoPredictiveText
                                    | Qt::ImhNoAutoUppercase);
    addrRow->addWidget(m_hostEdit, 3);

    m_portEdit = new QLineEdit(addBox);
    m_portEdit->setPlaceholderText(tr("Port"));
    m_portEdit->setInputMethodHints(Qt::ImhDigitsOnly);
    m_portEdit->setMaximumWidth(72);
    m_portEdit->setText(QStringLiteral("9876"));
    addrRow->addWidget(m_portEdit, 1);
    addLayout->addLayout(addrRow);

    // ── Button row: Connect [stretch] | 📥 | 📡 ─────────────────────────
    const QString iconBtnStyle = QStringLiteral(
        "QPushButton { background:%1; border:1px solid %2; border-radius:%3px;"
        "  font-size:18px; min-width:%4px; min-height:%4px; max-width:%4px; max-height:%4px; }"
        "QPushButton:pressed { background:%5; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::RADIUS_SM)
        .arg(Theme::TOUCH_MIN)
        .arg(QLatin1String(Theme::BG_HOVER()));

    auto *btnRow = new QHBoxLayout;
    btnRow->setSpacing(Theme::SPACE_XS);

    m_addBtn = new QPushButton(tr("Connect"), addBox);
    m_addBtn->setObjectName(QStringLiteral("accentBtn"));
    m_addBtn->setFixedHeight(Theme::TOUCH_MIN);
    connect(m_addBtn, &QPushButton::clicked, this, &ContactsPage::onAddClicked);
    btnRow->addWidget(m_addBtn, 1); // stretch — takes all remaining space

    auto *importBtn2 = new QPushButton(QStringLiteral("📥"), addBox);
    importBtn2->setToolTip(tr("Import vCard"));
    importBtn2->setStyleSheet(iconBtnStyle);
    connect(importBtn2, &QPushButton::clicked, this, &ContactsPage::onImportVCard);
    btnRow->addWidget(importBtn2);

    m_beaconToggle = new QPushButton(QStringLiteral("📡"), addBox);
    m_beaconToggle->setToolTip(tr("Scan for nearby devices"));
    m_beaconToggle->setStyleSheet(iconBtnStyle);
    m_beaconToggle->setCheckable(true);
    connect(m_beaconToggle, &QPushButton::toggled, this, [this, beaconSep](bool on) {
        m_beaconPanel->setVisible(on);
        beaconSep->setVisible(on);
        if (!on && m_beaconActive)
            emit beaconToggleRequested(false);
    });
    btnRow->addWidget(m_beaconToggle);

    addLayout->addLayout(btnRow);

    // Prune timer — removes beacon peers that haven't been seen for 30s
    m_beaconPruneTimer = new QTimer(this);
    m_beaconPruneTimer->setInterval(Timings::kBeaconPruneMs);
    connect(m_beaconPruneTimer, &QTimer::timeout, this, [this]() {
        if (isVisible()) pruneStaleNearbyPeers();
    });

    root->addWidget(addBox);

    // V32.34+: restore the last-used filter tab (persisted across launches),
    // defaulting to All. m_cardLayout exists by now, so the tabSelected
    // handler's rebuildCards() is safe — it'll show skeletons until contacts
    // load, then real cards. Wired AFTER the tabSelected connect above.
    if (m_filterTabs) m_filterTabs->setPersistenceKey(
        QStringLiteral("ui/tabs/contacts"));
}

// ─── Beacon panel ─────────────────────────────────────────────────────────────


void ContactsPage::toggleBeaconPanel()
{
    // Drive through the checkable button so the toggled() signal fires
    // and the visibility lambda runs consistently.
    if (m_beaconToggle)
        m_beaconToggle->setChecked(!m_beaconToggle->isChecked());
    m_beaconExpanded = m_beaconToggle ? m_beaconToggle->isChecked() : false;
    if (!m_beaconExpanded && m_beaconActive) {
        // Auto-stop when collapsing
        emit beaconToggleRequested(false);
    }
}

void ContactsPage::setBeaconActive(bool active)
{
    m_beaconActive = active;
    if (m_beaconStartBtn)
        m_beaconStartBtn->setText(active ? tr("Stop Beacon") : tr("Start Beacon"));
    if (m_beaconStatus) {
        m_beaconStatus->setText(active
            ? tr("scanning...") // scanning…
            : tr("tap to scan"));
    }
    if (active)
        m_beaconPruneTimer->start();
    else {
        m_beaconPruneTimer->stop();
        // Clear nearby peers on stop
        m_nearbyPeers.clear();
        rebuildCards();
    }
}

void ContactsPage::onNearbyPeerSeen(const Core::UserId &id, const QString &displayName)
{
    // Don't show peers already in contacts
    if (m_contacts.contains(id.value)) return;

    NearbyPeer &peer = m_nearbyPeers[id.value];
    peer.id          = id;
    peer.displayName = displayName.isEmpty() ? id.value : displayName;
    peer.lastSeenMs  = QDateTime::currentMSecsSinceEpoch();

    // Update beacon status
    const int n = static_cast<int>(m_nearbyPeers.size());
    if (m_beaconStatus)
        m_beaconStatus->setText(n == 1
            ? tr("1 device nearby")
            : tr("%1 devices nearby").arg(n));

    rebuildCards(); // adds/updates Nearby section
}

void ContactsPage::pruneStaleNearbyPeers()
{
    const qint64 now    = QDateTime::currentMSecsSinceEpoch();
    const qint64 maxAge = 30000; // 30s
    bool changed = false;
    for (auto it = m_nearbyPeers.begin(); it != m_nearbyPeers.end(); ) {
        if (now - it->lastSeenMs > maxAge) {
            it = m_nearbyPeers.erase(it);
            changed = true;
        } else {
            ++it;
        }
    }
    if (changed) {
        const int n = static_cast<int>(m_nearbyPeers.size());
        if (m_beaconStatus)
            m_beaconStatus->setText(n == 0 ? tr("scanning...")
                : n == 1 ? tr("1 device nearby") : tr("%1 devices nearby").arg(n));
        rebuildCards();
    }
}

// ─── Card builder ─────────────────────────────────────────────────────────────

QWidget *ContactsPage::makeCard(const Core::Contact &c)
{
    auto *card = new QWidget(m_cardContainer);
    card->setObjectName(QStringLiteral("contactCardHeader_") + c.id.value);
    card->setCursor(Qt::PointingHandCursor);

    const QColor avatarBg = avatarColorFor(c.id.value);
    const bool online = c.online;

    card->setStyleSheet(QStringLiteral(
        "QWidget { background:%1; border-radius:%2px;"
        " border:1px solid %3; }"
        "QWidget:hover { background:%4; border-color:%5; }")
        .arg(QLatin1String(Theme::BG_CARD())).arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::BORDER()))
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::BORDER_FOCUS())));

    auto *row = new QHBoxLayout(card);
    row->setContentsMargins(Theme::SPACE_SM, Theme::SPACE_SM,
                             Theme::SPACE_SM, Theme::SPACE_SM);
    row->setSpacing(Theme::SPACE_SM);

    // ── Avatar circle ─────────────────────────────────────────────────────
    auto *avatar = new QLabel(initials(c), card);
    avatar->setFixedSize(48, 48);
    avatar->setAlignment(Qt::AlignCenter);
    avatar->setStyleSheet(QStringLiteral(
        "background:%1; border-radius:24px; color:white;"
        " font-size:17px; font-weight:600; border:none;")
        .arg(avatarBg.name()));
    row->addWidget(avatar);

    // ── Name + sc-ID ─────────────────────────────────────────────────────
    auto *textCol = new QVBoxLayout;
    textCol->setSpacing(2);

    auto *nameLabel = new QLabel(c.label(), card);
    nameLabel->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:600; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_BASE).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    nameLabel->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Preferred);
    textCol->addWidget(nameLabel);

    // Second line: sc-ID for online/Pending contacts, "last seen X ago" for offline Known.
    QString idLineText = c.id.value;
    if (!c.online && c.state == Core::ContactState::Known && c.lastSeen.isValid()) {
        const QString rel = humanRelativeTime(c.lastSeen);
        if (!rel.isEmpty())
            idLineText = tr("last seen %1").arg(rel);
    }
    auto *idLabel = new QLabel(idLineText, card);
    idLabel->setObjectName(QStringLiteral("idLine_") + c.id.value);
    idLabel->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; font-family:monospace; background:transparent; border:none;")
        .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
    textCol->addWidget(idLabel);

    // V32.34+ contacts depth: conversation-activity line. When we have
    // stored conversation history with this contact, show "↙/↗ <reltime>"
    // (direction of the most recent snap + how long ago) so the card reads
    // as a conversation summary at a glance. Unread threads get an accent
    // color + bold. Contacts with no history simply omit this line.
    {
        const auto ait = m_convActivity.constFind(c.id.value);
        if (ait != m_convActivity.constEnd() && ait->valid
            && ait->lastSnapAt.isValid()) {
            const QString rel = humanRelativeTime(ait->lastSnapAt);
            QString actText = tr("last snap %1").arg(rel.isEmpty()
                ? tr("recently") : rel);
            if (ait->unreadCount > 0)
                actText = tr("%1 new · %2")
                    .arg(ait->unreadCount).arg(actText);
            auto *actLine = new QLabel(actText, card);
            actLine->setObjectName(QStringLiteral("convact_") + c.id.value);
            const bool unread = ait->unreadCount > 0;
            actLine->setStyleSheet(QStringLiteral(
                "font-size:%1px; color:%2; background:transparent;"
                " border:none;%3")
                .arg(Theme::FONT_XS)
                .arg(unread ? QLatin1String(Theme::ACCENT())
                            : QLatin1String(Theme::TEXT_SECONDARY()))
                .arg(unread ? QStringLiteral(" font-weight:700;")
                            : QString()));
            textCol->addWidget(actLine);
        }
    }

    // Connection status line — hidden until a connection attempt starts.
    // Updated live by setConnectionStatus() as phases progress.
    auto *statusLine = new QLabel(card);
    statusLine->setObjectName(QStringLiteral("status_") + c.id.value);
    statusLine->setFixedHeight(16);
    statusLine->setStyleSheet(QStringLiteral(
        "font-size:%1px; background:transparent; border:none;")
        .arg(Theme::FONT_XS));
    statusLine->setVisible(false);
    textCol->addWidget(statusLine);

    // Network stat line — shows transport type + address, updated every 3 s with a fade.
    auto *netstatLine = new QLabel(card);
    netstatLine->setObjectName(QStringLiteral("netstat_") + c.id.value);
    netstatLine->setFixedHeight(14);
    netstatLine->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; font-family:monospace;"
        " background:transparent; border:none;")
        .arg(Theme::FONT_XS).arg(QLatin1String(Theme::TEXT_DISABLED())));
    netstatLine->setVisible(false);
    // Opacity effect — used for the fade-in animation.
    auto *fx = new QGraphicsOpacityEffect(netstatLine);
    fx->setOpacity(0.0);
    netstatLine->setGraphicsEffect(fx);
    textCol->addWidget(netstatLine);

    // Last-location hint — shown for offline contacts with a recent GPS fix.
    // Updated by setLastLocation() when AppController emits locationCacheUpdated.
    auto *locHintLine = new QLabel(card);
    locHintLine->setObjectName(QStringLiteral("lochint_") + c.id.value);
    locHintLine->setFixedHeight(13);
    locHintLine->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_XS)
        .arg(QLatin1String(Theme::TEXT_DISABLED())));
    locHintLine->setVisible(false);
    textCol->addWidget(locHintLine);

    if (!c.notes.isEmpty()) {
        auto *notesLabel = new QLabel(c.notes, card);
        notesLabel->setStyleSheet(QStringLiteral(
            "font-size:%1px; color:%2; background:transparent; border:none;")
            .arg(Theme::FONT_SM).arg(QLatin1String(Theme::TEXT_SECONDARY())));
        notesLabel->setWordWrap(true);
        textCol->addWidget(notesLabel);
    }
    row->addLayout(textCol, 1);

    // ── Online badge ──────────────────────────────────────────────────────
    auto *badge = new QLabel(online ? QStringLiteral("●") : QStringLiteral("○"), card);
    badge->setObjectName(QStringLiteral("badge_") + c.id.value);
    badge->setStyleSheet(QStringLiteral(
        "font-size:14px; color:%1; background:transparent; border:none;")
        .arg(online ? QLatin1String(Theme::SUCCESS()) : QLatin1String(Theme::TEXT_DISABLED())));
    badge->setObjectName(QStringLiteral("badge_") + c.id.value);
    row->addWidget(badge);

    // Online badge pulse: gently oscillate opacity when peer is connected.
    // Draws the eye to active contacts without being distracting.
    if (online) {
        auto *pulseFx = new QGraphicsOpacityEffect(badge);
        pulseFx->setOpacity(1.0);
        badge->setGraphicsEffect(pulseFx);
        auto *pulse = new QPropertyAnimation(pulseFx, "opacity", badge);
        pulse->setDuration(1800);
        pulse->setStartValue(1.0);
        pulse->setKeyValueAt(0.5, 0.5);
        pulse->setEndValue(1.0);
        pulse->setEasingCurve(QEasingCurve::SineCurve);
        pulse->setLoopCount(-1);
        pulse->start();
    }

    // ── Call button (voice builds only) ───────────────────────────────────
    // Starts a 1:1 voice call by opening the conversation with this contact
    // already in-call (InboxWidget::startCallWith), so the user can call without
    // first exchanging a snap. Compiled in only when the voice backend is built.
#if defined(SNAPCLONE_RUST_GROUP) && defined(SNAPCLONE_RUST_AUDIO)
    {
        auto *callBtn = new QPushButton(QStringLiteral("call"), card);
        callBtn->setFixedSize(Theme::TOUCH_MIN, Theme::TOUCH_MIN);
        callBtn->setToolTip(tr("Start a voice call with this contact"));
        callBtn->setStyleSheet(QStringLiteral(
            "QPushButton { background:transparent; border:none; font-size:11px; font-weight:600;"
            " color:%1; border-radius:%2px; }"
            "QPushButton:hover { background:%3; }"
            "QPushButton:pressed { background:%4; }")
            .arg(QLatin1String(Theme::ACCENT())).arg(Theme::RADIUS_SM)
            .arg(QLatin1String(Theme::BG_RAISED()))
            .arg(QLatin1String(Theme::BG_HOVER())));
        const SnapClone::Core::UserId callId = c.id;
        const QString callName = c.label();
        connect(callBtn, &QPushButton::clicked, this, [this, callId, callName]() {
            emit callRequested(callId, callName);
        });
        row->addWidget(callBtn);
    }
#endif

    // ── Share location button ─────────────────────────────────────────────
    // Sends the user's current GPS fix to this contact as a Location snap.
    // Privacy gate: MainWindow handler checks the "Share location" Setting
    // before actually transmitting; if disabled, shows a hint to enable it.
    auto *shareLocBtn = new QPushButton(QStringLiteral("loc"), card);
    shareLocBtn->setFixedSize(Theme::TOUCH_MIN, Theme::TOUCH_MIN);
    shareLocBtn->setToolTip(tr("Share my current location with this contact"));
    shareLocBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background:transparent; border:none; font-size:11px; font-weight:600;"
        " color:%1; border-radius:%2px; }"
        "QPushButton:hover { background:%3; }"
        "QPushButton:pressed { background:%4; }")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())).arg(Theme::RADIUS_SM)
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::BG_HOVER())));
    const SnapClone::Core::UserId shareTargetId = c.id;
    connect(shareLocBtn, &QPushButton::clicked, this, [this, shareTargetId]() {
        emit locationShareRequested(shareTargetId);
    });
    row->addWidget(shareLocBtn);

    // ── Edit button ───────────────────────────────────────────────────────
    // V32.24 changes:
    //   1. Was text-label "edit" at 11px — invisible on mobile despite
    //      taking up TOUCH_MIN×TOUCH_MIN. Now uses the pencil SVG icon
    //      via SvgIcon::secondary so it actually shows.
    //   2. Mistap-safe trigger. Editing a contact has destructive
    //      potential (rename, change sc-id, delete) and the button sits
    //      right next to the share-location button in a tight row, so
    //      easy to hit by accident. Switched to LONG-PRESS only — a
    //      brief tap shows a transient hint instead of opening the
    //      editor. Single tap-and-hold for ~500ms opens it.
    //
    //   Implementation note: we use the QPushButton's `pressed` /
    //   `released` signals plus a per-button QTimer to detect the hold.
    //   Did NOT use Qt's built-in `autoRepeat` because that fires
    //   repeatedly during hold which we don't want.
    auto *editBtn = new QPushButton(card);
    editBtn->setIcon(SvgIcon::secondary(QStringLiteral("edit"), QSize(20, 20)));
    editBtn->setIconSize(QSize(20, 20));
    editBtn->setFixedSize(Theme::TOUCH_MIN, Theme::TOUCH_MIN);
    editBtn->setToolTip(tr("Edit contact"));
    editBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background:transparent; border:none;"
        " color:%1; border-radius:%2px; }"
        "QPushButton:hover { background:%3; }"
        "QPushButton:pressed { background:%4; }")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())).arg(Theme::RADIUS_SM)
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::BG_HOVER())));
    const QString cardId = c.id.value;

    // V32.31: BELT-AND-BRACES long-press detection. User reported
    // "tap to edit still does nothing" even after the V32.30 PressGesture
    // FSM was added. Rather than debug PressGesture in isolation, run
    // BOTH approaches in parallel:
    //
    //   1. PressGesture (preferred path — touch-and-mouse aware)
    //   2. The original V32.24 pressed/QTimer/released signal chain
    //
    // Whichever fires first opens the editor. A simple "already fired"
    // guard prevents double-opening when both paths happen to work.
    //
    // The V32.24 path works on desktop mouse (verified historically) but
    // was unreliable on touch — exactly the case PressGesture was meant
    // to cover. Keeping both means the user is never stuck without a
    // working long-press, regardless of which gesture stack we trip on.
    //
    // V32.32 (revision 2): SIMPLIFIED further — `alreadyFired` guard
    // removed. It was a holdover from when three parallel paths were
    // wired (PressGesture, pressed/QTimer/released fallback, plain
    // clicked) and we needed to prevent multiple-firing within ONE
    // gesture. With only `clicked` left there's no race to guard
    // against — but the flag was BUTTON-SCOPED, not gesture-scoped,
    // so it stuck `true` forever after the first click. That's why
    // the second tap on the same card silently no-op'd:
    //   tap 1: alreadyFired=0 → fires → set to 1
    //   tap 2: alreadyFired=1 → bails (BUG)
    // Now: each click just calls openEditor directly. The dialog
    // itself prevents double-open: if dialog A is already visible
    // and another click happens, openEditor creates dialog B but
    // dialog A is still on top of the modal stack, so the user
    // resolves A first and the redundant B is a non-issue.
    connect(editBtn, &QPushButton::clicked, this,
            [this, cardId]() {
        qInfo("edit: QPushButton::clicked fired (cardId=%s)", qPrintable(cardId));
        openEditor(cardId);
    });

    row->addWidget(editBtn);

    // ── V32.34+ EXPANDING SUBMENU ────────────────────────────────────────────
    // Wrap the header row (everything built above) in a ContactCard, which adds
    // a collapsible action submenu that opens on tap and closes on swipe-out.
    // The inline buttons (share-loc, edit) keep working — ContactCard's header
    // event filter only treats a near-zero-travel release as a toggle, so a
    // button's own click is consumed first. The submenu carries the lower-
    // frequency / destructive actions that don't deserve permanent row space.
    auto *expandable = new ContactCard(c.id.value, m_cardContainer);
    expandable->setObjectName(QStringLiteral("contactCard_") + c.id.value);
    expandable->setHeader(card);   // `card` becomes the always-visible header

    const QString actId   = c.id.value;
    const QString actLabel = c.label();
    // Edit — same editor the long-press/pencil opens, now also reachable here.
    expandable->addAction(tr("Edit contact"), [this, actId]() {
        openEditor(actId);
    }, QStringLiteral("edit"));
    // Share location — mirrors the inline loc button for discoverability.
    {
        const SnapClone::Core::UserId shareId = c.id;
        expandable->addAction(tr("Share my location"), [this, shareId]() {
            emit locationShareRequested(shareId);
        }, QStringLiteral("beacon"));
    }
    // Add to call — breakout action to escalate the active call to a group (or
    // start one with this contact). Voice builds only.
#if defined(SNAPCLONE_RUST_GROUP) && defined(SNAPCLONE_RUST_AUDIO)
    {
        const SnapClone::Core::UserId addId = c.id;
        const QString addName = c.label();
        expandable->addAction(tr("Add to call"), [this, addId, addName]() {
            emit addToCallRequested(addId, addName);
        }, QStringLiteral("friends"));
    }
#endif
    // Copy sc-ID to clipboard.
    expandable->addAction(tr("Copy ID"), [actId]() {
        if (auto *cb = QGuiApplication::clipboard()) cb->setText(actId);
    }, QStringLiteral("export"));
    // Delete — destructive, styled red. Confirms inline, then removes via the
    // same removeContact() path the editor's delete uses (keeps one delete
    // codepath). Self-contained: no extra signal/wiring needed.
    {
        const Core::UserId delId = c.id;
        expandable->addAction(tr("Remove contact"), [this, delId, actLabel]() {
            const auto ret = QMessageBox::question(
                this, tr("Remove contact"),
                tr("Remove %1 from your contacts? This can't be undone.")
                    .arg(actLabel),
                QMessageBox::Yes | QMessageBox::No, QMessageBox::No);
            if (ret == QMessageBox::Yes) {
                removeContact(delId);
                emit contactDeleted(delId);
            }
        }, QStringLiteral("import"), /*danger=*/true);
    }

    // Single-open-at-a-time: when one card expands, collapse the previously
    // open one. Tracked on the page so swiping between cards feels tidy.
    connect(expandable, &ContactCard::expanded, this,
            [this](ContactCard *self) {
        if (m_openCard && m_openCard != self) m_openCard->collapse();
        m_openCard = self;
    });

    return expandable;
}

QWidget *ContactsPage::makeNearbyCard(const NearbyPeer &np)
{
    auto *card = new QWidget(m_cardContainer);
    card->setObjectName(QStringLiteral("nearbyCard_") + np.id.value);
    card->setStyleSheet(QStringLiteral(
        "QWidget { background:%1; border-radius:%2px; border:1px solid %3; }"
        "QWidget:hover { background:%4; }")
        .arg(QLatin1String(Theme::BG_CARD())).arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::ACCENT_DIM()))   // tinted border — "warm" pulse
        .arg(QLatin1String(Theme::BG_RAISED())));

    auto *row = new QHBoxLayout(card);
    row->setContentsMargins(Theme::SPACE_SM, Theme::SPACE_SM,
                             Theme::SPACE_SM, Theme::SPACE_SM);
    row->setSpacing(Theme::SPACE_SM);

    // Avatar with antenna glyph to distinguish from regular contacts
    auto *avatar = new QLabel(QStringLiteral("ð¡"), card); // 📡
    avatar->setFixedSize(40, 40);
    avatar->setAlignment(Qt::AlignCenter);
    avatar->setStyleSheet(QStringLiteral(
        "font-size:20px; background:%1; border-radius:20px; border:none;")
        .arg(QLatin1String(Theme::BG_RAISED())));
    row->addWidget(avatar);

    auto *textCol = new QVBoxLayout;
    textCol->setSpacing(2);
    auto *nameLabel = new QLabel(np.displayName, card);
    nameLabel->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:600; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_BASE).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    textCol->addWidget(nameLabel);
    auto *subLabel = new QLabel(np.id.value, card);
    subLabel->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; font-family:monospace; background:transparent; border:none;")
        .arg(Theme::FONT_XS).arg(QLatin1String(Theme::TEXT_DISABLED())));
    textCol->addWidget(subLabel);
    row->addLayout(textCol, 1);

    // Add button
    auto *addBtn = new QPushButton(tr("Add"), card);
    addBtn->setFixedSize(64, Theme::TOUCH_MIN);
    addBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background:%1; color:%2; border:1px solid %3;"
        "  border-radius:%4px; font-size:%5px; font-weight:600; }"
        "QPushButton:pressed { background:%6; }"
        "QPushButton:disabled { opacity:0.5; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::ACCENT()))
        .arg(QLatin1String(Theme::ACCENT_DIM()))
        .arg(Theme::RADIUS_SM)
        .arg(Theme::FONT_SM)
        .arg(QLatin1String(Theme::BG_HOVER())));
    const QString peerId = np.id.value;
    connect(addBtn, &QPushButton::clicked, this, [this, peerId, addBtn]() {
        emit nearbyPeerConnectRequested(peerId);
        addBtn->setText(QStringLiteral("\u2713")); // checkmark
        addBtn->setDisabled(true);
    });
    row->addWidget(addBtn);

    return card;
}

// V32.32: skeleton placeholder — see header for design rationale.
// Sized to ~72px tall to match a real contact card (avatar + 2 lines of
// text). No content beyond a uniform-grey rounded rectangle. Looks like
// "something is loading" without committing to a specific layout (which
// might mislead the user if the real card ends up different).
QWidget *ContactsPage::makeSkeletonCard()
{
    auto *card = new QWidget(m_cardContainer);
    card->setObjectName(QStringLiteral("skeletonCard"));
    card->setFixedHeight(72);
    card->setStyleSheet(QStringLiteral(
        "QWidget#skeletonCard {"
        "  background: %1;"
        "  border-radius: %2px;"
        "  border: 1px solid %3;"
        "}")
        .arg(QLatin1String(Theme::BG_CARD()))
        .arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::BORDER())));
    // Two faint inner bars suggesting "name" and "subtitle" lines.
    // Pure visual hint — both static, no animation.
    auto *lay = new QHBoxLayout(card);
    lay->setContentsMargins(Theme::SPACE_SM, Theme::SPACE_SM,
                             Theme::SPACE_SM, Theme::SPACE_SM);
    // Left "avatar" disc
    auto *disc = new QLabel(card);
    disc->setFixedSize(40, 40);
    disc->setStyleSheet(QStringLiteral(
        "background: %1; border-radius: 20px;")
        .arg(QLatin1String(Theme::BG_RAISED())));
    lay->addWidget(disc);
    // Right text bars (stacked)
    auto *textCol = new QVBoxLayout();
    textCol->setSpacing(6);
    auto *bar1 = new QLabel(card);
    bar1->setFixedHeight(12);
    bar1->setMinimumWidth(140);
    bar1->setStyleSheet(QStringLiteral(
        "background: %1; border-radius: 6px;")
        .arg(QLatin1String(Theme::BG_RAISED())));
    auto *bar2 = new QLabel(card);
    bar2->setFixedHeight(10);
    bar2->setMaximumWidth(90);
    bar2->setStyleSheet(QStringLiteral(
        "background: %1; border-radius: 5px;")
        .arg(QLatin1String(Theme::BG_RAISED())));
    textCol->addWidget(bar1);
    textCol->addWidget(bar2);
    textCol->addStretch();
    lay->addLayout(textCol, 1);
    return card;
}

// ─── Public API ──────────────────────────────────────────────────────────────

void ContactsPage::setContacts(const QList<Core::Contact> &contacts)
{
    m_contacts.clear();
    for (const Core::Contact &c : contacts)
        m_contacts[c.id.value] = c;
    // V32.32: mark first-load complete so rebuildCards stops showing
    // skeleton placeholders. Subsequent setContacts calls (e.g. after
    // a refresh) keep this flag true — the empty-state path takes over
    // if the contact list is genuinely empty.
    m_initialLoadDone = true;
    refreshConversationActivity();  // V32.34+: per-contact last-snap/unread
    rebuildCards();
}

void ContactsPage::refreshConversationActivityAndRebuild()
{
    // V32.34+: debounce. Multiple snap events in quick succession (offline
    // catch-up) would otherwise each trigger a full card rebuild. Coalesce
    // into a single refresh ~150ms after the last event. Lazily create the
    // timer so test harnesses that never call this don't allocate it.
    if (!m_activityRefreshTimer) {
        m_activityRefreshTimer = new QTimer(this);
        m_activityRefreshTimer->setSingleShot(true);
        m_activityRefreshTimer->setInterval(150);
        connect(m_activityRefreshTimer, &QTimer::timeout, this, [this]() {
            refreshConversationActivity();
            rebuildCards();
        });
    }
    m_activityRefreshTimer->start();  // restart on each call (debounce)
}

// V32.34+: pull per-contact conversation activity (last snap time + unread
// count) from storage so each card can read as a conversation summary, not
// just a presence row. Cheap: one query, results keyed by other-party id.
// Safe no-op when no controller/storage is wired yet (e.g. early construction
// or test harness) — m_convActivity just stays empty and cards omit the line.
void ContactsPage::refreshConversationActivity()
{
    m_convActivity.clear();
    if (!m_controller || !m_controller->storageManager()) return;
    const QString selfRaw = m_controller->stableId();
    if (selfRaw.isEmpty()) return;
    const Core::UserId selfId{ selfRaw };
    const auto convos =
        m_controller->storageManager()->loadConversations(selfId);
    for (const auto &s : convos) {
        ConvActivity a;
        a.lastSnapAt  = s.lastSnapAt;
        a.unreadCount = s.unreadCount;
        a.valid       = true;
        m_convActivity[s.otherParty.value] = a;
    }
}

void ContactsPage::addOrUpdateContact(const Core::Contact &contact)
{
    m_contacts[contact.id.value] = contact;
    // Try to update existing card in-place
    const QString cardName = QStringLiteral("contactCard_") + contact.id.value;
    if (QWidget *existing = m_cardContainer->findChild<QWidget *>(cardName)) {
        // Stop any spinner timer before deleting the card — the timer holds
        // a raw pointer to the status label inside the old card, so we must
        // stop it before deleteLater() frees that memory.
        const QString key = contact.id.value;
        if (QTimer *t = m_spinnerTimers.take(key)) {
            t->stop();
            t->deleteLater();
        }
        // Remove old card — rebuildCards() below will recreate it correctly.
        m_cardLayout->removeWidget(existing);
        existing->deleteLater();
        // rebuildCards() handles positioning, section sorting, and stat restoration.
        rebuildCards();
        return;
    }
    // New contact — do a full rebuild to place it in the correct section.
    rebuildCards();
}

// Restore the cached network stat string onto a card's netstat label (opacity 1, no fade).
void ContactsPage::restoreStatLabel(const QString &key)
{
    if (!m_peerStats.contains(key)) return;
    const QString name = QStringLiteral("netstat_") + key;
    if (!m_cardContainer) return;
    auto *lbl = m_cardContainer->findChild<QLabel *>(name);
    if (!lbl) return;
    lbl->setText(m_peerStats[key]);
    lbl->setVisible(true);
    if (auto *fx = qobject_cast<QGraphicsOpacityEffect *>(lbl->graphicsEffect()))
        fx->setOpacity(1.0);
}


void ContactsPage::markHasPendingSnap(const Core::UserId &id, bool pending)
{
    // V32.34+: delegate to the count-based pill. The old bool API maps to
    // "1 queued" / "none" so existing callers (if any) still work.
    setOutboundQueued(id, pending ? 1 : 0);
}

void ContactsPage::setOutboundQueued(const Core::UserId &id, int count)
{
    // Compact per-contact outbound pill ("⬆ N"), inserted just before the
    // online badge on the card's row. count==0 removes it. This is the
    // primary outbound surface now — the inbox no longer has a prominent
    // "Sent" block, so seeing "who has snaps going out" happens here, on
    // the contact overview, where it's most useful.
    const QString pillName = QStringLiteral("queued_") + id.value;
    if (!m_cardContainer) return;
    QLabel *pill = m_cardContainer->findChild<QLabel *>(pillName);

    if (count <= 0) {
        if (pill) pill->deleteLater();
        return;
    }

    const QString cardName = QStringLiteral("contactCard_") + id.value;
    QWidget *card = m_cardContainer->findChild<QWidget *>(cardName);
    if (!card) return;

    if (!pill) {
        pill = new QLabel(card);
        pill->setObjectName(pillName);
        pill->setToolTip(tr("Snaps queued for this contact — "
                            "will deliver when they're reachable"));
        auto *row = qobject_cast<QHBoxLayout *>(card->layout());
        // Insert before the badge (the last widget that matters in the row).
        // count()-2 mirrors markHasPendingSnap's original placement so the
        // pill sits left of the online dot.
        if (row) row->insertWidget(row->count() - 2, pill);
    }
    pill->setText(QStringLiteral("⬆ %1").arg(count));
    pill->setStyleSheet(QStringLiteral(
        "QLabel { background:%1; color:%2; border:1px solid %3;"
        " border-radius:%4px; padding:1px 8px; font-size:%5px;"
        " font-weight:700; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::WARNING()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::RADIUS_FULL).arg(Theme::FONT_XS));
}

static QString sanitizeStatText(const QString &s)
{
    QString out;
    out.reserve(s.size());
    for (const QChar c : s) {
        const uint cp = c.unicode();
        // Keep: printable ASCII, common Latin, basic punctuation, digits
        // Skip: control chars (< 32 except space), surrogates, PUA, specials
        if (cp == 0x20                       // space
            || (cp >= 0x21 && cp <= 0x7E)    // printable ASCII
            || (cp >= 0xA0 && cp <= 0x024F)  // Latin extended
            || (cp >= 0x2018 && cp <= 0x201E)// curly quotes
            || (cp >= 0x2190 && cp <= 0x21FF)// arrows (↑↓←→)
            || (cp >= 0x2600 && cp <= 0x26FF)// misc symbols (safe subset)
            || cp == 0x2026                  // ellipsis …
            || cp == 0x2013 || cp == 0x2014) // en/em dash
        {
            out.append(c);
        } else if (cp == 0x2713 || cp == 0x2714) {
            out.append(QLatin1Char('v')); // checkmark → v
        }
        // else: skip — renders as box or crashes on some fonts
    }
    return out.trimmed();
}

void ContactsPage::setConnectionStatus(const Core::UserId &id,
                                        const QString &phase,
                                        const QString &detail)
{
    const QString key = id.value;
    const QString safeDetail = sanitizeStatText(detail);
    (void)safeDetail; // used in setText calls below

    // Stop any existing spinner for this peer
    if (QTimer *old = m_spinnerTimers.take(key)) {
        old->stop();
        old->deleteLater();
    }

    // If this peer has no card yet (and we have a real status to show),
    // create a minimal placeholder so the spinner appears immediately.
    // Skip on clear (empty phase) — no point creating a card just to hide the label.
    if (!phase.isEmpty() && !m_contacts.contains(key)) {
        // Create a Pending placeholder card — but only if no card widget exists yet.
        // This avoids a full rebuildCards() if the card was already created by
        // a prior peerConnectionStatus call (which fires many times per connection).
        const QString cardName = QStringLiteral("contactCard_") + key;
        const bool cardExists = m_cardContainer
            && m_cardContainer->findChild<QWidget *>(cardName) != nullptr;
        if (!cardExists) {
            Core::Contact placeholder;
            placeholder.id          = id;
            placeholder.displayName = id.value;
            placeholder.online      = false;
            placeholder.state       = Core::ContactState::Pending;
            m_contacts[key] = placeholder;
            rebuildCards(); // only rebuild when card truly doesn't exist
        }
    }

    // Find the status label
    const QString statusName = QStringLiteral("status_") + key;
    if (!m_cardContainer) return;
    QLabel *statusLine = m_cardContainer->findChild<QLabel *>(statusName);
    if (!statusLine) return;

    if (phase.isEmpty()) {
        // Clear — hide the label
        statusLine->setVisible(false);
        statusLine->setText(QString());
        return;
    }

    // Choose color and icon prefix based on phase
    QString color;
    bool useSpinner = false;
    QString icon;
    if (phase == QLatin1String("connected")) {
        color  = QLatin1String(Theme::SUCCESS());
        icon   = QStringLiteral("✓ ");
        // Auto-clear after 3s
        QTimer::singleShot(Timings::kStatusFadeSuccessMs, statusLine, [statusLine]() {
            statusLine->setVisible(false);
        });
    } else if (phase == QLatin1String("failed")) {
        color      = QLatin1String(Theme::DANGER());
        icon       = QStringLiteral("✗ ");
        // Auto-clear after 8s
        QTimer::singleShot(Timings::kStatusFadeFailMs, statusLine, [statusLine]() {
            statusLine->setVisible(false);
        });
    } else {
        // Active phase — amber spinner
        color      = QLatin1String(Theme::WARNING());
        useSpinner = true;
        icon       = QStringLiteral("⠋ "); // initial frame
    }

    statusLine->setStyleSheet(QStringLiteral(
        "font-size:%1px; color:%2; background:transparent; border:none;")
        .arg(Theme::FONT_XS).arg(color));
    statusLine->setText(icon + safeDetail);
    statusLine->setVisible(true);

    if (useSpinner) {
        // Braille spinner frames — animate at ~8 fps
        static const char *kFrames[] = { "⠋ ", "⠙ ", "⠸ ", "⠴ ", "⠦ ", "⠇ " };
        constexpr int kNFrames = 6;
        auto *frame = new int(0);
        auto *timer = new QTimer(this);
        timer->setInterval(Timings::kSpinnerFrameMs);
        const QString detailCopy = safeDetail;
        const QString colorCopy  = color;
        connect(timer, &QTimer::timeout, statusLine,
                [statusLine, frame, detailCopy, colorCopy]() {
                    *frame = (*frame + 1) % kNFrames;
                    statusLine->setText(
                        QString::fromUtf8(kFrames[*frame]) + detailCopy);
                });
        connect(timer, &QTimer::destroyed, this,
                [frame]() { delete frame; });
        timer->start();
        m_spinnerTimers[key] = timer;
    }
}

// Strip characters that Qt cannot render cleanly on all platforms:
// control chars, private-use area, replacement char, surrogates, etc.
void ContactsPage::setNetworkStat(const Core::UserId &id, const QString &stat)
{
    const QString key = id.value;
    const QString clean = sanitizeStatText(stat);
    m_peerStats[key] = clean; // stored; timer will fade it in at next tick

    if (!m_cardContainer) return; // not yet built
    const QString name = QStringLiteral("netstat_") + key;
    auto *lbl = m_cardContainer->findChild<QLabel *>(name);
    if (!lbl) return;

    lbl->setText(clean);
    lbl->setVisible(true);

    auto *effect = qobject_cast<QGraphicsOpacityEffect *>(lbl->graphicsEffect());
    if (!effect) return;

    auto *anim = new QPropertyAnimation(effect, "opacity", lbl);
    anim->setDuration(400);
    anim->setStartValue(0.0);
    anim->setEndValue(1.0);
    anim->setEasingCurve(QEasingCurve::InOutQuad);
    anim->start(QAbstractAnimation::DeleteWhenStopped);
}

void ContactsPage::clearNetworkStat(const Core::UserId &id)
{
    const QString key = id.value;
    m_peerStats.remove(key);
    const QString name = QStringLiteral("netstat_") + key;
    if (auto *lbl = m_cardContainer->findChild<QLabel *>(name)) {
        lbl->setVisible(false);
        lbl->setText(QString());
        if (auto *fx = qobject_cast<QGraphicsOpacityEffect *>(lbl->graphicsEffect()))
            fx->setOpacity(0.0);
    }
}

void ContactsPage::markOnline(const Core::UserId &id, bool online)
{
    if (!m_contacts.contains(id.value)) return;
    const bool wasOnline = m_contacts[id.value].online;
    m_contacts[id.value].online = online;

    if (online != wasOnline) {
        // Section membership changed — full rebuild to move card between groups.
        rebuildCards();
    } else {
        // Same section — just update the badge in-place (cheaper).
        const QString badgeName = QStringLiteral("badge_") + id.value;
        if (QLabel *badge = m_cardContainer->findChild<QLabel *>(badgeName)) {
            badge->setText(online ? QStringLiteral("●") : QStringLiteral("○"));
            badge->setStyleSheet(QStringLiteral(
                "font-size:14px; color:%1; background:transparent; border:none;")
                .arg(online ? QLatin1String(Theme::SUCCESS())
                            : QLatin1String(Theme::TEXT_DISABLED())));
        }
    }
}

void ContactsPage::removeContact(const Core::UserId &id)
{
    m_contacts.remove(id.value);
    m_peerStats.remove(id.value);
    if (QTimer *t = m_spinnerTimers.take(id.value)) { t->stop(); t->deleteLater(); }
    // Full rebuild — removes the card and cleans up any now-empty section label.
    rebuildCards();
}

std::optional<Core::UserId> ContactsPage::selectedContact() const
{
    if (m_selectedId.isEmpty()) return std::nullopt;
    return Core::UserId{ m_selectedId };
}

QString ContactsPage::selectedContactName() const
{
    const auto id = selectedContact();
    if (!id.has_value()) return {};
    // m_contacts is a QHash<QString, Contact> keyed by stable ID
    const auto it = m_contacts.constFind(id->value);
    if (it != m_contacts.constEnd()) {
        const Core::Contact &c = it.value();
        return c.nickname.isEmpty() ? c.displayName : c.nickname;
    }
    return id->value;
}

// ─── Private ──────────────────────────────────────────────────────────────────

// ─── Section label factory ───────────────────────────────────────────────────
// Returns a container widget with the label + a 1px hairline separator beneath.
static QWidget *makeSectionLabel(const QString &text, QWidget *parent)
{
    auto *container = new QWidget(parent);
    container->setObjectName(QStringLiteral("sectionHeader"));
    auto *vlay = new QVBoxLayout(container);
    vlay->setContentsMargins(4, 10, 4, 0);
    vlay->setSpacing(4);

    auto *lbl = new QLabel(text.toUpper(), container);
    lbl->setObjectName(QStringLiteral("sectionLabel"));
    lbl->setStyleSheet(QStringLiteral(
        "font-size:10px; font-weight:700; letter-spacing:1.5px;"
        " color:%1; background:transparent; border:none;")
        .arg(QLatin1String(Theme::TEXT_DISABLED())));
    vlay->addWidget(lbl);

    // Hairline separator
    auto *line = new QFrame(container);
    line->setFrameShape(QFrame::HLine);
    line->setFixedHeight(1);
    line->setStyleSheet(QStringLiteral(
        "border:none; background:%1;").arg(QLatin1String(Theme::BORDER())));
    vlay->addWidget(line);

    return container;
}

void ContactsPage::applyCardFilterIndex(int idx)
{
    // V32.34+: shared by the SegmentedTabStrip and the SubTabbed override
    // (unified bar / swipe nav) so both drive m_cardFilter identically.
    m_cardFilter = (idx == 1) ? CardFilter::Online
                 : (idx == 2) ? CardFilter::Pending
                              : CardFilter::All;
    rebuildCards();
    // Keep the in-page strip's highlight in sync when the change came from
    // elsewhere (bar/swipe), guarding against a redundant re-emit.
    if (m_filterTabs && m_filterTabs->currentIndex() != idx)
        m_filterTabs->setCurrentIndex(idx);
}

QString ContactsPage::subTabLabel(int index) const
{
    switch (index) {
        case 0: return tr("All");
        case 1: return tr("Online");
        case 2: return tr("Pending");
        default: return QString();
    }
}

int ContactsPage::subTabBadge(int index) const
{
    // Online tab → count of online contacts; Pending tab → count of
    // ContactState::Pending. Matches rebuildCards' bucketing exactly so the
    // unified bar shows the same numbers the filtered view contains.
    if (index == 1) {
        int n = 0;
        for (const Core::Contact &c : m_contacts) if (c.online) ++n;
        return n;
    }
    if (index == 2) {
        int n = 0;
        for (const Core::Contact &c : m_contacts)
            if (c.state == Core::ContactState::Pending) ++n;
        return n;
    }
    return 0;
}

void ContactsPage::rebuildCards()
{
    // Stop all spinner timers before clearing cards to prevent use-after-free.
    // Timers hold raw QLabel* pointers into card widgets; if the timer fires
    // after deleteLater() schedules destruction, it writes to freed memory.
    for (QTimer *t : m_spinnerTimers) { t->stop(); t->deleteLater(); }
    m_spinnerTimers.clear();

    // Clear everything except the trailing stretch.
    QList<QWidget *> toRemove;
    for (int i = 0; i < m_cardLayout->count(); ++i) {
        QLayoutItem *item = m_cardLayout->itemAt(i);
        if (!item) continue;
        QWidget *w = item->widget();
        if (w && w != m_emptyLabel) toRemove.append(w);
    }
    for (QWidget *w : toRemove) { m_cardLayout->removeWidget(w); w->deleteLater(); }
    // The skeleton card pointers are now dangling (deleted via deleteLater).
    // Clear our tracking list so we don't try to remove them twice on the
    // next rebuild.
    m_skeletonCards.clear();

    // V32.32: pre-load skeleton state. When setContacts() has NEVER been
    // called, we don't know yet if the user has contacts or not — showing
    // either the empty-message OR an empty card area would be misleading.
    // Instead show 4 grey placeholders that read as "loading."
    if (!m_initialLoadDone) {
        // Hide the "No contacts yet" label during loading.
        if (m_emptyLabel) m_emptyLabel->setVisible(false);
        constexpr int kSkeletonCount = 4;
        for (int i = 0; i < kSkeletonCount; ++i) {
            QWidget *sk = makeSkeletonCard();
            // Insert before the trailing stretch (last item). If there's no
            // stretch (shouldn't happen, but defensive), append.
            int insertAt = m_cardLayout->count();
            if (insertAt > 0 && !m_cardLayout->itemAt(insertAt - 1)->widget()) {
                insertAt -= 1;
            }
            m_cardLayout->insertWidget(insertAt, sk);
            m_skeletonCards.append(sk);
        }
        return;
    }

    const QString filter = m_filterText.toLower();

    // Classify contacts into four buckets — order determines section order.
    QList<Core::Contact> online, pending, recent, contacts;
    for (const Core::Contact &c : m_contacts) {
        if (!filter.isEmpty()) {
            const bool match = c.label().toLower().contains(filter)
                || c.id.value.toLower().contains(filter)
                || c.notes.toLower().contains(filter);
            if (!match) continue;
        }
        if (c.online) {
            online.append(c);
        } else if (c.state == Core::ContactState::Pending) {
            pending.append(c);
        } else {
            // Known + offline: split by recency of lastSeen.
            const qint64 secsAgo = c.lastSeen.isValid()
                ? c.lastSeen.secsTo(QDateTime::currentDateTimeUtc()) : -1;
            if (secsAgo >= 0 && secsAgo < 7 * 86400)
                recent.append(c);
            else
                contacts.append(c);
        }
    }

    // Sort: online by label; pending by label; recent by lastSeen desc; contacts by label.
    auto byLabel = [](const Core::Contact &a, const Core::Contact &b) {
        return a.label().toLower() < b.label().toLower();
    };
    auto byLastSeen = [](const Core::Contact &a, const Core::Contact &b) {
        return a.lastSeen > b.lastSeen; // newest first
    };
    std::sort(online.begin(),   online.end(),   byLabel);
    std::sort(pending.begin(),  pending.end(),  byLabel);
    std::sort(recent.begin(),   recent.end(),   byLastSeen);
    std::sort(contacts.begin(), contacts.end(), byLabel);

    // V32.34+: live count badge on the "Online" tab (index 1), independent
    // of the active filter so the number is always meaningful.
    if (m_filterTabs) m_filterTabs->setBadge(1, static_cast<int>(online.size()));
    // V32.34+ unified nav: nudge the bottom bar to repaint its count pills
    // (it reads the actual count from subTabBadge()). Harmless when the top
    // strip is the active chrome instead.
    emit badgesChanged();

    // Insert before the stretch (last item).
    const int stretchIdx = m_cardLayout->count() - 1;
    int insertAt = stretchIdx; // grows upward as we insert
    int total = 0;

    auto addSection = [&](const QString &label, const QList<Core::Contact> &list) {
        if (list.isEmpty()) return;
        const QString hdr = QStringLiteral("%1  (%2)").arg(label).arg(list.size());
        auto *sectionLbl = makeSectionLabel(hdr, m_cardContainer);
        m_cardLayout->insertWidget(insertAt, sectionLbl);
        ++insertAt;
        for (const Core::Contact &c : list) {
            QWidget *card = makeCard(c);
            m_cardLayout->insertWidget(insertAt, card);
            ++insertAt;
            restoreStatLabel(c.id.value);
            restoreLocHint(c.id.value);
        }
        total += static_cast<int>(list.size());
    };

    // V32.34+: the active filter tab decides which sections show. "All"
    // shows everything (the long-standing behavior); "Online" shows only the
    // connected bucket; "Pending" shows only the connecting/unseen bucket.
    const bool showAll     = (m_cardFilter == CardFilter::All);
    const bool showOnline  = showAll || (m_cardFilter == CardFilter::Online);
    const bool showPending = showAll || (m_cardFilter == CardFilter::Pending);

    // Nearby section — LAN beacon peers not yet in contacts. Treated as a
    // form of "online/discoverable", so it shows under All + Online.
    if (!m_nearbyPeers.isEmpty() && (showAll || showOnline)) {
        const QString hdr = QStringLiteral("%1  (%2)")
            .arg(tr("Nearby")).arg(m_nearbyPeers.size());
        auto *sectionLbl = makeSectionLabel(hdr, m_cardContainer);
        m_cardLayout->insertWidget(insertAt, sectionLbl);
        ++insertAt;
        for (const NearbyPeer &np : m_nearbyPeers) {
            QWidget *card = makeNearbyCard(np);
            m_cardLayout->insertWidget(insertAt, card);
            ++insertAt;
        }
        total += static_cast<int>(m_nearbyPeers.size());
    }

    if (showOnline)  addSection(tr("Online"),     online);
    if (showPending) addSection(tr("Connecting"), pending);
    if (showAll)     addSection(tr("Recent"),     recent);
    if (showAll)     addSection(tr("Contacts"),   contacts);

    // V32.34+: filter-aware empty text. "All" with nothing → the standard
    // no-contacts message (already set on m_emptyLabel). A filter that hides
    // everything gets a filter-specific line so the blank list isn't
    // confusing.
    if (total == 0 && !showAll) {
        m_emptyLabel->setText(m_cardFilter == CardFilter::Online
            ? tr("No contacts online right now")
            : tr("No pending contacts"));
    } else if (total == 0) {
        m_emptyLabel->setText(
            tr("No contacts yet.\n\nAdd a peer below using their sc-ID\n"
               "or IP address."));
    }
    m_emptyLabel->setVisible(total == 0);
}

void ContactsPage::openEditor(const QString &id)
{
    qInfo(
           "openEditor called for id=%s (m_contacts has %d entries, contains=%d)",
           qPrintable(id), static_cast<int>(m_contacts.size()),
           m_contacts.contains(id) ? 1 : 0);

    // V32.34+: guard against stacked editors. If we already have one
    // open, just raise it rather than spawning another. QPointer
    // auto-nulls if the dialog was destroyed, so a stale pointer
    // doesn't block legitimate re-opens after a previous close.
    if (m_openEditor) {
        // V32.37+ regression diagnostic + self-heal: if editing "worked a couple
        // builds ago" and now shows nothing, the prime suspect is a STALE
        // m_openEditor that a close path failed to clear — every reopen then hits
        // this guard and "raises" a dialog that's already gone.
        const bool stillVisible = m_openEditor->isVisible();
        qInfo("openEditor: GUARD HIT — existing m_openEditor visible=%d parent=%s",
              stillVisible ? 1 : 0,
              m_openEditor->parentWidget()
                  ? m_openEditor->parentWidget()->metaObject()->className() : "null");
        if (stillVisible) {
            // A genuinely-open editor: raise it, don't stack a second.
            m_openEditor->raise();
            m_openEditor->activateWindow();
            return;
        }
        // NOT visible → the pointer is stale (a close path hid the dialog but
        // didn't clear the guard, e.g. finished() never reached us). Clear it and
        // fall through to open a FRESH editor instead of being permanently
        // blocked. Tear down the orphan so it doesn't leak.
        qInfo("openEditor: tracked dialog is not visible — treating as stale, "
              "clearing guard and opening fresh (self-heal)");
        QPointer<QDialog> orphan = m_openEditor;
        m_openEditor = nullptr;
        if (orphan) orphan->deleteLater();
    }
    if (!m_contacts.contains(id)) {
        qWarning(
                  "openEditor: m_contacts does NOT contain id=%s — bailing. "
                  "Available ids: %s", qPrintable(id),
                  qPrintable(QStringList(m_contacts.keys()).join(QStringLiteral(", "))));
        return;
    }
    // Snapshot the contact now — rebuildCards() may run during exec() and
    // delete/recreate cards, but m_contacts is stable.
    const Core::Contact snapshot = m_contacts.value(id);

    // V32.32 (revision 2): parent the dialog to the TOP-LEVEL WINDOW,
    // not to ContactsPage. Why: ContactsPage is buried inside the
    // SwipeNavigator's page stack, which itself lives inside a wrapper
    // widget that hosts AmbientBlobs as a raised overlay. A dialog
    // parented to ContactsPage z-orders within that subtree — possibly
    // BELOW the blobs layer or other raised siblings. The log showed
    // visible=1 but the user couldn't see the dialog — classic
    // z-order-under-overlay symptom.
    //
    // Top-level parent gives the dialog its own toplevel Qt window
    // (still modal to the application), which floats above all the
    // in-window layering. The visual effect is exactly what the user
    // expects: a modal sheet that occludes the page beneath it.
    QWidget *topLevel = window();  // walks up to the top-level QWidget
    auto *dlg = new ContactEditorDialog(snapshot, topLevel ? topLevel : this);
    m_openEditor = dlg;  // V32.34+: track for the stacked-dialog guard.
                         // QPointer auto-nulls when dlg is destroyed.
    // Do NOT use WA_DeleteOnClose — exec() processes pending events including
    // deleteLater() calls. If the card that spawned this dialog gets deleted
    // mid-exec, we need the dialog to outlive the event. Delete manually below.
    connect(dlg, &ContactEditorDialog::deleteRequested,
            this, [this](const Core::UserId &uid) {
                removeContact(uid);
                emit contactDeleted(uid);
            });

    // V32.35+ EDIT-CONTACT FIX: persist the edit on Save. The dialog is opened
    // non-blocking with open() (not exec()), so the documented "call
    // editedContact() after exec()" pattern never ran — onSave() updated the
    // dialog's internal m_contact and accept()ed, but NOTHING read it back, so
    // a renamed contact silently reverted on close. QDialog::accept() emits
    // accepted(); here we read editedContact(), update the in-memory map, and
    // emit contactEdited — which MainWindow already routes to
    // StorageManager::saveContact (the persist plumbing existed; only this
    // emission was missing). rebuildCards() refreshes the visible label.
    {
        QPointer<ContactEditorDialog> dlgPtr(dlg);
        connect(dlg, &QDialog::accepted, this, [this, dlgPtr]() {
            if (!dlgPtr) return;
            const Core::Contact edited = dlgPtr->editedContact();
            if (edited.id.value.isEmpty()) return;   // defensive
            m_contacts.insert(edited.id.value, edited);
            emit contactEdited(edited);
            rebuildCards();
        });
    }

    // V32.37 FIX (dialog-not-visible): TRUST MobileDialog's sheet machinery.
    //
    // ContactEditorDialog IS a MobileDialog — an IN-WINDOW sheet: it parents a
    // scrim to the top-level window and slides itself up from the bottom edge as
    // a CHILD widget (pos animates (0,root.h) → (0,root.h-dialog.h)). Its open()
    // already runs animateSheetIn() (scrim show+raise, slide, stuck-timer) then
    // QDialog::open() for non-blocking events.
    //
    // The old V32.32 "belt-and-braces" here was actively BREAKING that and is
    // the reason the dialog wasn't visible on open:
    //   • setWindowFlag(WindowStaysOnTopHint) forced the dialog to become a
    //     SEPARATE top-level OS window — so the in-window scrim sheet model fell
    //     apart: the scrim painted inside the main window while the dialog
    //     floated off as its own surface (on Plasma Mobile, frequently spawning
    //     behind / off-screen — the exact symptom the flag was meant to cure,
    //     made worse).
    //   • the manual move() to a "centered" point fought animateSheetIn's pos
    //     animation (one clobbered the other depending on event timing).
    //   • the extra show()/raise()/activateWindow() after open() raced the
    //     animation and, on a child-widget sheet, are meaningless.
    // Removing all of it lets the sheet present the way every other MobileDialog
    // in the app already does (which the user sees working elsewhere).
    dlg->open();
    // V32.36+ FIX: clean up on ACTUAL close, not immediately. The block here
    // used to be a leftover from the exec() era:
    //     const int result = QDialog::Rejected;   // hardcoded
    //     ... dlg->deleteLater();                  // ran right after open()
    // With exec() (blocking) that ran when the dialog closed. After the switch
    // to open() (non-blocking), openEditor returns immediately — so that
    // deleteLater() was scheduled on the VERY NEXT event-loop pass, destroying
    // the dialog before the user could see or touch it. That's the "edit button
    // shows no UI" symptom: open() made it visible, then it was torn down a
    // millisecond later. It also meant m_openEditor was never cleared on a real
    // close (no finished hookup), so any path that DIDN'T hit the immediate
    // delete would leave the guard stuck and silently "raise" a dead dialog.
    //
    // Correct lifecycle: delete on finished() and clear the guard there, so the
    // dialog lives for the whole interaction and the next open works cleanly.
    connect(dlg, &QDialog::finished, this, [this, dlg](int /*result*/) {
        // Acceptance is already handled by the accepted() connection above
        // (reads editedContact, persists, rebuilds). Here we only tear down.
        m_openEditor = nullptr;           // clear the stacked-dialog guard
        dlg->deleteLater();               // safe now — interaction is over
    });
}

void ContactsPage::hideEvent(QHideEvent *e)
{
    // V32.34+: collapse any expanded contact card when the page is hidden
    // (swiped to another page / app backgrounded) so it doesn't linger open or
    // reappear expanded on return. QPointer guards a concurrently-deleted card.
    if (m_openCard) m_openCard->collapse();
    QWidget::hideEvent(e);
}

bool ContactsPage::eventFilter(QObject *obj, QEvent *ev)
{
    // Tap-to-set-sender removed (V32.15): this used to set m_selectedId and
    // emit contactSelected so the camera page knew who to send to. Problem
    // was that with multiple ongoing conversations users would forget who
    // was currently "active" — and there was no visual selection ring on
    // the card to remind them. The fix: the send path now ALWAYS shows a
    // ContactPickerDialog after capture (see MainWindow's snapCaptured
    // handler), so there's no need for a sticky "current contact" state.
    //
    // The eventFilter is still installed on cards because we may want
    // tap-to-open-detail or long-press-to-edit later. For now, all card
    // taps are ignored except for child QPushButtons (which still
    // bubble their own clicked() signals — edit pencil, location share,
    // etc.).
    //
    // m_selectedId is preserved on the class for backward-compat with
    // external callers (e.g. ContactsPage::setSelectedContact, used by
    // MainWindow's "open contact from inbox" path), but no longer
    // written from card taps here.
    Q_UNUSED(obj);
    Q_UNUSED(ev);
    return QWidget::eventFilter(obj, ev);
}

void ContactsPage::onAddClicked()
{
    const QString input = m_hostEdit->text().trimmed();
    if (input.isEmpty()) return;
    if (input.startsWith(QStringLiteral("sc-"))) {
        emit addFriendRequested(input, 0);
    } else if (input.contains(QLatin1Char(':'))) {
        const QStringList parts = input.split(QLatin1Char(':'));
        bool ipPortOk = false;
        const quint16 port = static_cast<quint16>(parts.last().toUShort(&ipPortOk));
        if (!parts.first().isEmpty() && ipPortOk && port > 0)
            emit addFriendRequested(parts.first(), port);
    } else {
        bool fieldPortOk = false;
        const quint16 port = static_cast<quint16>(m_portEdit->text().toUShort(&fieldPortOk));
        if (fieldPortOk && port > 0) emit addFriendRequested(input, port);
    }
    m_hostEdit->clear();
    m_portEdit->setText(QStringLiteral("9876")); // Bug 12 fix: reset to default
}

void ContactsPage::onImportVCard()
{
    const QString path = QFileDialog::getOpenFileName(
        this, tr("Import vCard"), {},
        tr("vCard files (*.vcf *.vcard)"));
    if (path.isEmpty()) return;
    QFile f(path);
    if (!f.open(QIODevice::ReadOnly | QIODevice::Text)) return;
    const QList<Core::Contact> imported =
        Core::VCardIO::importVCard(QString::fromUtf8(f.readAll()));
    for (const Core::Contact &c : imported) {
        addOrUpdateContact(c);
        emit contactEdited(c);
    }
    emit importVCardRequested();
}

void ContactsPage::onFilterChanged(const QString &text)
{
    m_filterText = text;
    rebuildCards();
}

void ContactsPage::setLastLocation(const Core::UserId &id,
                                    const Core::PeerLocation &loc)
{
    // Cache so we can restore after rebuildCards() recreates cards
    if (loc.isValid())
        m_peerLocations[id.value] = loc;
    else
        m_peerLocations.remove(id.value);

    if (!m_cardContainer) return;
    restoreLocHint(id.value);
}

void ContactsPage::restoreLocHint(const QString &key)
{
    const QString name = QStringLiteral("lochint_") + key;
    auto *lbl = m_cardContainer ? m_cardContainer->findChild<QLabel *>(name) : nullptr;
    if (!lbl) return;

    const auto it = m_peerLocations.constFind(key);
    if (it == m_peerLocations.constEnd() || !it->isValid()) {
        lbl->setVisible(false);
        return;
    }
    lbl->setText(QStringLiteral("\U0001F4CD last seen %1").arg(it->ageString()));
    lbl->setVisible(true);
}


void ContactsPage::setSelectedContact(const Core::UserId &id)
{
    // Store selection and emit — the list widget will highlight it on the next
    // repaint if the contact exists. m_selectedId is a QString (the stable ID).
    m_selectedId = id.value;
    if (m_contacts.contains(id.value))
        emit contactSelected(id);
}

// V32.31: landmark creation entry point. Opens AddLandmarkDialog with
// the current best-effort location (lastKnownLocation for self), saves
// the resulting Landmark via storageManager. All landmarks created
// here are LandmarkScope::Private — DHT publish path will land in a
// later release after the privacy review.
void ContactsPage::onAddLandmarkClicked()
{
    if (!m_controller) {
        // Defensive: shouldn't happen at runtime (MainWindow always sets
        // the controller), but a test instance might skip it.
        return;
    }
    const Core::UserId selfId{ m_controller->stableId() };
    // Pull current location via the same lookup the snap-attach path
    // uses. Returns nullopt when no GPS fix and no manual override.
    const auto loc = m_controller->lastKnownLocation(selfId);
    const double lat = loc ? loc->lat       : qQNaN();
    const double lon = loc ? loc->lon       : qQNaN();
    const double acc = loc ? loc->accuracyM : 0.0;

    auto *dialog = new AddLandmarkDialog(selfId, lat, lon, acc, this);
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    connect(dialog, &AddLandmarkDialog::saved,
            this, [this](Core::Landmark lm) {
        if (!m_controller || !m_controller->storageManager()) return;
        if (m_controller->storageManager()->saveLandmark(lm)) {
            // V32.31: no toast / list refresh yet. The landmarks list
            // UI itself is a Phase 2 deliverable. For now the user has
            // confirmation via the dialog accepting + closing.
        }
    });
    dialog->exec();
}

} // namespace SnapClone::UI
