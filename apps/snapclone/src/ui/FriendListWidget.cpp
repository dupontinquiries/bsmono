#include "ui/FriendListWidget.hpp"
#include "ui/ContactEditorDialog.hpp"
#include "core/VCardIO.hpp"
#include <QFileDialog>
#include <QFile>
#include "ui/Theme.hpp"

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QListWidget>
#include <QListWidgetItem>
#include <QPushButton>
#include <QLineEdit>
#include <QLabel>
#include <QGroupBox>

// ─── UI/FriendListWidget.cpp ──────────────────────────────────────────────────
//
// Friend list + manual peer connection form.
// Online status shown as a coloured dot in the list item text.
// All colours from Theme namespace.
// ─────────────────────────────────────────────────────────────────────────────

namespace SnapClone::UI {

FriendListWidget::FriendListWidget(QWidget *parent)
    : QWidget(parent)
{
    buildUi();
}

void FriendListWidget::buildUi()
{
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                              Theme::SPACE_MD, Theme::SPACE_MD);
    root->setSpacing(Theme::SPACE_SM);

    // ── Header ────────────────────────────────────────────────────────────
    auto *title = new QLabel(tr("Friends"), this);
    title->setObjectName(QStringLiteral("titleLabel"));
    root->addWidget(title);

    // ── Friend list + empty state ─────────────────────────────────────────
    // Both live in the same stretch slot; we toggle visibility between them.
    auto *listArea = new QWidget(this);
    auto *listLayout = new QVBoxLayout(listArea);
    listLayout->setContentsMargins(0, 0, 0, 0);
    listLayout->setSpacing(0);

    m_list = new QListWidget(listArea);
    m_list->setVisible(false); // hidden until peers arrive
    listLayout->addWidget(m_list, 1);

    m_emptyLabel = new QLabel(
        tr("No peers found yet.\n\n"
           "Make sure both devices are on the same network\n"
           "with ports 9876/tcp and 9877/udp open.\n\n"
           "Or add a peer manually below."), listArea);
    m_emptyLabel->setAlignment(Qt::AlignCenter);
    m_emptyLabel->setWordWrap(true);
    m_emptyLabel->setObjectName(QStringLiteral("subtitleLabel"));
    listLayout->addWidget(m_emptyLabel, 1);

    root->addWidget(listArea, 1);

    // ── Add peer section ─────────────────────────────────────────────────
    // Uses the global QGroupBox stylesheet from Theme for consistent card look.
    auto *addBox = new QGroupBox(tr("Add Peer by Address"), this);

    auto *addLayout = new QVBoxLayout(addBox);
    addLayout->setSpacing(Theme::SPACE_SM);

    auto *addrRow = new QHBoxLayout;
    m_hostEdit = new QLineEdit(this);
    m_hostEdit->setPlaceholderText(tr("Host / IP  or  DHT ID (sc-...)"));
    m_hostEdit->setAttribute(Qt::WA_InputMethodEnabled);
    m_hostEdit->setInputMethodHints(Qt::ImhUrlCharactersOnly
                                    | Qt::ImhNoPredictiveText
                                    | Qt::ImhNoAutoUppercase);
    addrRow->addWidget(m_hostEdit, 3);

    m_portEdit = new QLineEdit(this);
    m_portEdit->setPlaceholderText(tr("Port"));
    m_portEdit->setAttribute(Qt::WA_InputMethodEnabled);
    m_portEdit->setInputMethodHints(Qt::ImhDigitsOnly);
    m_portEdit->setMaximumWidth(80);
    m_portEdit->setText(QStringLiteral("9876"));
    addrRow->addWidget(m_portEdit, 1);
    addLayout->addLayout(addrRow);

    m_addFriendButton = new QPushButton(tr("Connect"), this);
    m_addFriendButton->setObjectName(QStringLiteral("accentBtn"));
    addLayout->addWidget(m_addFriendButton);

    // Import vCard button
    auto *importBtn = new QPushButton(tr("📥 Import vCard"), this);
    importBtn->setToolTip(tr("Import contacts from a .vcf file"));
    addLayout->addWidget(importBtn);
    connect(importBtn, &QPushButton::clicked,
            this, &FriendListWidget::onImportVCard);

    root->addWidget(addBox);

    // ── Connections ───────────────────────────────────────────────────────
    connect(m_addFriendButton, &QPushButton::clicked,
            this, &FriendListWidget::onAddFriendClicked);
    connect(m_list, &QListWidget::itemDoubleClicked,
            this, &FriendListWidget::onItemDoubleClicked);
    connect(m_list, &QListWidget::currentRowChanged, this, [this](int row) {
        if (row < 0) return;
        const auto *item = m_list->item(row);
        if (item) emit friendSelected(Core::UserId{ item->data(Qt::UserRole).toString() });
    });
}

void FriendListWidget::setFriends(const QList<Core::UserProfile> &friends)
{
    m_list->clear();
    for (const Core::UserProfile &f : friends) {
        addOrUpdatePeer(f);
    }
    updateEmptyState();
}

void FriendListWidget::addOrUpdatePeer(const Core::UserProfile &profile)
{
    // Check if already in the list — update rather than duplicate.
    for (int i = 0; i < m_list->count(); ++i) {
        auto *item = m_list->item(i);
        if (item->data(Qt::UserRole).toString() == profile.id.value) {
            const QString dot = profile.online ? QStringLiteral("●") : QStringLiteral("○");
            item->setText(QStringLiteral("%1  %2").arg(dot, profile.displayName));
            item->setForeground(profile.online
                ? QColor(QLatin1String(Theme::SUCCESS()))
                : QColor(QLatin1String(Theme::TEXT_SECONDARY())));
            return;
        }
    }
    // New peer — insert.
    const QString dot   = profile.online ? QStringLiteral("●") : QStringLiteral("○");
    const QString label = QStringLiteral("%1  %2").arg(dot, profile.displayName);
    auto *item          = new QListWidgetItem(label, m_list);
    item->setData(Qt::UserRole, profile.id.value);
    item->setToolTip(profile.id.value); // full "username@ip" on hover
    item->setForeground(profile.online
        ? QColor(QLatin1String(Theme::SUCCESS()))
        : QColor(QLatin1String(Theme::TEXT_SECONDARY())));

    updateEmptyState();
}

void FriendListWidget::markOnline(const Core::UserId &userId, bool online)
{
    for (int i = 0; i < m_list->count(); ++i) {
        auto *item = m_list->item(i);
        if (item->data(Qt::UserRole).toString() != userId.value) continue;

        // Rebuild the label — strip the old dot (first two chars: dot + space).
        const QString name = item->text().mid(2);
        const QString dot  = online ? QStringLiteral("●") : QStringLiteral("○");
        item->setText(QStringLiteral("%1  %2").arg(dot, name));
        item->setForeground(online
            ? QColor(QLatin1String(Theme::SUCCESS()))
            : QColor(QLatin1String(Theme::TEXT_SECONDARY())));
        break;
    }
}

std::optional<Core::UserId> FriendListWidget::selectedFriend() const
{
    const auto *item = m_list->currentItem();
    if (!item) return std::nullopt;
    return Core::UserId{ item->data(Qt::UserRole).toString() };
}

void FriendListWidget::onAddFriendClicked()
{
    const QString input = m_hostEdit->text().trimmed();
    if (input.isEmpty()) return;

    // Accept two formats:
    //   "sc-a3f7c291"       — DHT stable ID (resolved via DHT lookup)
    //   "192.168.1.5"       — direct IP, uses port field
    //   "192.168.1.5:9876"  — direct IP with embedded port
    if (input.startsWith(QStringLiteral("sc-"))) {
        // DHT ID — port 0 signals to AppController to do a DHT lookup
        emit addFriendRequested(input, 0);
    } else if (input.contains(QLatin1Char(':'))) {
        // "host:port" format
        const QStringList parts = input.split(QLatin1Char(':'));
        const quint16 port = static_cast<quint16>(parts.last().toUShort());
        const QString host = parts.first();
        if (!host.isEmpty() && port > 0)
            emit addFriendRequested(host, port);
        return;
    } else {
        const quint16 port = static_cast<quint16>(m_portEdit->text().toUShort());
        if (port == 0) return;
        emit addFriendRequested(input, port);
    }
    m_hostEdit->clear();
}

void FriendListWidget::setContacts(const QList<Core::Contact> &contacts)
{
    m_list->clear();
    m_contacts.clear();
    for (const Core::Contact &c : contacts)
        addOrUpdateContact(c);
}

void FriendListWidget::addOrUpdateContact(const Core::Contact &contact)
{
    m_contacts[contact.id.value] = contact;
    // Update existing row or insert new
    for (int i = 0; i < m_list->count(); ++i) {
        auto *item = m_list->item(i);
        if (item->data(Qt::UserRole).toString() != contact.id.value) continue;
        const QString dot = contact.online ? QStringLiteral("●") : QStringLiteral("○");
        item->setText(QStringLiteral("%1  %2").arg(dot, contact.label()));
        item->setForeground(contact.online
            ? QColor(QLatin1String(Theme::SUCCESS()))
            : QColor(QLatin1String(Theme::TEXT_SECONDARY())));
        return;
    }
    const QString dot   = contact.online ? QStringLiteral("●") : QStringLiteral("○");
    const QString label = QStringLiteral("%1  %2").arg(dot, contact.label());
    auto *item = new QListWidgetItem(label, m_list);
    item->setData(Qt::UserRole, contact.id.value);
    item->setToolTip(QStringLiteral("%1\n%2")
        .arg(contact.id.value, contact.displayName));
    item->setForeground(contact.online
        ? QColor(QLatin1String(Theme::SUCCESS()))
        : QColor(QLatin1String(Theme::TEXT_SECONDARY())));
    updateEmptyState();
}

void FriendListWidget::onItemDoubleClicked(QListWidgetItem *item)
{
    if (!item) return;
    const QString id = item->data(Qt::UserRole).toString();
    if (!m_contacts.contains(id)) return;
    auto *dlg = new ContactEditorDialog(m_contacts[id], this);
    dlg->setAttribute(Qt::WA_DeleteOnClose);
    connect(dlg, &ContactEditorDialog::deleteRequested,
            this, &FriendListWidget::contactDeleted);
    dlg->open();
    // Result handled via accepted() signal — see constructor connects
    if (false // exec() replaced with open() — result via signal
        && dlg->result() == QDialog::Accepted) {
        const Core::Contact edited = dlg->editedContact();
        m_contacts[id] = edited;
        addOrUpdateContact(edited); // refresh label
        emit contactEdited(edited);
    }
}

void FriendListWidget::onImportVCard()
{
    const QString path = QFileDialog::getOpenFileName(
        this, tr("Import vCard"), QString{},
        tr("vCard files (*.vcf *.vcard)"));
    if (path.isEmpty()) return;
    QFile f(path);
    if (!f.open(QIODevice::ReadOnly | QIODevice::Text)) return;
    const QList<Core::Contact> imported =
        Core::VCardIO::importVCard(QString::fromUtf8(f.readAll()));
    for (const Core::Contact &c : imported) {
        m_contacts[c.id.value] = c;
        addOrUpdateContact(c);
        emit contactEdited(c); // AppController will saveContact
    }
    emit importVCardRequested();
}

void FriendListWidget::updateEmptyState()
{
    const bool hasItems = (m_list->count() > 0);
    m_list->setVisible(hasItems);
    m_emptyLabel->setVisible(!hasItems);
}

} // namespace SnapClone::UI
