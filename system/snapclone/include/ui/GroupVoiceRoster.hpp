#pragma once
// ─── UI/GroupVoiceRoster.hpp ──────────────────────────────────────────────────
//
// The calm companion to GroupVoiceBar: who is in the voice channel and who holds
// the floor right now. The bar is the control (press to talk, the lock, your own
// level); the roster is the room (the other people, and who is speaking).
//
// It is a PURE VIEW — no audio or network objects. The controller wires it to the
// pieces already built:
//
//   channel membership                       → setParticipants
//   GroupRadioSession::floorHolderChanged    → setActiveSpeaker
//   the active speaker's level (your own mic, or a decoded peer level)
//                                            → setSpeakerLevel
// and back:
//   participantActivated  → open that contact
//
// Aesthetic (matching VISION + GroupVoiceBar): muted rows, one accent at a time.
// Only the active speaker is animated — a soft ring that breathes with their
// level — so a silent channel is perfectly still (and cheap on the FP5).
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QString>
#include <QList>
#include <QVector>

class QTimer;

namespace SnapClone::UI {

class GroupVoiceRoster : public QWidget {
    Q_OBJECT
public:
    struct Participant {
        QString id;
        QString name;
        bool isSelf = false;
    };

    explicit GroupVoiceRoster(QWidget *parent = nullptr);
    ~GroupVoiceRoster() override;

    QSize sizeHint() const override;
    int count() const { return m_people.size(); }
    QString activeSpeaker() const { return m_activeId; }

public Q_SLOTS:
    /// The people in the voice channel (order preserved; self can be anywhere).
    void setParticipants(const QList<Participant> &people);
    /// Who holds the floor. Empty id = floor open (nobody highlighted).
    void setActiveSpeaker(const QString &id);
    /// The active speaker's mic level 0..1 (drives the breathing ring; smoothed).
    void setSpeakerLevel(float level01);

Q_SIGNALS:
    void participantActivated(const QString &id);

protected:
    void paintEvent(QPaintEvent *) override;
    void mouseReleaseEvent(QMouseEvent *) override;

private:
    int rowHeight() const;
    int rowIndexAt(const QPointF &pos) const;
    void ensurePulse(bool running);
    void onPulse();
    void paintRow(QPainter &p, const Participant &person, const QRectF &row, bool active);
    void paintAvatar(QPainter &p, const QRectF &cell, const QString &name,
                     bool active, bool isSelf);

    QVector<Participant> m_people;
    QString m_activeId;
    float m_level = 0.0f;
    float m_displayLevel = 0.0f; // eased toward m_level for calm motion
    float m_phase = 0.0f;        // breathing phase for the active ring
    QTimer *m_pulse = nullptr;
};

} // namespace SnapClone::UI
