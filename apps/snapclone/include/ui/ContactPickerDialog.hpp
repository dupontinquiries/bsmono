#pragma once
// ─── ContactPickerDialog ──────────────────────────────────────────────────────
// Modal sheet that appears after camera capture to let the user choose
// which contact to send the snap to. Shows online contacts first with
// their pulse badge, then offline contacts dimmed.

#include "core/Types.hpp"
#include "ui/MobileDialog.hpp"
#include <QList>

QT_BEGIN_NAMESPACE
class QListWidget;
class QListWidgetItem;
class QPushButton;
class QLabel;
QT_END_NAMESPACE

namespace SnapClone::UI {

class ContactPickerDialog : public MobileDialog
{
    Q_OBJECT
public:
    explicit ContactPickerDialog(const QList<Core::Contact> &contacts,
                                  QWidget *parent = nullptr);

    [[nodiscard]] Core::UserId selectedContact() const;

    // Multi-select mode: tap to toggle several contacts, then confirm with the
    // footer button. Used to form a group / add people to a call. The default
    // stays single-select (the snap-send flow), so existing callers are unchanged.
    void setMultiSelect(bool on, const QString &confirmLabel = QString());
    /// Replace the header title (e.g. "New voice chat") so multi-select reuses
    /// of this picker read as a different action than the snap send.
    void setHeaderTitle(const QString &title);
    [[nodiscard]] QList<Core::UserId> selectedContacts() const;

signals:
    void contactSelected(const Core::UserId &id);
    void contactsConfirmed(const QList<Core::UserId> &ids);

private:
    void buildUi(const QList<Core::Contact> &contacts);
    void onItemDoubleClicked(QListWidgetItem *item);

    QListWidget *m_list    = nullptr;
    QPushButton *m_sendBtn = nullptr;
    QLabel      *m_titleLabel = nullptr;
    bool         m_multi   = false;
};

} // namespace SnapClone::UI
