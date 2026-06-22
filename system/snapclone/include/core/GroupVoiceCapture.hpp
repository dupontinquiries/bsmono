#ifndef SNAPCLONE_CORE_GROUPVOICECAPTURE_HPP
#define SNAPCLONE_CORE_GROUPVOICECAPTURE_HPP

#include <QObject>
#include <QByteArray>
#include <QString>

class QAudioSource;
class QIODevice;
struct CaptureFramer;
struct VadDetector;
struct MicConditioner;  // V32.46: Rust mic boost + compressor (opaque)

namespace SnapClone::Core {

// Mic capture for group voice. Opens the default audio input at 16 kHz mono i16,
// re-chunks the stream into fixed 20 ms frames (via the Rust CaptureFramer),
// scores each frame for voice activity (the Rust VAD), and emits:
//   * frameReady    — one PCM frame, for the encoder/sender (when transmitting)
//   * voiceActivity — [0,1] likelihood, for GroupRadioSession's VOX gate
//   * micLevel      — [0,1] RMS, for a mic-level meter
//
// It does NOT decide whether to transmit and never touches the network or the
// codec — it just produces frames + telemetry. GroupRadioSession consumes
// voiceActivity to drive the gate/floor; a sender consumes frameReady to encode
// + seal + send only while transmitting. Decoupling capture from the transmit
// decision means the VAD keeps running (for the meter and VOX) even when we are
// not sending.
//
// Build: needs the snapclone-audio static lib (VAD + framer FFI) and
// Qt6::Multimedia.
class GroupVoiceCapture : public QObject {
    Q_OBJECT
public:
    explicit GroupVoiceCapture(QObject *parent = nullptr);
    ~GroupVoiceCapture() override;

    GroupVoiceCapture(const GroupVoiceCapture &) = delete;
    GroupVoiceCapture &operator=(const GroupVoiceCapture &) = delete;

    bool isCapturing() const { return m_capturing; }
    int frameSamples() const { return m_frameSamples; } // mono i16 per frame

public Q_SLOTS:
    void start();   // open the mic and begin producing frames
    void stop();    // close the mic

Q_SIGNALS:
    void frameReady(const QByteArray &pcm);   // one 20 ms mono i16 frame
    void voiceActivity(float likelihood);     // [0,1] VAD → GroupRadioSession
    void micLevel(float rms);                 // [0,1] mic meter
    void captureError(const QString &what);

private Q_SLOTS:
    void onReadyRead();

private:
    void drainFrames();

    QAudioSource *m_source = nullptr;
    QIODevice *m_io = nullptr;     // owned by m_source (pull-mode device)
    CaptureFramer *m_framer = nullptr;
    VadDetector *m_vad = nullptr;
    MicConditioner *m_conditioner = nullptr;  // V32.46: mic boost + compressor
    QByteArray m_pcmBytes;         // byte remainder across reads (odd trailing byte)
    QByteArray m_frameScratch;     // reused pop buffer (frameSamples * 2 bytes)
    int m_frameSamples = 0;
    bool m_capturing = false;
};

} // namespace SnapClone::Core

#endif // SNAPCLONE_CORE_GROUPVOICECAPTURE_HPP
