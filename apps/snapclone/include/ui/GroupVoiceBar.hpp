#pragma once
// ─── UI/GroupVoiceBar.hpp ─────────────────────────────────────────────────────
//
// Calm on-air control surface for group voice. A single bar that surfaces the
// whole voice stack at a glance: who holds the floor, your live mic level, the
// security tier (public / securing / private), and a press-and-hold talk button.
//
// It is a PURE VIEW — it owns no audio or network objects. The controller wires
// it to the pieces already built:
//
//   GroupRadioSession::floorHolderChanged    → setTalker
//   GroupRadioSession::transmitStateChanged  → setOnAir
//   GroupVoiceCapture::micLevel              → setMicLevel
//   security mode / awaiting-PQ              → setSecurityState
// and back:
//   talkPressed / talkReleased  → GroupRadioSession::pressTalk / releaseTalk
//   privateModeRequested        → security negotiation (set local request)
//
// Interaction (intentional, calm — matching VISION):
//   * press-and-hold the round talk button to take the floor (half-duplex)
//   * tap the lock to request/withdraw private mode
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QString>
#include <QRectF>
#include <QPointF>

#include "ui/FrequentButton.hpp"

class QTimer;

namespace SnapClone::UI {

class GroupVoiceBar : public QWidget {
    Q_OBJECT
public:
    enum class Security { Public, Securing, Private };

    explicit GroupVoiceBar(QWidget *parent = nullptr);
    ~GroupVoiceBar() override;

    QSize sizeHint() const override;

    Security security() const { return m_security; }
    bool isOnAir() const { return m_onAir; }

public Q_SLOTS:
    /// Security tier shown by the lock (Securing = private agreed, handshaking).
    void setSecurityState(Security state);
    /// Who holds the floor. Empty name = floor open. `isSelf` styles it as "You".
    void setTalker(const QString &name, bool isSelf);
    /// Live mic level 0..1 (smoothed for calm motion).
    void setMicLevel(float level01);
    /// Whether we are currently transmitting (lights the talk button).
    void setOnAir(bool onAir);
    /// Whether the talk button is the primary action right now: grand in push-to-
    /// talk (you hold it constantly), comfortable in the auto modes (VOX /
    /// continuous) where it's rarely touched. (FrequentButton sizing.)
    void setTalkProminent(bool primary);

Q_SIGNALS:
    void talkPressed();
    void talkReleased();
    void privateModeRequested(bool enabled);

protected:
    void paintEvent(QPaintEvent *) override;
    void mousePressEvent(QMouseEvent *) override;
    void mouseMoveEvent(QMouseEvent *) override;
    void mouseReleaseEvent(QMouseEvent *) override;

private:
    QRectF lockRect() const;
    QRectF talkRect() const;
    QRectF centerRect() const;
    void paintCard(QPainter &p);
    void paintLock(QPainter &p);
    void paintCenter(QPainter &p);
    void paintTalk(QPainter &p);
    void paintMicGlyph(QPainter &p, const QRectF &r, const QColor &c) const;
    void paintLockGlyph(QPainter &p, const QRectF &r, const QColor &c, bool locked) const;
    void ensurePulse(bool running);
    void onPulse();

    Security m_security = Security::Public;
    QString m_talker;
    bool m_talkerIsSelf = false;
    float m_micLevel = 0.0f;
    float m_displayLevel = 0.0f; // eased toward m_micLevel for calm motion
    bool m_onAir = false;
    bool m_pttDown = false;
    // Swipe-to-lock: hold the talk button and swipe up to latch transmit on
    // (hands-free); while latched a tap releases it. m_pressPos anchors the swipe.
    bool m_talkLatched = false;
    QPointF m_pressPos;
    // Talk-button size factor (FrequentButton): grand in PTT, comfortable in the
    // auto modes. Set via setTalkProminent.
    double m_talkScale = FrequentButton::kGrandScale;
    float m_phase = 0.0f;        // pulse phase (securing / on-air glow)
    QTimer *m_pulse = nullptr;
};

} // namespace SnapClone::UI
