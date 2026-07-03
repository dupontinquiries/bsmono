#include "ui/LoginWidget.hpp"
#include "ui/Theme.hpp"

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QLineEdit>
#include <QPushButton>
#include <QLabel>
#include <QCryptographicHash>
#include <QKeyEvent>

// ─── UI/LoginWidget.cpp ───────────────────────────────────────────────────────
//
// Fallback login screen — shown only when auto-login fails.
// In normal operation this widget is never visible.
//
// The raw password is hashed (SHA-256) before the loginRequested signal fires;
// the plain-text password never leaves this widget.
// ─────────────────────────────────────────────────────────────────────────────

namespace SnapClone::UI {

LoginWidget::LoginWidget(QWidget *parent)
    : QWidget(parent)
{
    buildUi();
}

void LoginWidget::buildUi()
{
    // Full-widget background so it completely covers the swiper underneath.
    setStyleSheet(QStringLiteral("QWidget { background-color: %1; }")
                      .arg(QLatin1String(Theme::BG_DEEP())));

    auto *root = new QVBoxLayout(this);
    root->setAlignment(Qt::AlignCenter);
    root->setSpacing(Theme::SPACE_MD);
    root->setContentsMargins(Theme::SPACE_XL, Theme::SPACE_XL,
                              Theme::SPACE_XL, Theme::SPACE_XL);

    // ── Logo ──────────────────────────────────────────────────────────────
    auto *logo = new QLabel(QStringLiteral("👻"), this);
    logo->setAlignment(Qt::AlignCenter);
    logo->setStyleSheet(QStringLiteral("font-size: 72px; background: transparent;"));
    root->addWidget(logo);

    auto *title = new QLabel(QStringLiteral("SnapClone"), this);
    title->setObjectName(QStringLiteral("titleLabel"));
    title->setAlignment(Qt::AlignCenter);
    root->addWidget(title);

    root->addSpacing(Theme::SPACE_MD);

    // ── Inputs ────────────────────────────────────────────────────────────
    m_usernameEdit = new QLineEdit(this);
    m_usernameEdit->setPlaceholderText(tr("Username"));
    // Qt::ImhNoPredictiveText: suppress autocorrect/autocomplete on mobile.
    // WA_InputMethodEnabled: required for Wayland compositors to show the VK.
    m_usernameEdit->setAttribute(Qt::WA_InputMethodEnabled);
    m_usernameEdit->setInputMethodHints(Qt::ImhNoPredictiveText
                                        | Qt::ImhPreferLowercase);
    root->addWidget(m_usernameEdit);

    m_passwordEdit = new QLineEdit(this);
    m_passwordEdit->setPlaceholderText(tr("Password (optional)"));
    m_passwordEdit->setEchoMode(QLineEdit::Password);
    m_passwordEdit->setAttribute(Qt::WA_InputMethodEnabled);
    m_passwordEdit->setInputMethodHints(Qt::ImhHiddenText
                                        | Qt::ImhSensitiveData
                                        | Qt::ImhNoPredictiveText
                                        | Qt::ImhNoAutoUppercase);
    root->addWidget(m_passwordEdit);

    // ── Error label (hidden until needed) ─────────────────────────────────
    m_errorLabel = new QLabel(this);
    m_errorLabel->setObjectName(QStringLiteral("errorLabel"));
    m_errorLabel->setAlignment(Qt::AlignCenter);
    m_errorLabel->setWordWrap(true);
    m_errorLabel->hide();
    root->addWidget(m_errorLabel);

    // ── Login button ──────────────────────────────────────────────────────
    m_loginButton = new QPushButton(tr("Sign In"), this);
    m_loginButton->setObjectName(QStringLiteral("accentBtn"));
    root->addWidget(m_loginButton);

    // ── Hint ──────────────────────────────────────────────────────────────
    auto *hint = new QLabel(
        tr("No account needed — any username gets you started."), this);
    hint->setObjectName(QStringLiteral("subtitleLabel"));
    hint->setAlignment(Qt::AlignCenter);
    hint->setWordWrap(true);
    root->addWidget(hint);

    // ── Connections ───────────────────────────────────────────────────────
    connect(m_loginButton,  &QPushButton::clicked,
            this, &LoginWidget::onLoginClicked);
    connect(m_passwordEdit, &QLineEdit::returnPressed,
            this, &LoginWidget::onLoginClicked);
    connect(m_usernameEdit, &QLineEdit::returnPressed,
            this, [this]() { m_passwordEdit->setFocus(); });
}

void LoginWidget::onLoginClicked()
{
    const QString username = m_usernameEdit->text().trimmed();
    clearError();

    if (username.isEmpty()) {
        showError(tr("Username cannot be empty."));
        return;
    }

    const QString password = m_passwordEdit->text();
    const QByteArray hash  =
        QCryptographicHash::hash(password.toUtf8(), QCryptographicHash::Sha256).toHex();

    emit loginRequested(username, QString::fromLatin1(hash));
}

void LoginWidget::showError(const QString &message)
{
    m_errorLabel->setText(message);
    m_errorLabel->show();
    m_passwordEdit->clear();
    m_passwordEdit->setFocus();
}

void LoginWidget::clearError()
{
    m_errorLabel->clear();
    m_errorLabel->hide();
}

} // namespace SnapClone::UI
