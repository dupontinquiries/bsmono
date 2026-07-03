// ─── core/VoiceNoteRecorder.hpp ──────────────────────────────────────────────
//
// Records a short ephemeral voice note. It reuses GroupVoiceCapture for the mic
// (same conditioner/gate as live voice), encodes each 20 ms mono frame to Opus,
// and accumulates a self-delimiting blob: a sequence of [u16-LE length][bytes]
// per frame. The blob travels in Snap::encryptedBlob with MediaType::Audio and
// is decoded on the receiving side by VoiceNotePlayer.
//
// Audio is only functional when the build carries the voice codec
// (SC_VOICE_CODEC); otherwise start() is a no-op and stop() yields an empty blob.
//
#pragma once

#include <QObject>
#include <QByteArray>

namespace SnapClone::Core {

class GroupVoiceCapture;

class VoiceNoteRecorder : public QObject {
    Q_OBJECT
public:
    explicit VoiceNoteRecorder(QObject *parent = nullptr);
    ~VoiceNoteRecorder() override;

    void start();

    /// Stop and finalize. Returns the encoded blob; if \a durationSecs is
    /// non-null it receives the rounded length in seconds (min 1). An empty
    /// blob means nothing was captured.
    QByteArray stop(int *durationSecs = nullptr);

    [[nodiscard]] bool isRecording() const { return m_recording; }
    [[nodiscard]] int  frameCount()  const { return m_frameCount; }

private slots:
    void onFrame(const QByteArray &pcm);

private:
    GroupVoiceCapture *m_capture    = nullptr;
    void              *m_enc        = nullptr;   // OpusEncoder* (opaque; FFI stays out of header)
    QByteArray         m_blob;
    int                m_frameCount = 0;
    bool               m_recording  = false;
};

} // namespace SnapClone::Core
