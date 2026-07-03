#ifndef SNAPCLONE_CORE_GROUPRADIOSESSION_HPP
#define SNAPCLONE_CORE_GROUPRADIOSESSION_HPP

#include <QObject>
#include <QString>
#include <QElapsedTimer>
#include <cstdint>

#include "core/RadioTransmitLogic.hpp"

// Opaque Rust FFI handles — declared here, defined in the C headers included by
// the .cpp (keeps the C headers out of every TU that includes this one).
struct ScGroupFloorControl;
struct TransmitGate;

class QTimer;

namespace SnapClone::Core {

// Mirrors SC_RADIO_MODE_* / the Rust TalkMode enum.
enum class TalkMode { Off = 0, PushToTalk = 1, VoiceActivated = 2, Continuous = 3 };

// Drives one group voice channel's half-duplex push-to-talk. Owns the Rust
// floor controller (the group "talking stick") and the per-frame transmit gate,
// and turns local intent (PTT / VOX / continuous) plus peer floor announcements
// into a single answer — should the mic transmit right now — alongside the
// floor announcements we must broadcast and the "who's talking" state for UI.
//
// Transport-agnostic by design: it never touches the network. Peer events come
// IN via slots (onPeerFloorAssert / onPeerFloorRelease); the announcements we
// want broadcast go OUT via signals (floorAssertRequested / floorReleaseRequested);
// transmitStateChanged drives the audio encoder. The owner (AppController) wires
// those to the actual group transport and audio pipeline. This keeps the session
// unit-testable and out of the websocket include chain.
//
// Build: requires the snapclone-group and snapclone-audio static libs
// (-DSNAPCLONE_RUST_GROUP=ON) at link time.
class GroupRadioSession : public QObject {
    Q_OBJECT
public:
    // `selfId` is our sc-id; the floor uses it for the contention tiebreak.
    explicit GroupRadioSession(QString selfId, QObject *parent = nullptr);
    ~GroupRadioSession() override;

    GroupRadioSession(const GroupRadioSession &) = delete;
    GroupRadioSession &operator=(const GroupRadioSession &) = delete;

    TalkMode talkMode() const { return m_mode; }
    bool transmitting() const { return m_transmitting; }
    QString currentHolder() const { return m_holder; } // empty = floor open

public Q_SLOTS:
    void start();                            // begin the tick loop
    void stop();                             // tear down; release floor if held
    void setTalkMode(TalkMode mode);

    // Local intent.
    void pressTalk();                        // PTT down (or VOX/continuous "begin")
    void releaseTalk();                      // PTT up
    void onVoiceActivity(float likelihood);  // [0,1] VAD per audio frame (VOX)

    // Peer events arriving from the group transport.
    void onPeerFloorAssert(const QString &holder, quint64 claimTs);
    void onPeerFloorRelease(const QString &holder);

Q_SIGNALS:
    void transmitStateChanged(bool transmitting);                   // → mic encoder on/off
    void floorHolderChanged(const QString &holder);                 // → UI on-air / who's talking
    void floorAssertRequested(const QString &me, quint64 claimTs);  // → broadcast to peers
    void floorReleaseRequested(const QString &me);                  // → broadcast to peers

private:
    void evaluate();                 // one orchestration step (also the tick body)
    void setTransmitting(bool on);
    void refreshHolder();
    quint64 nowMs() const;           // monotonic ms since start()

    QString m_selfId;
    TalkMode m_mode = TalkMode::PushToTalk;
    bool m_pttHeld = false;
    float m_lastVad = 0.0f;
    bool m_transmitting = false;
    QString m_holder;                // cached holder, for change detection
    quint64 m_claimTs = 0;           // our wall-clock claim time while wanting the floor

    ScGroupFloorControl *m_floor = nullptr;
    TransmitGate *m_gate = nullptr;
    RadioTransmitLogic m_logic;
    QTimer *m_tick = nullptr;
    QElapsedTimer m_clock;
};

} // namespace SnapClone::Core

#endif // SNAPCLONE_CORE_GROUPRADIOSESSION_HPP
