#ifndef SNAPCLONE_CORE_GROUPVOICERECEIVER_HPP
#define SNAPCLONE_CORE_GROUPVOICERECEIVER_HPP

#include <QObject>
#include <QByteArray>
#include <QString>

#include <string>
#include <unordered_map>

class QAudioSink;
class QIODevice;
class QTimer;
struct JitterBuffer; // Rust FFI handles (opaque)
struct OpusDecoder;
struct Obfuscator;
struct HarshGuard;   // V32.48: painful-sound limiter (opaque)
struct OutputAgc;    // play-out loudness normalizer (opaque)

namespace SnapClone::Core {

// Receive half of group voice. The channel opens an incoming (sealed) packet and
// hands the decoded wire bytes here via onVoiceFrame. We parse the wire frame,
// push it into the jitter buffer keyed by sequence, and a steady play-out timer
// pops one result per frame interval: decode + play, conceal (Opus PLC), or
// silence. Audio goes to a QAudioSink at 16 kHz mono i16. Crypto + transport live
// at the channel boundary (this class never touches the network or the keys).
//
// Wiring: channel (open) → onVoiceFrame; start()/stop() bracket an active call.
//
// Build: needs the snapclone-audio static lib built with the `opus` feature
// (-DSNAPCLONE_OPUS_CODEC=ON); without it nothing can be decoded and playback is
// silence (decoderUnavailable fires once).
class GroupVoiceReceiver : public QObject {
    Q_OBJECT
public:
    explicit GroupVoiceReceiver(QObject *parent = nullptr);
    ~GroupVoiceReceiver() override;

    GroupVoiceReceiver(const GroupVoiceReceiver &) = delete;
    GroupVoiceReceiver &operator=(const GroupVoiceReceiver &) = delete;

    bool isPlaying() const { return m_playing; }

public Q_SLOTS:
    void start();                                    // open the sink + start play-out
    void stop();                                     // stop + flush
    void onVoiceFrame(const QString &senderId, const QByteArray &wireFrame);  // ← channel (already opened)

    // Enable/disable playback obfuscation (private tier). The channel turns this
    // on when the security mode becomes Private, seeding from the session key so
    // the perturbation pattern is channel-specific. intensity 0..1.
    void setObfuscation(bool enabled, quint64 seed = 0, float intensity = 0.5f);

Q_SIGNALS:
    void playbackError(const QString &what);
    void decoderUnavailable(); // once, if built without the opus feature

private Q_SLOTS:
    void onPlayoutTick();

private:
    void writeSilence();

    // Per-sender decode: each peer in a group gets its own jitter buffer + Opus
    // decoder, because Opus decoder state is per-stream — feeding one decoder
    // interleaved frames from several senders scrambles the audio. onPlayoutTick
    // pops + decodes each stream and SUMS them into one play-out frame. For a
    // 1-to-1 call the map holds a single entry and the mix is a pass-through.
    struct PeerStream {
        JitterBuffer *jitter  = nullptr;
        OpusDecoder  *decoder = nullptr;
    };
    PeerStream &streamFor(const QString &senderId);
    void clearStreams();

    std::unordered_map<std::string, PeerStream> m_streams; // keyed by sender id
    Obfuscator *m_obfuscator = nullptr; // playback obfuscation (private tier; null = off)
    HarshGuard *m_harshGuard = nullptr; // V32.48: painful-sound ducker (null = off)
    OutputAgc  *m_outputAgc  = nullptr; // play-out loudness normalizer (null = off)
    QAudioSink *m_sink = nullptr;
    QIODevice *m_io = nullptr;   // owned by m_sink (push mode)
    QTimer *m_timer = nullptr;

    QByteArray m_frameBuf;   // reused jitter-pop output (encoded payload)
    QByteArray m_pcmScratch; // reused decode output (PCM bytes), per peer
    QByteArray m_mixScratch; // i32 mix accumulator summed across peers
    QByteArray m_outScratch; // final mixed i16 frame written to the sink
    QByteArray m_silence;    // a frame of zeros for gap fill
    int m_frameSamples = 0;  // nominal frame (320 @ 16k/20 ms)
    // V32.49: latency knobs, read from QSettings in the ctor (defaults below
    // preserve current behaviour). Jitter depth + cushion are receiver-only —
    // each device dials its own play-out latency, no coordination needed.
    int m_frameMs = 20;        // audio/voiceFrameMs   (snapped to a valid Opus size)
    int m_jitterTarget = 3;    // audio/voiceJitterTarget
    int m_jitterMax = 50;      // audio/voiceJitterMax
    int m_cushionFrames = 4;   // audio/voicePlayoutCushion
    int m_playoutTickMs = 10;  // derived = m_frameMs / 2
    bool m_playing = false;
    bool m_decoderOk = false;
    bool m_warnedNoDecoder = false;
};

} // namespace SnapClone::Core

#endif // SNAPCLONE_CORE_GROUPVOICERECEIVER_HPP
