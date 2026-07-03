// ─── core/VoiceNotePlayer.hpp ────────────────────────────────────────────────
//
// Sequential playback for ephemeral voice notes. A single shared instance owns
// the output, so notes never overlap: enqueue() appends to a FIFO and they play
// strictly in order, each starting only when the previous finishes. Decoding is
// done up-front (notes are short) and the PCM is pulled by a QAudioSink.
//
// Functional only with the voice codec (SC_VOICE_CODEC); otherwise decode yields
// nothing and enqueue() completes immediately with a finished() signal.
//
#pragma once

#include <QObject>
#include <QByteArray>
#include <QList>
#include <QPair>
#include <QString>
#include <QAudio>

class QAudioSink;
class QBuffer;
class QTimer;

namespace SnapClone::Core {

class VoiceNotePlayer : public QObject {
    Q_OBJECT
public:
    static VoiceNotePlayer &instance();

    /// Queue a note for playback. \a tag is echoed back in started()/finished()
    /// so callers (e.g. a feed tile) can reflect play state.
    void enqueue(const QByteArray &blob, const QString &tag = QString());

    /// Stop the current note and clear the queue.
    void stopAll();

    /// Seek the currently-playing note to an absolute position (ms). No-op if
    /// nothing is playing. The note is decoded in full up front, so this is a
    /// cheap reposition — no re-decode. Scrub-while-playing supported.
    void seek(qint64 ms);

    /// Resolve playable bytes for a snap: the in-memory blob if present, else
    /// read the note back from disk (localPath). Voice notes are persisted as a
    /// file and the DB row's blob is cleared to save RAM (same as image snaps),
    /// so after a reload the bytes live only on disk.
    static QByteArray resolveBytes(const QByteArray &blob, const QString &localPath);

    [[nodiscard]] bool    isPlaying()   const { return m_playing; }
    [[nodiscard]] QString currentTag()  const { return m_curTag; }
    [[nodiscard]] int     queued()      const { return static_cast<int>(m_queue.size()); }

signals:
    void started(QString tag);
    void finished(QString tag);
    /// Emitted ~12×/s while a note plays so a UI can drive a progress slider.
    void positionChanged(qint64 playedMs, qint64 totalMs, QString tag);

private slots:
    void onSinkState(QAudio::State s);

private:
    explicit VoiceNotePlayer(QObject *parent = nullptr);
    ~VoiceNotePlayer() override;

    void       playNext();
    void       cleanupSink();
    QByteArray decodeBlob(const QByteArray &blob);

    QList<QPair<QByteArray, QString>> m_queue;
    QAudioSink *m_sink   = nullptr;
    QBuffer    *m_buffer = nullptr;
    QTimer     *m_progressTimer = nullptr;   // drives positionChanged
    qint64      m_totalMs = 0;                  // duration of the current note
    qint64      m_playBaseMs = 0;   // absolute ms at the current buffer start (seek base)
    bool        m_seeking = false;  // guards onSinkState during a seek restart
    void       *m_dec    = nullptr;   // OpusDecoder* (opaque)
    QByteArray  m_pcm;                // decoded PCM for the current note
    QString     m_curTag;
    bool        m_playing = false;
};

} // namespace SnapClone::Core
