#pragma once

// ─── UI/FriendListWidget.hpp ──────────────────────────────────────────────────
//
// Shows friends with online/offline indicators and allows selecting a
// recipient before sending a snap.
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include "core/Types.hpp"
#include <QHash>
#include <optional>

QT_BEGIN_NAMESPACE
class QListWidget;
class QLabel;
class QPushButton;
class QLineEdit;
class QListWidgetItem;
QT_END_NAMESPACE

namespace SnapClone::UI {

class FriendListWidget : public QWidget
{
    Q_OBJECT

public:
    explicit FriendListWidget(QWidget *parent = nullptr);

    void setFriends(const QList<Core::UserProfile> &friends);
    void setContacts(const QList<Core::Contact> &contacts);
    void addOrUpdateContact(const Core::Contact &contact);
    /// Add a newly discovered peer or update their online status.
    /// Called automatically when peerConnected fires — no user action needed.
    void addOrUpdatePeer(const Core::UserProfile &profile);
    void markOnline(const Core::UserId &userId, bool online);
    [[nodiscard]] std::optional<Core::UserId> selectedFriend() const;

signals:
    void friendSelected(SnapClone::Core::UserId id);
    // Emitted when the user hits Connect.
    // If host is a DHT ID (starts with "sc-"), port will be 0 and
    // AppController resolves it via DHT lookup before connecting.
    void addFriendRequested(QString host, quint16 port);
    void contactEdited(SnapClone::Core::Contact contact);
    void contactDeleted(SnapClone::Core::UserId id);
    void importVCardRequested();

private slots:
    void onAddFriendClicked();
    void onItemDoubleClicked(QListWidgetItem *item);
    void onImportVCard();

private:
    void buildUi();
    void updateEmptyState();

    QListWidget *m_list            = nullptr;
    QLabel      *m_emptyLabel      = nullptr;
    QLineEdit   *m_hostEdit        = nullptr;
    QLineEdit   *m_portEdit        = nullptr;
    QPushButton *m_addFriendButton = nullptr;
    QHash<QString, Core::Contact> m_contacts; // keyed by stable ID
};

} // namespace SnapClone::UI
