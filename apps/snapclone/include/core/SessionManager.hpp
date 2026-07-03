#pragma once

// ─── Core/SessionManager.hpp ─────────────────────────────────────────────────
//
// Manages the local user's authenticated session.
//
// This is intentionally decoupled from network concerns: it only answers
// "who is logged in and what are their credentials?" — it does not know
// how credentials are verified across the network.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <optional>
#include "core/Types.hpp"

namespace SnapClone::Core {

enum class SessionState {
    LoggedOut,
    LoggingIn,
    LoggedIn,
};

class SessionManager : public QObject
{
    Q_OBJECT
    Q_PROPERTY(SessionState state READ state NOTIFY stateChanged)

public:
    explicit SessionManager(QObject *parent = nullptr);

    [[nodiscard]] SessionState            state()         const;
    [[nodiscard]] std::optional<UserId>   currentUserId() const;
    [[nodiscard]] std::optional<UserProfile> currentProfile() const;

    void beginLogin(const QString &username, const QString &passwordHash);
    void logout();

signals:
    void stateChanged(SnapClone::Core::SessionState newState);
    void loginSucceeded(SnapClone::Core::UserProfile profile);
    void loginFailed(QString reason);

private:
    SessionState              m_state   = SessionState::LoggedOut;
    std::optional<UserProfile> m_profile;
};

} // namespace SnapClone::Core
