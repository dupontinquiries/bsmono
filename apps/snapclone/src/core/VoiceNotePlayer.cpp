// ─── core/VoiceNotePlayer.cpp ────────────────────────────────────────────────
#include "core/VoiceNotePlayer.hpp"

// Mirror the CMake backend guards so the player is always linkable and inert
// when the codec/Multimedia backend isn't built (QAudioSink + Opus FFI are only
// linked in a voice build).
#if defined(SNAPCLONE_RUST_GROUP) && defined(SNAPCLONE_RUST_AUDIO)
#  define SC_VOICE_CORE 1
#endif
#if defined(SC_VOICE_CORE) && defined(SNAPCLONE_OPUS_CODEC)
#  define SC_VOICE_CODEC 1
#endif

#ifdef SC_VOICE_CODEC
#include "audio/snapclone_audio_ffi.h"
#include <QAudioSink>
#include <QAudioFormat>
#endif

#include <QBuffer>
#include <QFile>
#include <QTimer>

namespace SnapClone::Core {

namespace {
constexpr int kSampleRate = 16000;
constexpr int kChannels   = 1;
} // namespace

VoiceNotePlayer &VoiceNotePlayer::instance()
{
    static VoiceNotePlayer s_instance;
    return s_instance;
}

VoiceNotePlayer::VoiceNotePlayer(QObject *parent) : QObject(parent) {}

VoiceNotePlayer::~VoiceNotePlayer()
{
    cleanupSink();
#ifdef SC_VOICE_CODEC
    if (m_dec)
        sc_opus_decoder_free(static_cast<OpusDecoder *>(m_dec));
#endif
}

void VoiceNotePlayer::enqueue(const QByteArray &blob, const QString &tag)
{
    m_queue.append(qMakePair(blob, tag));
    if (!m_playing)
        playNext();
}

void VoiceNotePlayer::stopAll()
{
    m_queue.clear();
    cleanupSink();
    m_playing = false;
}

QByteArray VoiceNotePlayer::resolveBytes(const QByteArray &blob, const QString &localPath)
{
    if (!blob.isEmpty()) return blob;
    if (localPath.isEmpty()) return {};
    QFile f(localPath);
    return f.open(QIODevice::ReadOnly) ? f.readAll() : QByteArray{};
}

void VoiceNotePlayer::cleanupSink()
{
    if (m_progressTimer) m_progressTimer->stop();
#ifdef SC_VOICE_CODEC
    if (m_sink) {
        m_sink->stop();
        m_sink->deleteLater();
        m_sink = nullptr;
    }
#endif
    if (m_buffer) {
        m_buffer->close();
        m_buffer->deleteLater();
        m_buffer = nullptr;
    }
}

QByteArray VoiceNotePlayer::decodeBlob(const QByteArray &blob)
{
    QByteArray pcm;
#ifdef SC_VOICE_CODEC
    if (!m_dec)
        m_dec = sc_opus_decoder_new(kSampleRate, kChannels);
    if (!m_dec) return pcm;

    const char *p = blob.constData();
    const qsizetype n = blob.size();
    qsizetype off = 0;
    int16_t out[4096];                      // 20 ms @16 kHz = 320 samples; generous cap
    while (off + 2 <= n) {
        const quint16 len =
            static_cast<unsigned char>(p[off]) |
            (static_cast<unsigned char>(p[off + 1]) << 8);
        off += 2;
        if (len == 0 || off + len > n) break;
        const int got = sc_opus_decode(
            static_cast<OpusDecoder *>(m_dec),
            reinterpret_cast<const unsigned char *>(p + off),
            static_cast<size_t>(len), out, sizeof(out) / sizeof(out[0]), 0);
        off += len;
        if (got > 0)
            pcm.append(reinterpret_cast<const char *>(out),
                       static_cast<qsizetype>(got) * 2);
    }
#else
    Q_UNUSED(blob);
#endif
    return pcm;
}

void VoiceNotePlayer::playNext()
{
    cleanupSink();

    if (m_queue.isEmpty()) {
        m_playing = false;
        return;
    }

    const auto item = m_queue.takeFirst();
    m_curTag = item.second;
    m_pcm    = decodeBlob(item.first);

    if (m_pcm.isEmpty()) {
        // Nothing to play (no codec, or empty/corrupt note) — report done and
        // move on so the queue never stalls.
        const QString tag = m_curTag;
        QTimer::singleShot(0, this, [this, tag]() {
            emit finished(tag);
            playNext();
        });
        return;
    }

    m_playing = true;
    m_playBaseMs = 0;   // fresh note starts at the buffer head

#ifdef SC_VOICE_CODEC
    QAudioFormat fmt;
    fmt.setSampleRate(kSampleRate);
    fmt.setChannelCount(kChannels);
    fmt.setSampleFormat(QAudioFormat::Int16);

    m_buffer = new QBuffer(this);
    m_buffer->setData(m_pcm);
    m_buffer->open(QIODevice::ReadOnly);

    m_sink = new QAudioSink(fmt, this);
    connect(m_sink, &QAudioSink::stateChanged,
            this, &VoiceNotePlayer::onSinkState);
    m_sink->start(m_buffer);

    m_totalMs = static_cast<qint64>(m_pcm.size() / 2) * 1000 / kSampleRate;
    if (!m_progressTimer) {
        m_progressTimer = new QTimer(this);
        m_progressTimer->setInterval(80);
        connect(m_progressTimer, &QTimer::timeout, this, [this]() {
            if (!m_sink) return;
            const qint64 played = m_playBaseMs +
                static_cast<qint64>(m_sink->processedUSecs() / 1000);
            emit positionChanged(qMin(played, m_totalMs), m_totalMs, m_curTag);
        });
    }
    m_progressTimer->start();
#endif

    emit started(m_curTag);
}

void VoiceNotePlayer::seek(qint64 ms)
{
#ifdef SC_VOICE_CODEC
    if (!m_sink || !m_buffer || m_pcm.isEmpty() || m_totalMs <= 0) return;
    ms = qBound<qint64>(0, ms, m_totalMs);
    // Byte offset in the mono int16 PCM, aligned to a whole sample.
    qint64 off = (ms * kSampleRate / 1000) * 2;
    off = qBound<qint64>(0, off, static_cast<qint64>(m_pcm.size()));
    off &= ~static_cast<qint64>(1);

    // Restart the sink from the new buffer position. The guard stops the
    // stop()→state-change from being read as "note finished"; processedUSecs
    // resets to 0 on the new start, so we rebase the reported position to `ms`.
    m_seeking = true;
    m_sink->stop();
    m_buffer->seek(off);
    m_playBaseMs = ms;
    m_sink->start(m_buffer);
    m_seeking = false;

    if (m_progressTimer && !m_progressTimer->isActive()) m_progressTimer->start();
    emit positionChanged(ms, m_totalMs, m_curTag);
#else
    Q_UNUSED(ms);
#endif
}

void VoiceNotePlayer::onSinkState(QAudio::State s)
{
    if (m_seeking) return;   // ignore the stop/restart churn from a seek
    // Idle == the QIODevice was fully pulled (note finished). Advance on the
    // next loop turn to avoid re-entrancy inside the sink's signal.
    if (s == QAudio::IdleState) {
        const QString tag = m_curTag;
        QTimer::singleShot(0, this, [this, tag]() {
            emit finished(tag);
            playNext();
        });
    }
}

} // namespace SnapClone::Core
