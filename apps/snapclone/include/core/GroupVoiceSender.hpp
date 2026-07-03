#ifndef SNAPCLONE_CORE_GROUPVOICESENDER_HPP
#define SNAPCLONE_CORE_GROUPVOICESENDER_HPP

#include <QObject>
#include <QByteArray>
#include <QElapsedTimer>

struct OpusEncoder; // Rust FFI handle (opaque)

namespace SnapClone::Core {

// Send half of group voice. While transmitting, it Opus-encodes each captured
// PCM frame and wraps it in a versioned wire frame (seq + timestamp + flags),
// then emits that packet. It is CRYPTO- AND TRANSPORT-AGNOSTIC: it emits the
// UNSEALED wire bytes and the channel layer is responsible for encrypting +
// sending (voice wants a lightweight per-frame AEAD with the group key, not the
// full signed/ratcheted message seal, which is too heavy for 50 fps audio).
// When not transmitting, frames are dropped.
//
// Wiring: GroupRadioSession::transmitStateChanged → setTransmitting;
//         GroupVoiceCapture::frameReady           → onFrameReady;
//         voicePacketReady                         → channel (seal + send).
//
// Sequence numbers restart at 0 each talk-spurt and the first frame is flagged
// TALKSPURT_START so the receiver resets its jitter buffer cleanly.
//
// Build: needs the snapclone-audio static lib built with the `opus` feature
// (-DSNAPCLONE_OPUS_CODEC=ON); without it the encoder is unavailable and frames
// are dropped with an encodeError.
class GroupVoiceSender : public QObject {
    Q_OBJECT
public:
    explicit GroupVoiceSender(int bitrateBitsPerSec = 16000, QObject *parent = nullptr);
    ~GroupVoiceSender() override;

    GroupVoiceSender(const GroupVoiceSender &) = delete;
    GroupVoiceSender &operator=(const GroupVoiceSender &) = delete;

    bool isTransmitting() const { return m_transmitting; }
    bool encoderReady() const { return m_encoderOk; }

public Q_SLOTS:
    void setTransmitting(bool on);             // ← transmitStateChanged
    void onFrameReady(const QByteArray &pcm);  // ← frameReady (one mono i16 frame)

Q_SIGNALS:
    void voicePacketReady(const QByteArray &wireFrame); // → channel: seal + send
    void encodeError(const QString &what);

private:
    OpusEncoder *m_encoder = nullptr;
    bool m_transmitting = false;
    bool m_talkspurtStart = false;
    quint32 m_seq = 0;
    QElapsedTimer m_clock;       // talk-spurt-relative timestamp (ms)
    QByteArray m_encodeScratch;  // reused Opus output buffer
    bool m_encoderOk = false;
};

} // namespace SnapClone::Core

#endif // SNAPCLONE_CORE_GROUPVOICESENDER_HPP
