#include "ui/HelpManualDialog.hpp"
#include "ui/Theme.hpp"

#include <QTextBrowser>
#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QPushButton>
#include <QLabel>
#include <QTimer>
#include <QScrollBar>
#include <QEvent>
#include <QStandardPaths>
#include <QGuiApplication>
#include <QScreen>

namespace SnapClone::UI {

namespace {
// How long the manual sits open with no scrolling/interaction before it
// dismisses itself. Generous, because reading takes time — and any scroll
// resets it, so an active reader never hits it.
constexpr int kIdleDismissMs = 30000;  // 30s
}

HelpManualDialog::HelpManualDialog(QWidget *parent)
    : QDialog(parent)
{
    setWindowTitle(tr("User manual"));
    setModal(true);

    // Fill most of a phone screen; cap to a comfortable reading width on desktop.
    if (auto *scr = QGuiApplication::primaryScreen()) {
        const QSize avail = scr->availableSize();
        resize(qMin(avail.width(), 540),
               qMin(avail.height() - 40, 920));
    }
    setStyleSheet(QStringLiteral("QDialog { background:%1; }")
                      .arg(QLatin1String(Theme::BG_BASE())));

    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                             Theme::SPACE_MD, Theme::SPACE_MD);
    root->setSpacing(Theme::SPACE_SM);

    // Header: title + Close.
    auto *headerRow = new QHBoxLayout;
    auto *title = new QLabel(tr("SnapClone manual"), this);
    title->setStyleSheet(QStringLiteral(
        "font-size:%1px; font-weight:700; color:%2; background:transparent;")
        .arg(Theme::FONT_LG).arg(QLatin1String(Theme::TEXT_PRIMARY())));
    headerRow->addWidget(title);
    headerRow->addStretch();

    auto *closeBtn = new QPushButton(tr("Close"), this);
    closeBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background:%1; color:%2; border:1px solid %3;"
        " border-radius:%4px; padding: 6px 14px; font-size:%5px; }"
        "QPushButton:pressed { background:%6; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::RADIUS_SM).arg(Theme::FONT_BASE)
        .arg(QLatin1String(Theme::BG_HOVER())));
    connect(closeBtn, &QPushButton::clicked, this, &QDialog::accept);
    headerRow->addWidget(closeBtn);
    root->addLayout(headerRow);

    // Body — the manual itself.
    m_view = new QTextBrowser(this);
    m_view->setOpenExternalLinks(false);
    m_view->setMarkdown(helpManualMarkdown());
    m_view->setStyleSheet(QStringLiteral(
        "QTextBrowser { background:%1; color:%2; border:1px solid %3;"
        " border-radius:%4px; padding:%5px; font-size:%6px; }")
        .arg(QLatin1String(Theme::BG_CARD()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::RADIUS_MD).arg(Theme::SPACE_SM).arg(Theme::FONT_BASE));
    root->addWidget(m_view, 1);

    // Idle auto-dismiss whose countdown resets on scroll / interaction.
    m_idle = new QTimer(this);
    m_idle->setSingleShot(true);
    m_idle->setInterval(kIdleDismissMs);
    connect(m_idle, &QTimer::timeout, this, &QDialog::accept);

    // The explicit ask: scrolling resets the timeout. Catch every form of
    // scroll — drag, wheel, keyboard, or scrollbar — via the scrollbar's value.
    connect(m_view->verticalScrollBar(), &QScrollBar::valueChanged,
            this, [this](int) { resetIdleTimer(); });
    // Also treat direct interaction as activity so a reader who pauses on a
    // short section that doesn't move the scrollbar isn't dismissed mid-read.
    m_view->viewport()->installEventFilter(this);
    m_view->installEventFilter(this);

    resetIdleTimer();
}

void HelpManualDialog::resetIdleTimer()
{
    if (m_idle) m_idle->start();  // restart the single-shot countdown
}

bool HelpManualDialog::eventFilter(QObject *obj, QEvent *event)
{
    switch (event->type()) {
    case QEvent::Wheel:
    case QEvent::MouseButtonPress:
    case QEvent::MouseMove:
    case QEvent::TouchBegin:
    case QEvent::TouchUpdate:
    case QEvent::KeyPress:
        resetIdleTimer();
        break;
    default:
        break;
    }
    return QDialog::eventFilter(obj, event);
}

QString helpManualMarkdown()
{
    const QString media = QStandardPaths::writableLocation(
                              QStandardPaths::AppDataLocation)
                          + QStringLiteral("/media");
    const QString saved = QStandardPaths::writableLocation(
                              QStandardPaths::PicturesLocation)
                          + QStringLiteral("/SnapClone");
    const QString exports = QStandardPaths::writableLocation(
                              QStandardPaths::DownloadLocation);
    const QString db = QStandardPaths::writableLocation(
                              QStandardPaths::AppDataLocation)
                       + QStringLiteral("/db/snapclone.sqlite");

    return QStringLiteral(
"# SnapClone\n\n"
"A private, serverless messenger for sharing **snaps** — photos and short "
"videos meant to be seen and then let go. There are no accounts and no central "
"server: your messages travel peer-to-peer, directly between devices.\n\n"

"## Getting around\n\n"
"Swipe left and right between the five pages:\n\n"
"- **Camera** — take a photo, or hold the shutter to record a short video, then "
"send it to a friend.\n"
"- **Inbox** — snaps people have sent you, grouped by conversation. Tap one to "
"view it.\n"
"- **Friends** — the people you've added; add someone to start sharing.\n"
"- **Wayfinder** — find and connect with people nearby or by code.\n"
"- **Settings** — cameras, network, storage, appearance, and this manual.\n\n"

"## Snaps and how long they last\n\n"
"Open a snap to view it; once you've seen it, it leaves your inbox. Snaps are "
"**ephemeral by design** — by default they're removed from the app "
"automatically after about a month.\n\n"
"To keep a moment, **save it** while you're viewing it; the save button copies "
"it to your photo library and that copy is never auto-removed. Once a month the "
"app also offers to **export** everything from the past month as a single "
"archive — a good habit if you'd like a backup of your moments.\n\n"

"### Keep everything (power users)\n\n"
"If you'd rather nothing were ever deleted automatically, turn on **Keep "
"everything** in *Settings → Storage*. New snaps then never expire, and "
"switching it on also keeps everything already on your device. With this on, "
"managing space and pulling exports is up to you.\n\n"

"## Voice and group calls\n\n"
"You can talk to a friend, or a group, by voice. Calls run in push-to-talk or "
"hands-free (voice-activated) mode. When you raise the phone to your ear during "
"a call, the screen dims and the call switches to push-to-talk automatically — "
"see the screensaver setting below.\n\n"

"## Proximity screensaver\n\n"
"A calm screensaver you can enable in *Settings*: when a voice conversation is "
"open and you set the phone down or hold it to your ear, the screen gently "
"dims, and a new snap wakes it again. It uses the phone's proximity sensor and "
"takes effect the next time you launch the app.\n\n"

"## Privacy, sounds, and appearance\n\n"
"SnapClone is built to stay quiet and private. You can tune notification and "
"call sounds, and choose a theme that suits you in *Settings → Appearance*. "
"Your identity keys and contacts live only on your device.\n\n"

"## Saving and exporting\n\n"
"- **Save** (while viewing a snap): keeps a copy in your photo library and "
"exempts it from auto-removal.\n"
"- **Monthly export**: bundles the last month's snaps into one archive you can "
"keep or move elsewhere.\n\n"

"---\n\n"

"## Where your files live\n\n"
"For power users who like to manage data directly, here's where things are kept "
"by default on this device:\n\n"
"- **Snap media (in-app copies)**\n  `%1`\n"
"- **Snaps you save**\n  `%2`\n"
"- **Monthly exports**\n  `%3`\n"
"- **Message database**\n  `%4`\n\n"
"Clearing the cache from *Settings → Storage* removes temporary video "
"recordings only; your identity keys, contacts, and saved snaps are left "
"alone.\n")
        .arg(media, saved, exports, db);
}

} // namespace SnapClone::UI
