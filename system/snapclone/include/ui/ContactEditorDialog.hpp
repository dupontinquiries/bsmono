#pragma once
#include "ui/MobileDialog.hpp"
#include "core/Types.hpp"

#include <QPoint>  // V32.34+: tap-to-copy press position

QT_BEGIN_NAMESPACE
class QLineEdit;
class QTextEdit;
class QLabel;
class QPushButton;
class QEvent;       // V32.34+: eventFilter parameter
QT_END_NAMESPACE

namespace SnapClone::UI {

/// Full-screen contact editor — shown when a user long-presses or taps
/// the ⋯ button on a contact in the FriendListWidget.
/// Lets the user set a local nickname and notes, view the sc-ID and
/// addresses, export the contact as vCard, or delete it.
///
/// V32.34+: enhanced with:
///   • Online status badge + "last seen N hours ago" text
///   • Avatar uses ThumbnailService — pulls latest snap thumbnail if
///     available, falls back to single-letter monogram.
///   • Tap-anywhere-on-id-row to copy (not just the small button)
///   • ChevronConfirm-based delete confirmation (matches app style)
///   • Pending-state hint banner ("This contact hasn't connected yet")
class ContactEditorDialog : public MobileDialog
{
    Q_OBJECT
public:
    explicit ContactEditorDialog(const Core::Contact &contact,
                                 QWidget *parent = nullptr);

    /// The contact as edited — call this after exec() == Accepted.
    [[nodiscard]] Core::Contact editedContact() const;

    /// V32.34+: optional avatar source — path to the most recent received
    /// photo from this contact. If set, the avatar slot loads it via
    /// ThumbnailService instead of showing the initials monogram. Call
    /// BEFORE exec(). Caller (FriendListWidget / InboxWidget) is
    /// responsible for resolving the path from their own snap index.
    void setAvatarSourcePath(const QString &path);

signals:
    void deleteRequested(Core::UserId id);

protected:
    /// V32.34+: tap-anywhere-to-copy on the sc-ID label. The label
    /// keeps its native text-selection capability; we only intercept
    /// short taps that don't move enough to be a deliberate drag.
    bool eventFilter(QObject *watched, QEvent *e) override;

private:
    void buildUi();
    void onSave();
    void onDelete();
    void onExportVCard();
    void onCopyId();
    /// V32.34+: helper — format lastSeen as a human-readable relative time
    /// ("just now", "2h ago", "yesterday"). Returns empty if no lastSeen.
    [[nodiscard]] QString lastSeenText() const;
    /// V32.34+: load avatar via ThumbnailService. If m_avatarSourcePath
    /// is empty or load fails, leaves the existing monogram in place.
    void loadAvatarAsync();

    Core::Contact m_contact;
    QString       m_avatarSourcePath;  ///< V32.34+: see setAvatarSourcePath

    QLineEdit *m_nicknameEdit = nullptr;
    QTextEdit *m_notesEdit    = nullptr;
    QLabel    *m_scIdLabel    = nullptr;
    QLabel    *m_addrLabel    = nullptr;
    QLabel    *m_avatarLabel  = nullptr;  ///< V32.34+: dynamic — fills with thumb when loaded
    QLabel    *m_statusBadge  = nullptr;  ///< V32.34+: Online / Last seen / Pending
    QPushButton *m_deleteBtn  = nullptr;  ///< V32.34+: anchor for ChevronConfirm

    /// V32.34+: per-instance state for the tap-to-copy gesture on
    /// m_scIdLabel. Was a function-local static initially, but two
    /// open editors would have clobbered each other's press state.
    QPoint m_idPressPos;
    bool   m_idPressArmed = false;
};

} // namespace SnapClone::UI
