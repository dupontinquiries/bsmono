#include <QSettings>
#include "core/GroupVoiceReceiver.hpp"

#include <QAudioSink>
#include <QAudioFormat>
#include <QAudioDevice>
#include <QMediaDevices>
#include <QIODevice>
#include <QTimer>

extern "C" {
#include "audio/snapclone_audio_ffi.h"
}

namespace SnapClone::Core {

namespace {
constexpr int kSampleRate = 16000;
constexpr int kChannels = 1;
constexpr int kFrameMs = 20;
constexpr int kJitterTarget = 3;   // ~60 ms prime
constexpr int kJitterMax = 50;     // ~1 s cap
constexpr int kMaxDecodeSamples = 1920; // 120 ms @ 16k = largest Opus frame
constexpr int kMaxPayloadBytes = 4096;
// Playout buffering. The old design wrote exactly one 20 ms frame per 20 ms
// timer tick into the sink with no buffer headroom — but a software timer can't
// stay phase-locked to the device's sample clock, so any tick jitter starved the
// sink (underrun → crackle), independent of CPU governor. Fix: give the sink a
// real cushion (~kCushionFrames) and top it up to that cushion every tick, and
// run the tick at half a frame so it refills well before the cushion drains.
// kPlayoutGain is a modest clamped makeup boost for low decoded levels (the
// voice-profile AGC/normalization will supersede it).
constexpr int   kCushionFrames = 4;            // ~80 ms cushion (bounds latency)
constexpr float kPlayoutGain   = 1.5f;         // ~+3.5 dB baseline makeup

// Opus only encodes a fixed set of frame durations; snap a requested ms to the
// nearest legal one so a stray setting can't produce un-decodable frames.
int snapOpusFrameMs(int req)
{
    const int valid[] = {5, 10, 20, 40, 60};
    int best = 20, bestd = 1 << 30;
    for (int v : valid) {
        const int d = req > v ? req - v : v - req;
        if (d < bestd) { bestd = d; best = v; }
    }
    return best;
}
} // namespace

GroupVoiceReceiver::GroupVoiceReceiver(QObject *parent)
    : QObject(parent)
{
    // V32.49: latency knobs (defaults preserve current behaviour). Jitter depth
    // and cushion are receiver-only — each device sets its own play-out latency.
    // Frame size affects the wire, so for a call both ends should match it.
    QSettings vs;
    m_frameMs = snapOpusFrameMs(
        vs.value(QStringLiteral("audio/voiceFrameMs"), kFrameMs).toInt());
    m_jitterTarget = qBound(1, vs.value(QStringLiteral("audio/voiceJitterTarget"),
                                        kJitterTarget).toInt(), kJitterMax);
    m_jitterMax = qMax(m_jitterTarget,
                       vs.value(QStringLiteral("audio/voiceJitterMax"), kJitterMax).toInt());
    m_cushionFrames = qBound(1, vs.value(QStringLiteral("audio/voicePlayoutCushion"),
                                         kCushionFrames).toInt(), 16);
    m_playoutTickMs = qMax(1, m_frameMs / 2);

    // Probe Opus availability once (built with the opus feature, or not). The
    // per-peer jitter buffers + decoders are created lazily in streamFor() as
    // senders appear in the call.
    if (OpusDecoder *probe = sc_opus_decoder_new(kSampleRate, kChannels)) {
        m_decoderOk = true;
        sc_opus_decoder_free(probe);
    } else {
        m_decoderOk = false;
    }

    // V32.48: painful-sound limiter on the final play-out PCM, to protect the
    // listener from sudden bangs / feedback screech / sibilant spikes. On by
    // default; all knobs tunable on-device via QSettings.
    QSettings hs;
    if (hs.value(QStringLiteral("audio/harshGuard"), true).toBool()) {
        m_harshGuard = sc_harsh_guard_new(
            kSampleRate,
            hs.value(QStringLiteral("audio/harshPeakDb"),    -3.0f).toFloat(),
            hs.value(QStringLiteral("audio/harshCrest"),      6.0f).toFloat(),
            hs.value(QStringLiteral("audio/harshHfRatio"),    0.55f).toFloat(),
            hs.value(QStringLiteral("audio/harshMaxAttenDb"), -12.0f).toFloat(),
            hs.value(QStringLiteral("audio/harshAttackMs"),   1.0f).toFloat(),
            hs.value(QStringLiteral("audio/harshReleaseMs"),  220.0f).toFloat());
    }

    // Play-out loudness normalizer — the real fix for quiet far ends. Unlike a
    // fixed make-up, it adaptively boosts quiet speech (up to a ceiling) and
    // eases loud speech down to one consistent target, with a tanh limiter so
    // the boost can never clip. On by default; knobs tunable via QSettings.
    if (hs.value(QStringLiteral("audio/outputAgc"), true).toBool()) {
        m_outputAgc = sc_output_agc_new(
            kSampleRate,
            hs.value(QStringLiteral("audio/agcTargetRms"),    0.25f).toFloat(),
            hs.value(QStringLiteral("audio/agcMaxGainDb"),    27.0f).toFloat(),
            hs.value(QStringLiteral("audio/agcMinGainDb"),    -6.0f).toFloat(),
            hs.value(QStringLiteral("audio/agcNoiseFloorDb"), -55.0f).toFloat(),
            hs.value(QStringLiteral("audio/agcAttackMs"),     10.0f).toFloat(),
            hs.value(QStringLiteral("audio/agcReleaseMs"),    300.0f).toFloat(),
            hs.value(QStringLiteral("audio/agcCeiling"),      0.97f).toFloat());
    }

    m_frameSamples = (kSampleRate * m_frameMs) / 1000;
    m_frameBuf.resize(kMaxPayloadBytes);
    m_pcmScratch.resize(kMaxDecodeSamples * 2);
    m_mixScratch.resize(kMaxDecodeSamples * 4); // i32 mix accumulator
    m_outScratch.resize(kMaxDecodeSamples * 2); // mixed i16 frame
    m_silence = QByteArray(m_frameSamples * 2, '\0');

    m_timer = new QTimer(this);
    m_timer->setInterval(m_playoutTickMs);
    m_timer->setTimerType(Qt::PreciseTimer);
    connect(m_timer, &QTimer::timeout, this, &GroupVoiceReceiver::onPlayoutTick);
}

GroupVoiceReceiver::~GroupVoiceReceiver()
{
    stop();
    clearStreams();
    if (m_obfuscator) sc_obfuscator_free(m_obfuscator);
    if (m_harshGuard) sc_harsh_guard_free(m_harshGuard);
    if (m_outputAgc) sc_output_agc_free(m_outputAgc);
}

void GroupVoiceReceiver::setObfuscation(bool enabled, quint64 seed, float intensity)
{
    if (m_obfuscator) {
        sc_obfuscator_free(m_obfuscator);
        m_obfuscator = nullptr;
    }
    if (enabled) {
        m_obfuscator = sc_obfuscator_new(seed, kSampleRate, intensity);
    }
}

void GroupVoiceReceiver::start()
{
    if (m_playing) return;
    if (!m_decoderOk && !m_warnedNoDecoder) {
        m_warnedNoDecoder = true;
        Q_EMIT decoderUnavailable();
    }

    QAudioFormat fmt;
    fmt.setSampleRate(kSampleRate);
    fmt.setChannelCount(kChannels);
    fmt.setSampleFormat(QAudioFormat::Int16);

    const QAudioDevice dev = QMediaDevices::defaultAudioOutput();
    if (dev.isNull()) {
        Q_EMIT playbackError(QStringLiteral("no audio output device"));
        return;
    }
    if (!dev.isFormatSupported(fmt)) {
        Q_EMIT playbackError(QStringLiteral("16 kHz mono int16 not supported by the output device"));
        return;
    }

    m_sink = new QAudioSink(dev, fmt, this);
    // Give the sink a real buffer so a late playout tick can't starve it.
    m_sink->setBufferSize(static_cast<qint64>(m_frameSamples) * 2 * m_cushionFrames);
    m_io = m_sink->start(); // push mode: write PCM to this QIODevice
    if (!m_io) {
        Q_EMIT playbackError(QStringLiteral("failed to start audio output"));
        m_sink->deleteLater();
        m_sink = nullptr;
        return;
    }
    m_sink->setVolume(1.0); // full digital volume; the system mixer still applies
    m_timer->start();
    m_playing = true;
}

void GroupVoiceReceiver::stop()
{
    if (m_timer) m_timer->stop();
    if (m_sink) {
        m_sink->stop();
        m_sink->deleteLater();
        m_sink = nullptr;
    }
    m_io = nullptr;
    clearStreams();
    m_playing = false;
}

void GroupVoiceReceiver::onVoiceFrame(const QString &senderId, const QByteArray &wireFrame)
{
    uint8_t ver = 0, codec = 0, flags = 0;
    uint32_t seq = 0, ts = 0;
    QByteArray payload(wireFrame.size(), Qt::Uninitialized); // payload <= wire size
    size_t plen = 0;

    const int rc = sc_voice_frame_decode(
        reinterpret_cast<const uint8_t *>(wireFrame.constData()),
        static_cast<size_t>(wireFrame.size()),
        &ver, &seq, &ts, &codec, &flags,
        reinterpret_cast<uint8_t *>(payload.data()),
        static_cast<size_t>(payload.size()), &plen);
    if (rc != 0) return; // malformed / unsupported version → drop

    // Route into this sender's own jitter buffer (created on first frame).
    PeerStream &st = streamFor(senderId);
    if (!st.jitter) return;
    if (flags & SC_VOICE_FLAG_TALKSPURT_START)
        sc_jitter_reset(st.jitter); // new burst restarts this sender's sequence
    sc_jitter_push(st.jitter, seq,
                   reinterpret_cast<const uint8_t *>(payload.constData()), plen);
}

void GroupVoiceReceiver::onPlayoutTick()
{
    if (!m_io || !m_sink) return;

    const size_t frameSamples = static_cast<size_t>(m_frameSamples);
    const qint64 frameBytes   = static_cast<qint64>(frameSamples) * 2;
    int16_t *pcm = reinterpret_cast<int16_t *>(m_pcmScratch.data());   // per-peer decode
    const size_t pcmCap = static_cast<size_t>(m_pcmScratch.size() / 2);
    int32_t *mix = reinterpret_cast<int32_t *>(m_mixScratch.data());   // i32 accumulator
    int16_t *out = reinterpret_cast<int16_t *>(m_outScratch.data());   // mixed i16 frame

    // Top the sink back up to its cushion (same bounded-refill policy as before:
    // write only while there's room so latency stays bounded, the guard stops a
    // free buffer from spinning). Each frame is now the SUM of every peer's
    // decoded stream — one jitter buffer + decoder per sender, mixed here.
    int guard = 0;
    while (m_sink->bytesFree() >= frameBytes && guard++ < m_cushionFrames + 2) {
        if (m_streams.empty()) {            // no senders yet → keep the sink fed
            writeSilence();
            continue;
        }

        for (size_t i = 0; i < frameSamples; ++i) mix[i] = 0;

        for (auto &entry : m_streams) {
            PeerStream &st = entry.second;
            if (!st.jitter) continue;
            size_t flen = 0;
            const int r = sc_jitter_pop(
                st.jitter, reinterpret_cast<uint8_t *>(m_frameBuf.data()),
                static_cast<size_t>(m_frameBuf.size()), &flen);

            int samples = 0;
            if (r == 0) {                   // frame ready → decode it
                if (m_decoderOk && st.decoder)
                    samples = sc_opus_decode(
                        st.decoder, reinterpret_cast<const uint8_t *>(m_frameBuf.constData()),
                        flen, pcm, pcmCap, 0);
            } else if (r == 1) {            // gap → packet-loss concealment
                if (m_decoderOk && st.decoder)
                    samples = sc_opus_decode(st.decoder, nullptr, 0, pcm, pcmCap, 0);
            }
            // r == 2 (silence / empty / error): this peer contributes nothing.

            const size_t n = qMin(static_cast<size_t>(samples < 0 ? 0 : samples), frameSamples);
            for (size_t i = 0; i < n; ++i) mix[i] += pcm[i];
        }

        // Collapse the i32 sum to i16. The clamp is the overflow safety; with
        // floor control there's normally a single talker so it rarely engages,
        // and the AGC / HarshGuard below shape the final level. For a 1-to-1 call
        // the sum is one stream, so this is a pass-through.
        for (size_t i = 0; i < frameSamples; ++i) {
            const int32_t v = mix[i];
            out[i] = static_cast<int16_t>(v < -32768 ? -32768 : (v > 32767 ? 32767 : v));
        }

        // Play-out conditioning on the mixed frame (the same chain as before,
        // applied once to the mix): private-tier perturbation, then loudness
        // normalization (or the legacy fixed make-up), then to the sink.
        if (m_obfuscator)
            sc_obfuscator_process(m_obfuscator, out, frameSamples);
        if (m_outputAgc) {
            if (m_harshGuard)
                sc_harsh_guard_process(m_harshGuard, out, frameSamples);
            sc_output_agc_process(m_outputAgc, out, frameSamples);
        } else {
            const float g = kPlayoutGain;
            for (size_t i = 0; i < frameSamples; ++i) {
                float x = out[i] * (1.0f / 32768.0f);
                if (x >  1.0f) x =  1.0f;
                else if (x < -1.0f) x = -1.0f;
                const float y = g * x - (g - 1.0f) * x * x * x;
                const int iv = static_cast<int>(y * 32767.0f);
                out[i] = static_cast<int16_t>(iv < -32768 ? -32768 : (iv > 32767 ? 32767 : iv));
            }
            if (m_harshGuard)
                sc_harsh_guard_process(m_harshGuard, out, frameSamples);
        }
        m_io->write(reinterpret_cast<const char *>(out), frameBytes);
    }
}

void GroupVoiceReceiver::writeSilence()
{
    if (m_io) {
        // one nominal frame of zeros keeps the sink fed and play-out steady
        m_io->write(m_silence.constData(), m_silence.size());
    }
}

GroupVoiceReceiver::PeerStream &GroupVoiceReceiver::streamFor(const QString &senderId)
{
    const std::string key = senderId.toStdString();
    auto it = m_streams.find(key);
    if (it != m_streams.end())
        return it->second;
    PeerStream st;
    st.jitter  = sc_jitter_new(m_jitterTarget, m_jitterMax);
    st.decoder = m_decoderOk ? sc_opus_decoder_new(kSampleRate, kChannels) : nullptr;
    return m_streams.emplace(key, st).first->second;
}

void GroupVoiceReceiver::clearStreams()
{
    for (auto &entry : m_streams) {
        if (entry.second.jitter)  sc_jitter_free(entry.second.jitter);
        if (entry.second.decoder) sc_opus_decoder_free(entry.second.decoder);
    }
    m_streams.clear();
}

} // namespace SnapClone::Core
