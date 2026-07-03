#include "core/GroupVoiceCapture.hpp"

#include <QAudioSource>
#include <QAudioFormat>
#include <QAudioDevice>
#include <QMediaDevices>
#include <QDebug>
#include <QIODevice>
#include <QSettings>

extern "C" {
#include "audio/snapclone_audio_ffi.h"
}

namespace SnapClone::Core {

namespace {
constexpr int kSampleRate = 16000; // Opus wideband; matches the VAD
constexpr int kFrameMs = 20;       // VOIP frame length (default)

// Opus encodes only a fixed set of frame durations; snap to the nearest legal
// one. (Mirrors the receiver — kept local so each TU is self-contained.)
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

GroupVoiceCapture::GroupVoiceCapture(QObject *parent)
    : QObject(parent)
{
    // V32.49: frame size is a latency knob (audio/voiceFrameMs). Smaller frames
    // cut every frame-denominated buffer but add packet/header overhead. Both
    // call ends should use the same value. Default preserves 20 ms.
    QSettings vs;
    const int frameMs = snapOpusFrameMs(
        vs.value(QStringLiteral("audio/voiceFrameMs"), kFrameMs).toInt());
    m_framer = sc_capture_framer_new(kSampleRate, frameMs);
    m_vad = sc_audio_vad_new(kSampleRate);
    m_frameSamples = m_framer ? sc_capture_framer_frame_samples(m_framer) : 0;
    if (m_frameSamples > 0)
        m_frameScratch.resize(m_frameSamples * 2); // 2 bytes per i16 sample

    // V32.46: mic boost + compressor. The Fairphone 5 mics are very quiet, so
    // this is on by default; all knobs are tunable on-device via QSettings so
    // gain/threshold can be dialed in without a rebuild. Defaults: +9 dB,
    // -12 dBFS, 1.5:1, 0 ms attack, 100 ms release.
    QSettings cs;
    if (cs.value(QStringLiteral("audio/micConditioner"), true).toBool()) {
        m_conditioner = sc_mic_conditioner_new(
            kSampleRate,
            cs.value(QStringLiteral("audio/micGainDb"),      15.0f).toFloat(),
            cs.value(QStringLiteral("audio/micThresholdDb"), -12.0f).toFloat(),
            cs.value(QStringLiteral("audio/micRatio"),       1.5f).toFloat(),
            cs.value(QStringLiteral("audio/micAttackMs"),    0.0f).toFloat(),
            cs.value(QStringLiteral("audio/micReleaseMs"),   100.0f).toFloat());
    }
}

GroupVoiceCapture::~GroupVoiceCapture()
{
    stop();
    if (m_framer) sc_capture_framer_free(m_framer);
    if (m_vad)    sc_audio_vad_free(m_vad);
    if (m_conditioner) sc_mic_conditioner_free(m_conditioner);
}

void GroupVoiceCapture::start()
{
    if (m_capturing) return;

    // App-wide microphone kill-switch (Sounds settings). When on, the whole
    // capture pipeline stays down — you can still hear a call, but the mic is
    // never opened. Checked at every start so toggling it off then placing a new
    // call is enough to re-enable; toggling on takes effect on the next call.
    {
        QSettings s;
        if (s.value(QStringLiteral("sounds/micPipelineOff"), false).toBool()) {
            Q_EMIT captureError(QStringLiteral("microphone disabled in settings"));
            return;
        }
    }

    QAudioFormat fmt;
    fmt.setSampleRate(kSampleRate);
    fmt.setChannelCount(1);
    fmt.setSampleFormat(QAudioFormat::Int16);

    // Pick the input device. An explicit override (audio/inputDevice, matched by
    // description substring) wins; otherwise the system default. On PipeWire the
    // "default" can resolve to a monitor/effect node that doesn't actually
    // capture, so the override is the user's escape hatch.
    QAudioDevice dev = QMediaDevices::defaultAudioInput();
    {
        QSettings s;
        const QString want = s.value(QStringLiteral("audio/inputDevice")).toString();
        if (!want.isEmpty()) {
            const auto inputs = QMediaDevices::audioInputs();
            for (const QAudioDevice &d : inputs) {
                if (d.description().contains(want, Qt::CaseInsensitive)) { dev = d; break; }
            }
        }
    }
    if (dev.isNull()) {
        Q_EMIT captureError(QStringLiteral("no audio input device"));
        return;
    }

    // Do NOT hard-fail on isFormatSupported. On PipeWire/PulseAudio the device
    // commonly advertises only its native rate (48 kHz) while the backend
    // transparently resamples our 16 kHz request — so this check is a false
    // negative that was silently killing capture even though the mic works in
    // every other app. Warn and try anyway; if the open truly fails, start()
    // below returns null and we surface the error then.
    const bool nativelySupported = dev.isFormatSupported(fmt);
    qInfo() << "[voice] opening mic input:" << dev.description()
            << "— native 16kHz/mono/int16 support:" << nativelySupported
            << "(resampling backend will adapt if false)";

    m_source = new QAudioSource(dev, fmt, this);

    m_pcmBytes.clear();
    if (m_framer) sc_capture_framer_reset(m_framer);
    if (m_conditioner) sc_mic_conditioner_reset(m_conditioner);

    m_io = m_source->start(); // pull mode: returns a QIODevice to read from
    if (!m_io) {
        Q_EMIT captureError(QStringLiteral("failed to start audio capture"));
        m_source->deleteLater();
        m_source = nullptr;
        return;
    }
    connect(m_io, &QIODevice::readyRead, this, &GroupVoiceCapture::onReadyRead);
    m_capturing = true;
}

void GroupVoiceCapture::stop()
{
    if (m_source) {
        m_source->stop();
        m_source->deleteLater();
        m_source = nullptr;
    }
    m_io = nullptr; // was owned by m_source
    m_pcmBytes.clear();
    m_capturing = false;
}

void GroupVoiceCapture::onReadyRead()
{
    if (!m_io || !m_framer) return;

    m_pcmBytes += m_io->readAll();
    const int nSamples = static_cast<int>(m_pcmBytes.size() / 2);
    if (nSamples > 0) {
        const int16_t *s = reinterpret_cast<const int16_t *>(m_pcmBytes.constData());
        sc_capture_framer_push(m_framer, s, static_cast<size_t>(nSamples));
        m_pcmBytes.remove(0, nSamples * 2); // keep any trailing odd byte for next read
    }
    drainFrames();
}

void GroupVoiceCapture::drainFrames()
{
    if (!m_framer || m_frameSamples <= 0) return;

    int16_t *out = reinterpret_cast<int16_t *>(m_frameScratch.data());
    const size_t cap = static_cast<size_t>(m_frameSamples);

    while (sc_capture_framer_pop(m_framer, out, cap) == 1) {
        // V32.46: boost + compress the quiet mic before it's metered, gated, or
        // encoded — so the VAD, the on-screen level, and the transmitted audio
        // all see the conditioned signal.
        if (m_conditioner) sc_mic_conditioner_process(m_conditioner, out, cap);
        const float vad = m_vad ? sc_audio_vad_process(m_vad, out, cap) : 0.0f;
        const float rms = sc_audio_frame_rms(out, cap);
        Q_EMIT voiceActivity(vad);
        Q_EMIT micLevel(rms);
        Q_EMIT frameReady(QByteArray(reinterpret_cast<const char *>(out),
                                     m_frameSamples * 2));
    }
}

} // namespace SnapClone::Core
