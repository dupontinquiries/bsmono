// ─── core/VoiceNoteRecorder.cpp ──────────────────────────────────────────────
#include "core/VoiceNoteRecorder.hpp"
#include "core/GroupVoiceCapture.hpp"

// Mirror the CMake backend guards (see GroupVoiceCoordinator) so this object is
// always compilable + linkable and simply inert when the voice backend / codec
// isn't built — that keeps callers (the conversation dialog) free of #ifdefs.
#if defined(SNAPCLONE_RUST_GROUP) && defined(SNAPCLONE_RUST_AUDIO)
#  define SC_VOICE_CORE 1
#endif
#if defined(SC_VOICE_CORE) && defined(SNAPCLONE_OPUS_CODEC)
#  define SC_VOICE_CODEC 1
#endif

#ifdef SC_VOICE_CODEC
#include "audio/snapclone_audio_ffi.h"
#endif

#include <QtGlobal>

namespace SnapClone::Core {

namespace {
constexpr uint32_t kSampleRate = 16000;
constexpr uint32_t kChannels   = 1;
constexpr int      kFrameMs     = 20;
constexpr int32_t  kBitrate     = 16000;   // bits/s — plenty for speech at 16 kHz
constexpr int      kMaxFrameOut = 4000;    // Opus packet cap for a 20 ms frame
} // namespace

VoiceNoteRecorder::VoiceNoteRecorder(QObject *parent) : QObject(parent) {}

VoiceNoteRecorder::~VoiceNoteRecorder()
{
    if (m_recording)
        stop(nullptr);
#ifdef SC_VOICE_CODEC
    if (m_enc)
        sc_opus_encoder_free(static_cast<OpusEncoder *>(m_enc));
#endif
}

void VoiceNoteRecorder::start()
{
    if (m_recording) return;
    m_blob.clear();
    m_frameCount = 0;

#ifdef SC_VOICE_CODEC
    if (!m_enc)
        m_enc = sc_opus_encoder_new(kSampleRate, kChannels, kBitrate);
    m_capture = new GroupVoiceCapture(this);
    connect(m_capture, &GroupVoiceCapture::frameReady,
            this, &VoiceNoteRecorder::onFrame);
    m_capture->start();
    m_recording = true;
#endif
}

void VoiceNoteRecorder::onFrame(const QByteArray &pcm)
{
#ifdef SC_VOICE_CODEC
    if (!m_enc || pcm.size() < 2) return;
    const int samples = pcm.size() / 2;
    unsigned char out[kMaxFrameOut];
    const int n = sc_opus_encode(
        static_cast<OpusEncoder *>(m_enc),
        reinterpret_cast<const int16_t *>(pcm.constData()),
        static_cast<size_t>(samples), out, sizeof(out));
    if (n > 0) {
        const quint16 len = static_cast<quint16>(n);
        m_blob.append(static_cast<char>(len & 0xFF));
        m_blob.append(static_cast<char>((len >> 8) & 0xFF));
        m_blob.append(reinterpret_cast<const char *>(out), n);
        ++m_frameCount;
    }
#else
    Q_UNUSED(pcm);
#endif
}

QByteArray VoiceNoteRecorder::stop(int *durationSecs)
{
    if (m_capture) {
        m_capture->stop();
        m_capture->deleteLater();
        m_capture = nullptr;
    }
    m_recording = false;
    if (durationSecs) {
        const int ms = m_frameCount * kFrameMs;
        *durationSecs = qMax(1, (ms + 999) / 1000);   // round up, floor at 1s
    }
    return m_blob;
}

} // namespace SnapClone::Core
