#include "ui/ContactEditorDialog.hpp"
#include "ui/Theme.hpp"
#include "ui/ThumbnailService.hpp"   // V32.34+: async avatar load
#include "ui/ChevronConfirm.hpp"     // V32.34+: app-style delete confirm
#include "core/VCardIO.hpp"

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QTextEdit>
#include <QPushButton>
#include <QFileDialog>
#include <QFile>
#include <QGuiApplication>
#include <QScreen>
#include <QClipboard>
#include <QTimer>
#include <QDateTime>           // V32.34+: lastSeen formatting
#include <QFrame>              // V32.34+: pending banner separator
#include <QMouseEvent>         // V32.34+: tap-anywhere-to-copy
#include <QPainter>            // V32.34+: avatar drawing fallback
#include <QPainterPath>        // V32.34+: round-clip the avatar
#include <QPointer>

namespace SnapClone::UI {

ContactEditorDialog::ContactEditorDialog(const Core::Contact &contact, QWidget *parent)
    : MobileDialog(parent)
    , m_contact(contact)
{
    setWindowTitle(tr("Edit Contact"));
    setModal(true);
    // V32.34+: fit to screen on mobile.
    //
    // The previous fixed 380×560 was wider than a typical phone (FP5 is
    // 360px logical wide), causing the dialog to clip its right edge and
    // making the "Save" button at the bottom unreachable on portrait
    // mobile. We now read the screen size at construction:
    //
    //   • Width: 92% of available screen width, capped at 420px so it
    //     doesn't grow ridiculous on desktop.
    //   • Height: 80% of available height, capped at 640px.
    //
    // MobileDialog handles centering and sheet-style chrome; we just
    // pick a sensible size for this dialog's content.
    if (auto *scr = QGuiApplication::primaryScreen()) {
        const QSize avail = scr->availableSize();
        const int w = qMin(420, static_cast<int>(avail.width()  * 0.92));
        const int h = qMin(640, static_cast<int>(avail.height() * 0.80));
        resize(w, h);
    } else {
        resize(360, 560);
    }
    buildUi();
}

void ContactEditorDialog::setAvatarSourcePath(const QString &path)
{
    m_avatarSourcePath = path;
    // If buildUi has already run we can load right away; else
    // buildUi will call loadAvatarAsync() at the end.
    if (m_avatarLabel) loadAvatarAsync();
}

QString ContactEditorDialog::lastSeenText() const
{
    if (m_contact.online) return tr("Online now");
    if (!m_contact.lastSeen.isValid()) {
        if (m_contact.state == Core::ContactState::Pending) {
            return tr("Never connected");
        }
        return QString();
    }
    const qint64 secs = m_contact.lastSeen.secsTo(QDateTime::currentDateTimeUtc());
    if (secs < 60)         return tr("Last seen just now");
    if (secs < 3600)       return tr("Last seen %1m ago").arg(secs / 60);
    if (secs < 86400)      return tr("Last seen %1h ago").arg(secs / 3600);
    if (secs < 7 * 86400)  return tr("Last seen %1 days ago").arg(secs / 86400);
    // Older than a week — show a fuller date so users can decide
    // whether the contact is "active enough" to keep around.
    return tr("Last seen %1").arg(m_contact.lastSeen.toLocalTime().toString(QStringLiteral("MMM d")));
}

void ContactEditorDialog::loadAvatarAsync()
{
    if (!m_avatarLabel) return;
    if (m_avatarSourcePath.isEmpty()) return;
    // Capture avatar label by QPointer — if the dialog is dismissed
    // before the worker finishes, the callback's listGuard check drops
    // the result without touching freed memory.
    QPointer<QLabel> guard = m_avatarLabel;
    ThumbnailService::instance().request(
        m_avatarSourcePath, 64, this,
        [guard](const QPixmap &pm) {
            if (!guard) return;
            if (pm.isNull()) return;
            // Round-clip the loaded pixmap so it matches the monogram's
            // circular shape. Doing this on the worker would tie the
            // service to a contact-specific style; we do it here on the
            // GUI thread instead — the work is bounded (64×64) and
            // happens once per editor open.
            QPixmap round(64, 64);
            round.fill(Qt::transparent);
            {
                QPainter p(&round);
                p.setRenderHint(QPainter::Antialiasing);
                QPainterPath clip;
                clip.addEllipse(0, 0, 64, 64);
                p.setClipPath(clip);
                p.drawPixmap(0, 0, pm);
            }
            guard->setText(QString());     // clear monogram
            guard->setPixmap(round);
            // Reset stylesheet so the rounded pixmap fills the circle;
            // the original monogram styling drew the colored background.
            guard->setStyleSheet(QString());
        });
}

void ContactEditorDialog::buildUi()
{
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                             Theme::SPACE_MD, Theme::SPACE_MD);
    root->setSpacing(Theme::SPACE_SM);

    // ── Header: avatar (round) + name + sc-ID (tappable to copy) ──────────
    auto *headerRow = new QHBoxLayout;
    headerRow->setSpacing(Theme::SPACE_MD);

    m_avatarLabel = new QLabel(this);
    m_avatarLabel->setFixedSize(64, 64);
    m_avatarLabel->setAlignment(Qt::AlignCenter);
    // V32.37: use design tokens (was hardcoded 32px radius / 28px font). A 64px
    // box with RADIUS_FULL renders as a circle; FONT_2XL is the redesign's
    // large-display size for monogram initials.
    m_avatarLabel->setStyleSheet(QStringLiteral(
        "background: %1; border-radius: %3px; color: %2;"
        " font-size: %4px; font-weight: bold;")
        .arg(QLatin1String(Theme::BG_RAISED()), QLatin1String(Theme::ACCENT()))
        .arg(32)                       // half of 64 = perfect circle
        .arg(Theme::FONT_2XL));
    const QString initials = m_contact.label().isEmpty()
        ? QStringLiteral("?")
        : m_contact.label().at(0).toUpper();
    m_avatarLabel->setText(initials);
    headerRow->addWidget(m_avatarLabel);

    auto *idCol = new QVBoxLayout;
    idCol->setSpacing(4);

    // Status badge: Online / Last seen / Pending / nothing.
    m_statusBadge = new QLabel(this);
    {
        const QString text = lastSeenText();
        m_statusBadge->setText(text);
        const QString badgeColor =
            m_contact.online ? QLatin1String(Theme::ACCENT())
                             : QLatin1String(Theme::TEXT_SECONDARY());
        m_statusBadge->setStyleSheet(QStringLiteral(
            "color: %1; font-size: %2px;").arg(badgeColor).arg(Theme::FONT_SM));
        m_statusBadge->setVisible(!text.isEmpty());
    }
    idCol->addWidget(m_statusBadge);

    m_scIdLabel = new QLabel(m_contact.id.value, this);
    m_scIdLabel->setTextInteractionFlags(Qt::TextSelectableByMouse
                                         | Qt::TextSelectableByKeyboard);
    m_scIdLabel->setStyleSheet(QStringLiteral(
        "font-family: monospace; font-size: %3px; color: %1; background: %2;"
        " border-radius: %4px; padding: %5px %6px;")
        .arg(QLatin1String(Theme::ACCENT()), QLatin1String(Theme::BG_RAISED()))
        .arg(Theme::FONT_BASE).arg(Theme::RADIUS_SM)
        .arg(Theme::SPACE_XS / 2).arg(Theme::SPACE_SM));
    m_scIdLabel->setWordWrap(true);
    // V32.34+: make the whole ID label tappable for copy. Cursor hint +
    // tooltip make the affordance discoverable. We install an event
    // filter rather than subclass QLabel — the click handler routes
    // through onCopyId() which already animates the "✓ Copied!" feedback.
    m_scIdLabel->setCursor(Qt::PointingHandCursor);
    m_scIdLabel->setToolTip(tr("Tap to copy"));
    m_scIdLabel->installEventFilter(this);
    idCol->addWidget(m_scIdLabel);

    auto *copyIdBtn = new QPushButton(tr("📋 Copy ID"), this);
    copyIdBtn->setFixedHeight(Theme::TOUCH_MIN);
    copyIdBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background: transparent; color: %1;"
        " border: 1px solid %1; border-radius: %3px; font-size: %4px; }"
        "QPushButton:pressed { background: %2; }")
        .arg(QLatin1String(Theme::ACCENT()), QLatin1String(Theme::BG_RAISED()))
        .arg(Theme::RADIUS_SM).arg(Theme::FONT_SM));
    connect(copyIdBtn, &QPushButton::clicked, this, &ContactEditorDialog::onCopyId);
    idCol->addWidget(copyIdBtn);

    headerRow->addLayout(idCol, 1);
    root->addLayout(headerRow);

    // V32.34+: Pending state banner. When a contact has been added via
    // QR/vCard but hasn't yet connected, the editor shouldn't pretend
    // they're a regular contact — surface that they haven't been seen
    // and explain what would change once they connect.
    if (m_contact.state == Core::ContactState::Pending) {
        auto *banner = new QLabel(
            tr("⏳ Pending — this contact hasn't connected yet. "
               "Their display name will appear when they do."), this);
        banner->setWordWrap(true);
        banner->setStyleSheet(QStringLiteral(
            "background: %1; color: %2; border: 1px solid %3;"
            " border-radius: %4px; padding: %5px; font-size: %6px;")
            .arg(QLatin1String(Theme::BG_RAISED()),
                 QLatin1String(Theme::TEXT_SECONDARY()),
                 QLatin1String(Theme::BORDER()))
            .arg(Theme::RADIUS_SM).arg(Theme::SPACE_SM).arg(Theme::FONT_SM));
        root->addWidget(banner);
    }

    // ── Nickname ──────────────────────────────────────────────────────────
    auto *nickLabel = new QLabel(tr("Nickname"), this);
    nickLabel->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px;").arg(QLatin1String(Theme::TEXT_SECONDARY())).arg(Theme::FONT_SM));
    root->addWidget(nickLabel);

    m_nicknameEdit = new QLineEdit(m_contact.nickname, this);
    m_nicknameEdit->setPlaceholderText(
        m_contact.displayName.isEmpty() ? tr("Enter a name…") : m_contact.displayName);
    m_nicknameEdit->setMinimumHeight(Theme::TOUCH_MIN);
    m_nicknameEdit->setStyleSheet(QStringLiteral(
        "QLineEdit { background: %1; color: %2; border: 1px solid %3;"
        " border-radius: %4px; padding: %6px; font-size: %7px; }"
        "QLineEdit:focus { border-color: %5; }")
        .arg(QLatin1String(Theme::BG_RAISED()), QLatin1String(Theme::TEXT_PRIMARY()),
             QLatin1String(Theme::BORDER())).arg(Theme::RADIUS_SM)
        .arg(QLatin1String(Theme::ACCENT())).arg(Theme::SPACE_SM).arg(Theme::FONT_LG));
    root->addWidget(m_nicknameEdit);

    // ── Notes ─────────────────────────────────────────────────────────────
    auto *notesLabel = new QLabel(tr("Notes"), this);
    notesLabel->setStyleSheet(nickLabel->styleSheet());
    root->addWidget(notesLabel);

    m_notesEdit = new QTextEdit(this);
    m_notesEdit->setPlaceholderText(tr("Optional notes…"));
    m_notesEdit->setPlainText(m_contact.notes);
    m_notesEdit->setFixedHeight(80);
    m_notesEdit->setStyleSheet(QStringLiteral(
        "QTextEdit { background: %1; color: %2; border: 1px solid %3;"
        " border-radius: %4px; padding: %6px; font-size: %7px; }"
        "QTextEdit:focus { border-color: %5; }")
        .arg(QLatin1String(Theme::BG_RAISED()), QLatin1String(Theme::TEXT_PRIMARY()),
             QLatin1String(Theme::BORDER())).arg(Theme::RADIUS_SM)
        .arg(QLatin1String(Theme::ACCENT())).arg(Theme::SPACE_SM).arg(Theme::FONT_BASE));
    root->addWidget(m_notesEdit);

    // ── Display name (read-only from peer's handshake) ────────────────────
    if (!m_contact.displayName.isEmpty()) {
        auto *dnRow = new QHBoxLayout;
        auto *dnKey = new QLabel(tr("Peer name:"), this);
        dnKey->setStyleSheet(nickLabel->styleSheet());
        auto *dnVal = new QLabel(m_contact.displayName, this);
        dnVal->setStyleSheet(QStringLiteral("color: %1; font-size: %2px;")
                                 .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_BASE));
        dnVal->setTextInteractionFlags(Qt::TextSelectableByMouse);
        dnRow->addWidget(dnKey);
        dnRow->addWidget(dnVal, 1);
        root->addLayout(dnRow);
    }

    root->addStretch();

    // ── Action buttons ────────────────────────────────────────────────────
    auto *btnRow = new QHBoxLayout;
    btnRow->setSpacing(Theme::SPACE_SM);

    auto *deleteBtn = new QPushButton(tr("🗑 Delete"), this);
    deleteBtn->setMinimumHeight(Theme::TOUCH_MIN);
    deleteBtn->setObjectName(QStringLiteral("dangerBtn"));
    // V32.34+: store for the ChevronConfirm anchor in onDelete().
    // The button is a child of the dialog and lives until the dialog
    // closes — onDelete only fires while the dialog is still alive,
    // so the pointer is always valid when read.
    m_deleteBtn = deleteBtn;
    connect(deleteBtn, &QPushButton::clicked, this, &ContactEditorDialog::onDelete);
    btnRow->addWidget(deleteBtn);

    auto *vcardBtn = new QPushButton(tr("📤 Export vCard"), this);
    vcardBtn->setMinimumHeight(Theme::TOUCH_MIN);
    connect(vcardBtn, &QPushButton::clicked, this, &ContactEditorDialog::onExportVCard);
    btnRow->addWidget(vcardBtn);

    btnRow->addStretch();

    auto *saveBtn = new QPushButton(tr("Save"), this);
    saveBtn->setMinimumHeight(Theme::TOUCH_MIN);
    saveBtn->setObjectName(QStringLiteral("accentBtn"));
    connect(saveBtn, &QPushButton::clicked, this, &ContactEditorDialog::onSave);
    btnRow->addWidget(saveBtn);

    root->addLayout(btnRow);

    // V32.34+: kick off async avatar load if a source was provided.
    // Called LAST so m_avatarLabel exists.
    loadAvatarAsync();
}

Core::Contact ContactEditorDialog::editedContact() const
{
    Core::Contact c = m_contact;
    c.nickname = m_nicknameEdit->text().trimmed();
    c.notes    = m_notesEdit->toPlainText().trimmed();
    return c;
}

// V32.34+: tap-to-copy on the sc-ID label.
//
// We install an event filter on m_scIdLabel instead of subclassing
// QLabel. The label still supports text-selection (a feature the user
// values for desktop) but a quick tap with no drag = copy. We
// distinguish via the press/release pair: if the press position and
// release position are within ~6 logical pixels of each other, treat
// it as a tap. Otherwise let the selection gesture proceed.
bool ContactEditorDialog::eventFilter(QObject *watched, QEvent *e)
{
    if (watched != m_scIdLabel) return MobileDialog::eventFilter(watched, e);
    switch (e->type()) {
    case QEvent::MouseButtonPress: {
        auto *me = static_cast<QMouseEvent *>(e);
        m_idPressPos   = me->position().toPoint();
        m_idPressArmed = true;
        // Don't accept — let QLabel's normal selection start.
        return false;
    }
    case QEvent::MouseButtonRelease: {
        if (!m_idPressArmed) return false;
        m_idPressArmed = false;
        auto *me = static_cast<QMouseEvent *>(e);
        const QPoint relPos = me->position().toPoint();
        const int dx = qAbs(relPos.x() - m_idPressPos.x());
        const int dy = qAbs(relPos.y() - m_idPressPos.y());
        // 6 px movement budget — anything more is a deliberate drag for
        // text selection, not a tap.
        if (dx <= 6 && dy <= 6) {
            onCopyId();
            return true;  // we handled it
        }
        return false;
    }
    default: break;
    }
    return MobileDialog::eventFilter(watched, e);
}

void ContactEditorDialog::onSave()
{
    m_contact = editedContact();
    accept();
}

void ContactEditorDialog::onDelete()
{
    // V32.34+: confirm via ChevronConfirm instead of QMessageBox.
    // ChevronConfirm matches the app's modal style, has built-in
    // anchoring near the button, and uses the UxGuide-aware "Tap" vs
    // "Hold" decision. The familiarity feature key "kDeleteContact"
    // already exists in the registry, so first-time deletes get a
    // hold-to-confirm (extra friction); repeat deletes after the user
    // is familiar get a quick tap-to-confirm.
    QPointer<ContactEditorDialog> self(this);
    QWidget *anchor = m_deleteBtn ? static_cast<QWidget *>(m_deleteBtn) : this;
    ChevronConfirm::present(
        anchor,
        tr("Remove %1 from your address book?").arg(m_contact.label()),
        QLatin1String("delete_contact"),
        [self]() {
            if (!self) return;
            emit self->deleteRequested(self->m_contact.id);
            self->reject();
        });
}

void ContactEditorDialog::onExportVCard()
{
    const QString path = QFileDialog::getSaveFileName(
        this, tr("Export vCard"),
        QStringLiteral("%1.vcf").arg(m_contact.label()),
        tr("vCard files (*.vcf)"));
    if (path.isEmpty()) return;

    QFile f(path);
    if (f.open(QIODevice::WriteOnly | QIODevice::Text)) {
        const Core::Contact c = editedContact();
        f.write(Core::VCardIO::exportContact(c).toUtf8());
        f.close();
    }
}

void ContactEditorDialog::onCopyId()
{
    QGuiApplication::clipboard()->setText(m_contact.id.value);
    // Brief confirmation via the label itself
    m_scIdLabel->setText(tr("✓ Copied!"));
    QTimer::singleShot(1500, this,
        [this]() { m_scIdLabel->setText(m_contact.id.value); });
}

} // namespace SnapClone::UI
