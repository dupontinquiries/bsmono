#include "core/SessionManager.hpp"
#include <QLoggingCategory>

Q_LOGGING_CATEGORY(lcSession, "snapclone.session")

namespace SnapClone::Core {

SessionManager::SessionManager(QObject *parent)
    : QObject(parent)
{}

SessionState SessionManager::state() const { return m_state; }

std::optional<UserId> SessionManager::currentUserId() const
{
    if (m_profile) return m_profile->id;
    return std::nullopt;
}

std::optional<UserProfile> SessionManager::currentProfile() const
{
    return m_profile;
}

void SessionManager::beginLogin(const QString &username, const QString &passwordHash)
{
    if (m_state == SessionState::LoggingIn) return;

    m_state = SessionState::LoggingIn;
    emit stateChanged(m_state);

    qCInfo(lcSession) << "Beginning login for user:" << username;

    // In a fully distributed system, login can be verified locally using a
    // stored credential hash (no central auth server required).
    // Here we accept any non-empty username/hash pair for demonstration.
    if (username.isEmpty() || passwordHash.isEmpty()) {
        m_state = SessionState::LoggedOut;
        emit stateChanged(m_state);
        emit loginFailed(QStringLiteral("Username and password are required."));
        return;
    }

    // Build a synthetic user profile.
    // The userId is stored verbatim (no case folding or space replacement)
    // so it matches the identity key files on disk.
    UserProfile profile;
    profile.id          = UserId{ username.trimmed() };
    profile.displayName = username.trimmed();
    profile.online      = true;

    m_profile = profile;
    m_state   = SessionState::LoggedIn;

    emit stateChanged(m_state);
    emit loginSucceeded(profile);

    qCInfo(lcSession) << "Login succeeded; userId =" << profile.id.value;
}

void SessionManager::logout()
{
    if (m_state == SessionState::LoggedOut) return;

    qCInfo(lcSession) << "Logging out user:" << (m_profile ? m_profile->id.value : QStringLiteral("<none>"));
    m_profile.reset();
    m_state = SessionState::LoggedOut;
    emit stateChanged(m_state);
}

} // namespace SnapClone::Core
