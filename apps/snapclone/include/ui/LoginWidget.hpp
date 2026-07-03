#pragma once

// ─── UI/LoginWidget.hpp ───────────────────────────────────────────────────────
//
// A simple username + password entry screen.
//
// Responsibilities:
//   • Collect credentials and emit loginRequested.
//   • Display an error message if login fails.
//   • Clear the password field on every failed attempt.
//
// It knows nothing about how authentication works.
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>

QT_BEGIN_NAMESPACE
class QLineEdit;
class QPushButton;
class QLabel;
QT_END_NAMESPACE

namespace SnapClone::UI {

class LoginWidget : public QWidget
{
    Q_OBJECT

public:
    explicit LoginWidget(QWidget *parent = nullptr);

    void showError(const QString &message);
    void clearError();

signals:
    /// Emitted when the user submits credentials.
    /// passwordHash is SHA-256(password) — raw password never leaves this widget.
    void loginRequested(QString username, QString passwordHash);

private slots:
    void onLoginClicked();

private:
    void buildUi();

    QLineEdit   *m_usernameEdit = nullptr;
    QLineEdit   *m_passwordEdit = nullptr;
    QPushButton *m_loginButton  = nullptr;
    QLabel      *m_errorLabel   = nullptr;
};

} // namespace SnapClone::UI
