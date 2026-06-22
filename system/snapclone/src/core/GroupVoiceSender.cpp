#include "core/GroupVoiceSender.hpp"

#include <QSettings>

extern "C" {
#include "audio/snapclone_audio_ffi.h"
}

namespace SnapClone::Core {

namespace {
constexpr int kSampleRate = 16000;
constexpr int kChannels = 1;
constexpr int kMaxPacketBytes = 4000; // generous; ~5 kbps 20 ms frames are tiny
} // namespace

GroupVoiceSender::GroupVoiceSender(int bitrateBitsPerSec, QObject *parent)
    : QObject(parent)
{
    m_encoder = sc_opus_encoder_new(kSampleRate, kChannels, bitrateBitsPerSec);
    m_encoderOk = (m_encoder != nullptr);
    m_encodeScratch.resize(kMaxPacketBytes);

    if (m_encoder) {
        // Frugal + fast by default; a high-quality mode trades bitrate and CPU
        // for fidelity + loss-robustness. Each lever is individually overridable
        // in QSettings — the mode just picks the defaults. At our 16 kHz capture
        // the effective bandwidth ceiling is wideband, so the HQ mode mainly
        // raises bitrate/complexity and turns on FEC; narrowband (0) / mediumband
        // (1) are available via audio/voiceMaxBandwidth for an ultra-lean
        // "telephone" feel. DTX (near-zero output during silence) is on for both.
        QSettings s;
        const bool hq = s.value(QStringLiteral("audio/voiceHighQuality"), false).toBool();
        const int bitrate    = s.value(QStringLiteral("audio/voiceBitrate"),      hq ? 32000 : 14000).toInt();
        const int complexity = s.value(QStringLiteral("audio/voiceComplexity"),   hq ? 10 : 5).toInt();
        const int maxBw      = s.value(QStringLiteral("audio/voiceMaxBandwidth"), 2).toInt(); // 2 = wideband
        const int dtx        = s.value(QStringLiteral("audio/voiceDtx"),          1).toInt();
        const int fec        = s.value(QStringLiteral("audio/voiceFec"),          hq ? 1 : 0).toInt();
        const int lossPct    = s.value(QStringLiteral("audio/voiceFecLossPct"),   10).toInt();
        sc_opus_encoder_apply_quality(m_encoder, bitrate, complexity, maxBw, dtx, fec, lossPct);
    }
}

GroupVoiceSender::~GroupVoiceSender()
{
    if (m_encoder) sc_opus_encoder_free(m_encoder);
}

void GroupVoiceSender::setTransmitting(bool on)
{
    if (on == m_transmitting) return;
    m_transmitting = on;
    if (on) {
        // new talk-spurt: restart sequence + clock, flag the first frame
        m_seq = 0;
        m_talkspurtStart = true;
        m_clock.restart();
        if (!m_encoderOk) {
            Q_EMIT encodeError(QStringLiteral(
                "Opus encoder unavailable — build the audio crate with "
                "-DSNAPCLONE_OPUS_CODEC=ON (needs libopus)"));
        }
    }
}

void GroupVoiceSender::onFrameReady(const QByteArray &pcm)
{
    if (!m_transmitting || !m_encoderOk) return;

    const int nSamples = static_cast<int>(pcm.size() / 2);
    if (nSamples <= 0) return;
    const int16_t *samples = reinterpret_cast<const int16_t *>(pcm.constData());

    const int bytes = sc_opus_encode(
        m_encoder, samples, static_cast<size_t>(nSamples),
        reinterpret_cast<uint8_t *>(m_encodeScratch.data()),
        static_cast<size_t>(m_encodeScratch.size()));
    if (bytes <= 0) {
        Q_EMIT encodeError(QStringLiteral("Opus encode failed"));
        return;
    }

    const uint8_t flags =
        m_talkspurtStart ? SC_VOICE_FLAG_TALKSPURT_START : SC_VOICE_FLAG_NONE;
    m_talkspurtStart = false;
    const quint32 ts = static_cast<quint32>(m_clock.elapsed());

    QByteArray wire(bytes + 32, Qt::Uninitialized); // opus bytes + msgpack slack
    size_t wlen = 0;
    const int rc = sc_voice_frame_encode(
        m_seq, ts, SC_VOICE_CODEC_OPUS, flags,
        reinterpret_cast<const uint8_t *>(m_encodeScratch.constData()),
        static_cast<size_t>(bytes),
        reinterpret_cast<uint8_t *>(wire.data()),
        static_cast<size_t>(wire.size()), &wlen);
    if (rc != 0) {
        Q_EMIT encodeError(QStringLiteral("voice wire encode failed"));
        return;
    }
    wire.resize(static_cast<int>(wlen));
    ++m_seq;
    Q_EMIT voicePacketReady(wire);
}

} // namespace SnapClone::Core
