#include "ui/ComponentTestWindow.hpp"
#include "ui/LoginWidget.hpp"
#include "ui/InboxWidget.hpp"
#include "ui/CameraWidget.hpp"
#include "ui/FriendListWidget.hpp"
#include "ui/SwipeNavigator.hpp"
#include "ui/Theme.hpp"

#include <QTabWidget>
#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QLabel>
#include <QPushButton>
#include <QGroupBox>
#include <QFrame>
#include <QScrollArea>
#include <QToolBar>
#include <QStatusBar>
#include <QDateTime>
#include <QUuid>

// ─── UI/ComponentTestWindow.cpp ───────────────────────────────────────────────
//
// Each tab builds its component in isolation — no AppController required.
// Synthetic data is injected via the control buttons below each tab.
// ─────────────────────────────────────────────────────────────────────────────

namespace SnapClone::UI {

// ─── Construction ─────────────────────────────────────────────────────────────

ComponentTestWindow::ComponentTestWindow(QWidget *parent)
    : QMainWindow(parent)
{
    setWindowTitle(QStringLiteral("Component Test Harness — SnapClone"));
    setMinimumSize(420, 700);
    resize(420, 750);
    setStyleSheet(Theme::globalStyleSheet());

    auto *toolbar = addToolBar(QStringLiteral("Info"));
    toolbar->setMovable(false);
    auto *infoLabel = new QLabel(
        tr("  Development tool — components receive synthetic data"), toolbar);
    infoLabel->setStyleSheet(QStringLiteral("color: #C8B89A; font-size: 12px;"));
    toolbar->addWidget(infoLabel);

    m_tabs = new QTabWidget(this);
    m_tabs->setTabPosition(QTabWidget::South);
    m_tabs->setStyleSheet(QStringLiteral(R"(
        QTabWidget::pane  { border: none; }
        QTabBar::tab      { background: #181818; color: #8C8884;
                            padding: 10px 18px; }
        QTabBar::tab:selected { background: #0E0E0E; color: #E8E4DF;
                                border-top: 2px solid #C8B89A; }
        QTabBar::tab:hover    { color: #E8E4DF; }
    )"));

    buildTabs();
    setCentralWidget(m_tabs);
    statusBar()->showMessage(tr("Select a tab to inspect a component."));
}

void ComponentTestWindow::buildTabs()
{
    m_tabs->addTab(buildLoginTab(),        QStringLiteral("Login"));
    m_tabs->addTab(buildInboxTab(),        QStringLiteral("Inbox"));
    m_tabs->addTab(buildCameraTab(),       QStringLiteral("Camera"));
    m_tabs->addTab(buildFriendsTab(),      QStringLiteral("Friends"));
    m_tabs->addTab(buildSwipeTab(),        QStringLiteral("Swipe"));
    m_tabs->addTab(buildThemeGalleryTab(), QStringLiteral("Theme"));
}

// ─── Synthetic data ───────────────────────────────────────────────────────────

QList<Core::Snap> ComponentTestWindow::makeSyntheticSnaps(int count)
{
    QList<Core::Snap> snaps;
    const QStringList senders = {
        QStringLiteral("alice@192.168.1.10"),
        QStringLiteral("bob@10.0.0.5"),
        QStringLiteral("very-long-username-that-overflows@172.16.0.1"),
    };
    for (int i = 0; i < count; ++i) {
        Core::Snap s;
        s.id          = Core::SnapId{ QUuid::createUuid().toString(QUuid::WithoutBraces) };
        s.senderId    = Core::UserId{ senders[i % static_cast<int>(senders.size())] };
        s.recipientId = Core::UserId{ QStringLiteral("self") };
        s.mediaType   = Core::MediaType::Image;
        s.sentAt      = QDateTime::currentDateTimeUtc().addSecs(-i * 60);
        s.expiresAt   = s.sentAt.addSecs(24 * 3600);
        s.viewed      = (i % 3 == 0);
        s.durationSecs = 5 + i;
        snaps.append(s);
    }
    return snaps;
}

// ─── Login tab ────────────────────────────────────────────────────────────────

QWidget *ComponentTestWindow::buildLoginTab()
{
    auto *container = new QWidget;
    auto *root      = new QVBoxLayout(container);
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);

    auto *widget = new LoginWidget(container);
    root->addWidget(widget, 1);

    auto *clearBtn = new QPushButton(tr("Clear error"),    container);
    auto *errorBtn = new QPushButton(tr("Short error"),    container);
    auto *longBtn  = new QPushButton(tr("Long error"),     container);

    connect(clearBtn, &QPushButton::clicked, widget, &LoginWidget::clearError);
    connect(errorBtn, &QPushButton::clicked, widget, [widget]() {
        widget->showError(QStringLiteral("Incorrect password. Please try again."));
    });
    connect(longBtn, &QPushButton::clicked, widget, [widget]() {
        widget->showError(QStringLiteral(
            "Network error: Unable to verify credentials. "
            "Check your connection and try again. Error code: ECONNREFUSED (111)"));
    });

    auto *btnRow = new QHBoxLayout;
    btnRow->setContentsMargins(12, 8, 12, 12);
    btnRow->addWidget(clearBtn);
    btnRow->addWidget(errorBtn);
    btnRow->addWidget(longBtn);
    root->addLayout(btnRow);
    return container;
}

// ─── Inbox tab ────────────────────────────────────────────────────────────────

QWidget *ComponentTestWindow::buildInboxTab()
{
    auto *container = new QWidget;
    auto *root      = new QVBoxLayout(container);
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);

    auto *widget = new InboxWidget(container);
    root->addWidget(widget, 1);

    for (const Core::Snap &s : makeSyntheticSnaps(5)) {
        widget->addSnap(s);
    }

    auto *addBtn   = new QPushButton(tr("+ Add snap"), container);
    auto *clearBtn = new QPushButton(tr("Clear"),      container);
    auto *floodBtn = new QPushButton(tr("Flood (50)"), container);

    static int snapCounter = 6;
    connect(addBtn, &QPushButton::clicked, widget, [widget]() {
        Core::Snap s;
        s.id       = Core::SnapId{ QUuid::createUuid().toString(QUuid::WithoutBraces) };
        s.senderId = Core::UserId{ QStringLiteral("test@%1").arg(snapCounter++) };
        s.sentAt   = QDateTime::currentDateTimeUtc();
        s.expiresAt = s.sentAt.addSecs(3600);
        widget->addSnap(s);
    });
    connect(clearBtn, &QPushButton::clicked, widget, &InboxWidget::clearSnaps);
    connect(floodBtn, &QPushButton::clicked, widget, [widget]() {
        for (const Core::Snap &s : makeSyntheticSnaps(50)) {
            widget->addSnap(s);
        }
    });

    auto *btnRow = new QHBoxLayout;
    btnRow->setContentsMargins(12, 8, 12, 12);
    btnRow->addWidget(addBtn);
    btnRow->addWidget(clearBtn);
    btnRow->addWidget(floodBtn);
    root->addLayout(btnRow);
    return container;
}

// ─── Camera tab ───────────────────────────────────────────────────────────────

QWidget *ComponentTestWindow::buildCameraTab()
{
    auto *container = new QWidget;
    auto *root      = new QVBoxLayout(container);
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);

    auto *widget   = new CameraWidget(container);
    root->addWidget(widget, 1);

    auto *startBtn = new QPushButton(tr("Start camera"), container);
    auto *stopBtn  = new QPushButton(tr("Stop camera"),  container);

    connect(startBtn, &QPushButton::clicked, widget, &CameraWidget::startCamera);
    connect(stopBtn,  &QPushButton::clicked, widget, &CameraWidget::stopCamera);

    // Log captured image dimensions in the status bar.
    connect(widget, &CameraWidget::snapCaptured,
            this, [this](const QImage &img) {
                statusBar()->showMessage(
                    tr("Captured: %1 × %2 px").arg(img.width()).arg(img.height()), 3000);
            });

    auto *btnRow = new QHBoxLayout;
    btnRow->setContentsMargins(12, 8, 12, 12);
    btnRow->addWidget(startBtn);
    btnRow->addWidget(stopBtn);
    root->addLayout(btnRow);
    return container;
}

// ─── Friends tab ──────────────────────────────────────────────────────────────

QWidget *ComponentTestWindow::buildFriendsTab()
{
    auto *container = new QWidget;
    auto *root      = new QVBoxLayout(container);
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);

    auto *widget = new FriendListWidget(container);

    const QList<Core::UserProfile> friends = {
        { Core::UserId{ QStringLiteral("alice@192.168.1.10") },
          QStringLiteral("alice@192.168.1.10"), QString{}, true  },
        { Core::UserId{ QStringLiteral("bob@10.0.0.5") },
          QStringLiteral("bob@10.0.0.5"),       QString{}, false },
        { Core::UserId{ QStringLiteral("carol@172.16.0.20") },
          QStringLiteral("carol@172.16.0.20"),  QString{}, true  },
    };
    widget->setFriends(friends);
    root->addWidget(widget, 1);

    auto *toggleBtn = new QPushButton(tr("Toggle bob online/offline"), container);
    static bool bobOnline = false;
    connect(toggleBtn, &QPushButton::clicked, widget, [widget]() {
        bobOnline = !bobOnline;
        widget->markOnline(Core::UserId{ QStringLiteral("bob@10.0.0.5") }, bobOnline);
    });

    auto *btnRow = new QHBoxLayout;
    btnRow->setContentsMargins(12, 8, 12, 12);
    btnRow->addWidget(toggleBtn);
    root->addLayout(btnRow);
    return container;
}

// ─── Swipe tab ────────────────────────────────────────────────────────────────

QWidget *ComponentTestWindow::buildSwipeTab()
{
    auto *container = new QWidget;
    auto *root      = new QVBoxLayout(container);
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);

    auto *nav = new SwipeNavigator(container);

    const QList<QPair<QString, QString>> pages = {
        { QStringLiteral("#C8B89A"), QStringLiteral("Page 0\n(swipe left →)") },
        { QStringLiteral("#6A9E78"), QStringLiteral("Page 1") },
        { QStringLiteral("#7A6E60"), QStringLiteral("Page 2") },
        { QStringLiteral("#C0614A"), QStringLiteral("Page 3\n(← swipe right)") },
    };
    for (const auto &[colour, text] : pages) {
        auto *page  = new QWidget(nav);
        page->setStyleSheet(QStringLiteral("background: %1;").arg(colour));
        auto *label = new QLabel(text, page);
        label->setAlignment(Qt::AlignCenter);
        label->setStyleSheet(QStringLiteral(
            "color: #0E0E0E; font-size: 20px; font-weight: 600; background: transparent;"));
        auto *l = new QVBoxLayout(page);
        l->addWidget(label);
        nav->addPage(page, text);
    }
    root->addWidget(nav, 1);

    auto *prevBtn = new QPushButton(tr("← Prev"), container);
    auto *nextBtn = new QPushButton(tr("Next →"), container);
    auto *info    = new QLabel(tr("Current page: 0"), container);
    info->setAlignment(Qt::AlignCenter);
    info->setStyleSheet(QStringLiteral("color: #8C8884; font-size: 12px;"));

    connect(prevBtn, &QPushButton::clicked, nav, [nav]() {
        nav->goToPage(nav->currentIndex() - 1);
    });
    connect(nextBtn, &QPushButton::clicked, nav, [nav]() {
        nav->goToPage(nav->currentIndex() + 1);
    });
    connect(nav, &SwipeNavigator::pageChanged, info, [info](int idx) {
        info->setText(QStringLiteral("Current page: %1").arg(idx));
    });

    auto *btnRow = new QHBoxLayout;
    btnRow->setContentsMargins(12, 8, 12, 12);
    btnRow->addWidget(prevBtn);
    btnRow->addWidget(info, 1);
    btnRow->addWidget(nextBtn);
    root->addLayout(btnRow);
    return container;
}

// ─── Theme gallery tab ────────────────────────────────────────────────────────

QWidget *ComponentTestWindow::buildThemeGalleryTab()
{
    auto *scroll = new QScrollArea;
    scroll->setWidgetResizable(true);
    scroll->setFrameShape(QFrame::NoFrame);

    auto *container = new QWidget;
    auto *root      = new QVBoxLayout(container);
    root->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                              Theme::SPACE_MD, Theme::SPACE_MD);
    root->setSpacing(Theme::SPACE_MD);

    // ── Section heading helper ────────────────────────────────────────────
    auto section = [&](const QString &name) {
        auto *lbl = new QLabel(name, container);
        lbl->setStyleSheet(QStringLiteral(
            "color: #8C8884; font-size: 11px; font-weight: 600; "
            "letter-spacing: 1px; background: transparent;"));
        root->addWidget(lbl);
    };

    // ── Colour swatch helper ──────────────────────────────────────────────
    auto swatch = [&](const QString &name, const char *colour) {
        auto *row  = new QHBoxLayout;
        auto *chip = new QFrame(container);
        chip->setFixedSize(40, 24);
        chip->setStyleSheet(
            QStringLiteral("background: %1; border-radius: 6px;").arg(QLatin1String(colour)));
        auto *lbl = new QLabel(
            QStringLiteral("%1  %2").arg(name, QLatin1String(colour)), container);
        lbl->setStyleSheet(QStringLiteral(
            "color: #E8E4DF; font-size: 13px; background: transparent;"));
        lbl->setTextInteractionFlags(Qt::TextSelectableByMouse);
        row->addWidget(chip);
        row->addWidget(lbl);
        row->addStretch();
        root->addLayout(row);
    };

    // ── Colour palette ────────────────────────────────────────────────────
    section(QStringLiteral("COLOUR PALETTE"));
    swatch(QStringLiteral("BG_DEEP"),    Theme::BG_DEEP());
    swatch(QStringLiteral("BG_CARD"),    Theme::BG_CARD());
    swatch(QStringLiteral("BG_RAISED"),  Theme::BG_RAISED());
    swatch(QStringLiteral("ACCENT"),     Theme::ACCENT());
    swatch(QStringLiteral("ACCENT_DIM"), Theme::ACCENT_DIM());
    swatch(QStringLiteral("DANGER"),     Theme::DANGER());
    swatch(QStringLiteral("SUCCESS"),    Theme::SUCCESS());

    // ── Buttons ───────────────────────────────────────────────────────────
    section(QStringLiteral("BUTTONS"));
    auto addBtn = [&](const QString &label, const QString &objName) {
        auto *btn = new QPushButton(label, container);
        if (!objName.isEmpty()) btn->setObjectName(objName);
        root->addWidget(btn);
    };
    addBtn(QStringLiteral("Default Button"),       QString{});
    addBtn(QStringLiteral("Accent Button"),        QStringLiteral("accentBtn"));
    addBtn(QStringLiteral("Danger / Destructive"), QStringLiteral("dangerBtn"));
    addBtn(QStringLiteral("📷  Icon Button"),      QStringLiteral("iconBtn"));

    auto *disabled = new QPushButton(QStringLiteral("Disabled"), container);
    disabled->setEnabled(false);
    root->addWidget(disabled);

    // ── Typography ────────────────────────────────────────────────────────
    section(QStringLiteral("TYPOGRAPHY"));
    const QList<QPair<int, QString>> sizes = {
        { Theme::FONT_2XL, QStringLiteral("2XL — 28px Display")  },
        { Theme::FONT_XL,  QStringLiteral("XL  — 22px Title")    },
        { Theme::FONT_LG,  QStringLiteral("LG  — 18px Heading")  },
        { Theme::FONT_BASE,QStringLiteral("Base — 15px Body")    },
        { Theme::FONT_SM,  QStringLiteral("SM  — 13px Caption")  },
        { Theme::FONT_XS,  QStringLiteral("XS  — 11px Label")    },
    };
    for (const auto &[size, text] : sizes) {
        auto *lbl = new QLabel(text, container);
        lbl->setStyleSheet(QStringLiteral(
            "color: #E8E4DF; font-size: %1px; background: transparent;").arg(size));
        root->addWidget(lbl);
    }

    root->addStretch();
    scroll->setWidget(container);
    return scroll;
}

} // namespace SnapClone::UI
