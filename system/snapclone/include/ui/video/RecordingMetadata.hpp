#pragma once
// ─── ui/video/RecordingMetadata.hpp ──────────────────────────────────────────
//
// In-process metadata cache for files VideoRecorder produces. Lets
// VideoTranscoder skip the slow gst-discoverer-1.0 probe (~5s on
// mobile, blocking the GUI thread) when transcoding a file we
// recorded ourselves this session.
//
// V32.32: started with audio-presence detection — the single field
// that was costing us 5s per transcode. The class is named generic
// (`RecordingMetadata`) rather than `AudioPresenceCache` so future
// fields (capture rotation, source camera id, mic gain used,
// pipeline backend kind) can land here without renaming the
// abstraction.
//
// Scope: in-memory only. Lost on process restart. The common case
// (record → immediately transcode → send) lives entirely within one
// process lifetime, so this covers the hot path. Cross-session
// persistence (sidecar JSON manifest) is filed in TODO.md as a
// follow-up if it turns out to matter.
//
// Thread safety: protected by an internal QMutex. Callers can write
// from the recorder's QProcess::finished lambda and read from the
// transcoder's setup path without external locking.

#include <QString>
#include <QMutex>
#include <QHash>
#include <optional>

namespace SnapClone::UI::Video {

/// Stores side-channel facts about recording files so callers don't
/// have to re-derive them via subprocess probes.
class RecordingMetadata
{
public:
    /// Singleton accessor. Process-wide cache; not per-widget.
    static RecordingMetadata &instance();

    /// Record whether `path` contains an audio track. Called by
    /// VideoRecorder right after it decides whether to include the
    /// audio branch in its pipeline.
    void setHasAudio(const QString &path, bool hasAudio);

    /// Look up whether `path` has audio. Returns empty optional if
    /// the cache doesn't know — caller should fall back to whatever
    /// probe they had before (gst-discoverer-1.0, etc.).
    [[nodiscard]] std::optional<bool> hasAudio(const QString &path) const;

    /// Forget everything we know about `path`. Called when a file is
    /// deleted (post-transcode cleanup) so the cache doesn't pin
    /// memory for paths that no longer exist.
    void forget(const QString &path);

    /// Forget EVERYTHING. Used by tests and by the on-app-exit path
    /// to avoid leaking entries that referenced temp files cleaned up
    /// asynchronously.
    void clear();

private:
    RecordingMetadata() = default;
    mutable QMutex m_mu;
    QHash<QString, bool> m_audioByPath;
};

} // namespace SnapClone::UI::Video
