#pragma once
// ─── ui/posture/VoiceActivityTracker.hpp ──────────────────────────────────────
//
// Remembers when each conversation last had voice (a received voice message, or
// live call audio) and answers "has there been voice here in the last N minutes?"
// This is what arms the PostureEngine: the posture behavior only runs in the
// inbox or a conversation that's been voice-active recently, and stands down
// everywhere else. A lapse timer re-evaluates when the window passes so arming
// drops on its own.
//
// Process-wide singleton: voice arrives deep in the inbox/dialog plumbing, and
// the controller that arms the engine lives at the window level — a singleton is
// the simplest seam between them.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QHash>
#include <QString>
#include <QTimer>

namespace SnapClone::UI {

class VoiceActivityTracker : public QObject {
    Q_OBJECT
public:
    static VoiceActivityTracker &instance();

    /// Record voice activity for a conversation (call on inbound voice / call
    /// audio). Resets that conversation's recency window to now.
    void noteVoice(const QString &conversationId);

    [[nodiscard]] bool isActive(const QString &conversationId) const;
    [[nodiscard]] bool anyActive() const;

    [[nodiscard]] int windowMs() const { return m_windowMs; }
    void setWindowMs(int ms);

Q_SIGNALS:
    /// The active set may have changed — a new voice arrived, or a window lapsed.
    /// Consumers should re-query isActive()/anyActive().
    void activityChanged();

private:
    explicit VoiceActivityTracker(QObject *parent = nullptr);
    void rescheduleLapse();

    QHash<QString, qint64> m_lastVoiceMs;   // conversationId → epoch ms of last voice
    int    m_windowMs = 5 * 60 * 1000;      // 5 minutes
    QTimer m_lapseTimer;                    // fires when the soonest entry lapses
};

} // namespace SnapClone::UI
