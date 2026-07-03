#pragma once
// ─── UI/ConversationSecurityBar.hpp ───────────────────────────────────────────
//
// A thin, calm banner for the top of a group conversation that states the
// security tier plainly and lets you ask for the private upgrade. It covers the
// whole conversation — messages, media, and voice all ride the same tier — so it
// lives above the thread rather than inside the voice bar (whose compact lock is
// the in-call mirror of this).
//
// Tiers mirror the negotiation in snapclone-audio's security.rs:
//   Standard  (Public)   — lightweight, always end-to-end encrypted
//   Securing  (await PQ) — both sides asked for private; handshake in flight
//   Private   (PQ ready) — post-quantum hybrid + playback obfuscation
//
// Pure view; the controller wires:
//   active mode + awaiting-PQ          → setTier
//   our own local request              → setPrivateRequested
// and back:
//   privateRequestToggled  → security negotiation (set local request)
//
// Only "Securing" animates (a quiet pulse while the handshake runs); Standard and
// Private are still. Requesting private when the peer hasn't yet keeps the tier
// Standard and just notes "awaiting the others".
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QString>
#include <QRectF>

class QTimer;

namespace SnapClone::UI {

class ConversationSecurityBar : public QWidget {
    Q_OBJECT
public:
    enum class Tier { Standard, Securing, Private };

    explicit ConversationSecurityBar(QWidget *parent = nullptr);
    ~ConversationSecurityBar() override;

    QSize sizeHint() const override;
    Tier tier() const { return m_tier; }
    bool privateRequested() const { return m_requested; }

public Q_SLOTS:
    void setTier(Tier tier);
    void setPrivateRequested(bool on); // reflect our local request (toggle state)

Q_SIGNALS:
    void privateRequestToggled(bool on);

protected:
    void paintEvent(QPaintEvent *) override;
    void mouseReleaseEvent(QMouseEvent *) override;

private:
    QRectF toggleRect() const;
    QRectF lockRect() const;
    void paintLockGlyph(QPainter &p, const QRectF &r, const QColor &c, bool closed) const;
    void paintToggle(QPainter &p);
    QColor tierColor() const;
    QString tierTitle() const;
    QString tierSubtitle() const;
    void ensurePulse();
    void onPulse();

    Tier m_tier = Tier::Standard;
    bool m_requested = false;
    float m_knob = 0.0f;   // toggle knob position 0..1, eased
    float m_phase = 0.0f;  // securing pulse phase
    QTimer *m_pulse = nullptr;
};

} // namespace SnapClone::UI
