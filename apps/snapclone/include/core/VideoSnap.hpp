#pragma once
// ─── core/VideoSnap.hpp ───────────────────────────────────────────────────────
//
// VIDEO SNAP — FUTURE IMPLEMENTATION SCAFFOLD
//
// This file defines the type system and protocol contracts for video snap
// support. It is intentionally not yet wired into the build — all types are
// defined, documented, and kept consistent with the existing snap protocol
// so that the implementation path is clear and the decisions are recorded.
//
// WHY THIS FILE EXISTS NOW
// ────────────────────────
// The existing snap protocol (v3) was designed for images: a single blob
// encrypted in one shot, chunked over UDP or sent via WebSocket in one frame.
// Video changes four things fundamentally:
//
//   1. SIZE — A 10s video at 720p/2Mbps ≈ 2.5MB. The current chunk system
//      caps at kChunkSize(8KB) × kMaxChunks(4096) = 32MB, so the wire
//      format can handle it. But the UDP reassembly map holds all chunks in
//      RAM simultaneously — 32MB of QByteArray per peer is expensive on mobile.
//      Video needs streaming reassembly: write chunks to disk as they arrive,
//      not held in a QHash<int,QByteArray>.
//
//   2. ENCODING — Still images are JPEG, losslessly re-encrypted. Video must
//      be H.264/H.265 (or VP9/AV1 for open codecs). Qt's QMediaRecorder
//      outputs whatever the platform encoder produces. We need a format
//      negotiation step so sender and receiver agree on codec.
//
//   3. DURATION — Videos have a playback duration separate from the snap
//      view timer. The snap durationSecs field currently means "how long the
//      snap is displayed after opening". For video snaps, the display duration
//      IS the video length, capped at kMaxVideoSecs. We reuse durationSecs
//      for this, but add a separate encoded_duration_ms field.
//
//   4. THUMBNAIL — The inbox should show a frame preview before the snap is
//      opened. A JPEG keyframe (first I-frame) extracted at record time and
//      sent in the OFFER message lets the UI render a thumbnail without
//      downloading the full video blob.
//
// PROTOCOL EXTENSION (v3 wire format additions)
// ──────────────────────────────────────────────
// The v3 OFFER message gains two new optional fields:
//
//   Field 5 (uint8):  media_type_flags
//     bit 0: is_video     (0 = still image, 1 = video)
//     bit 1: has_audio    (0 = silent, 1 = has audio track)
//     bit 2: is_looping   (0 = plays once, 1 = loops like a story)
//     bits 3-7: reserved
//
//   Field 6 (uint32): encoded_duration_ms
//     Actual encoded video duration in milliseconds (0 for images).
//     Used to display a progress bar during playback.
//
//   Field 7 (uint16): thumbnail_len
//   Field 8 (bytes):  thumbnail_jpeg
//     JPEG thumbnail (scaled to 200px wide) for inbox preview rendering.
//     Sent inline in the OFFER so the recipient can render it before the
//     full blob is delivered. Max 20KB; if larger, sender omits it.
//
// These fields are appended after the existing OFFER payload and are absent
// in v2 and earlier, so old clients simply ignore them. New clients check
// content_hash length as a sentinel for the extended format.
//
// SENDER PIPELINE (future, not yet implemented)
// ──────────────────────────────────────────────
//   CameraWidget → QMediaRecorder (H.264, AAC)
//     → output file: ~/.local/share/SnapClone/tmp/recording_<ts>.mp4
//     → SnapComposerDialog shows preview via QMediaPlayer
//     → on Send: VideoSnapEncoder::prepare(filePath, recipientId)
//       → extract thumbnail (first keyframe via QMediaPlayer seekTo(0))
//       → encrypt blob with RustCryptoService::encryptFor
//       → call AppController::sendSnap with mediaType=Video
//
//   QMediaRecorder configuration (target):
//     Container:   MP4 (or MKV fallback)
//     Video codec: H.264 baseline profile, CRF 28 (≈ 1.5 Mbps @ 720p)
//     Audio codec: AAC-LC, 128kbps, 44.1kHz stereo
//     Max duration: kMaxVideoSecs (10s) enforced via QMediaRecorder::setDuration
//     Max size:    kMaxVideoBytesWire (8MB) — reject recording if exceeded
//
// RECEIVER PIPELINE (future, not yet implemented)
// ─────────────────────────────────────────────────
//   SnapRouter::handleV3Message (ANNOUNCE/DATA path):
//     → detect is_video from media_type_flags in OFFER
//     → allocate VideoReassemblyContext instead of QByteArray in-memory
//     → VideoReassemblyContext::onChunk writes each chunk to a tmp file
//       (no RAM accumulation beyond one chunk buffer)
//     → on completion: decrypt → verify hash → move to snap media dir
//     → signal inboundReady with Snap{mediaType=Video, localPath=...}
//
//   SnapViewerDialog (video path, future):
//     → detect mediaType == MediaType::Video
//     → create QMediaPlayer + QVideoWidget instead of QLabel
//     → seek to 0, autoplay on open
//     → enforce durationSecs view timer (same as image path)
//     → delete localPath file on timer expiry (same as image path)
//
// STORAGE SCHEMA ADDITIONS (migration 11, future)
// ─────────────────────────────────────────────────
//   ALTER TABLE snaps ADD COLUMN encoded_duration_ms INTEGER NOT NULL DEFAULT 0;
//   ALTER TABLE snaps ADD COLUMN thumbnail_path TEXT NOT NULL DEFAULT '';
//   ALTER TABLE snaps ADD COLUMN has_audio INTEGER NOT NULL DEFAULT 0;
//   ALTER TABLE snaps ADD COLUMN is_looping INTEGER NOT NULL DEFAULT 0;
//
// OPEN QUESTIONS / DECISIONS NEEDED
// ────────────────────────────────────
//   Q1: Codec negotiation — do we want VP9/AV1 (open, better compression)
//       or H.264 (universal hardware decode on all target devices)?
//       Current target devices: Asahi Linux (aarch64), Fairphone postmarketOS.
//       Both have hardware H.264 decode. H.264 baseline is the safe choice.
//       AV1 encode is CPU-heavy and slow on mobile. Suggest: H.264 now, AV1 later.
//
//   Q2: Max duration — 10s matches Snapchat's original limit and keeps blobs
//       under 3MB at 2Mbps. Stories (looping) need longer (60s?). Separate limit.
//
//   Q3: Audio default — record with audio by default? Users expect it but
//       some contexts (quiet library, work call) need silent option. Add a
//       mute toggle in SnapComposerDialog.
//
//   Q4: Streaming vs store-and-forward — for video we could stream chunks
//       as they're recorded (pre-buffer 1s, then stream). This reduces latency
//       but requires the recipient to buffer chunks while others are in-flight.
//       For now: store-and-forward (record complete, then send). Streaming later.
//
//   Q5: Re-transmission on view — image snaps support re-sending the blob if
//       the recipient sends a NEED. Video blobs are 2-8MB; re-sending on NEED
//       is expensive. Consider: only resend the first 1s (enough to verify
//       decryption), require recipient to request full re-send explicitly.
//
// ─────────────────────────────────────────────────────────────────────────────

#include "core/Types.hpp"
#include <QString>
#include <QByteArray>
#include <QDateTime>

namespace SnapClone::Core {

// ── Media type enum (extends existing Snap.mediaType) ────────────────────────
// Currently the DB stores mediaType as a string ("image/jpeg", "image/png").
// For video we add "video/mp4" and "video/webm".
// The wire format uses a uint8 flags field; the DB string is the canonical form.

enum class MediaType : quint8 {
    Image    = 0x00,  // existing — JPEG/PNG still frame
    Video    = 0x01,  // new — MP4/H.264 with optional audio
    // Reserved for future:
    // Audio   = 0x02,  // voice note (audio only)
    // Story   = 0x03,  // looping video (>10s, persistent, not ephemeral)
};

inline MediaType mediaTypeFromMimeString(const QString &mime)
{
    if (mime.startsWith(QStringLiteral("video/"))) return MediaType::Video;
    return MediaType::Image; // default
}

inline QString mimeStringFromMediaType(MediaType t, bool preferH264 = true)
{
    switch (t) {
    case MediaType::Video:
        return preferH264
            ? QStringLiteral("video/mp4")
            : QStringLiteral("video/webm");
    default:
        return QStringLiteral("image/jpeg");
    }
}

// ── Video-specific snap metadata ──────────────────────────────────────────────
// Carried in the v3 OFFER extended fields and persisted in the DB.
// Missing from old snaps → all fields default to zero/empty.

struct VideoSnapMeta {
    quint32    encodedDurationMs = 0;     // actual video length in ms
    bool       hasAudio          = false; // whether audio track is present
    bool       isLooping         = false; // story-style loop (future)
    QByteArray thumbnailJpeg;            // inline JPEG keyframe (≤ 20KB, may be empty)

    // Wire format helpers (v3 OFFER extension fields 5–8)
    // Serialise appends to an existing QDataStream after the standard OFFER fields.
    // Deserialise reads from the same stream; returns false on malformed data.
    static constexpr quint8  kFlagIsVideo  = 0x01;
    static constexpr quint8  kFlagHasAudio = 0x02;
    static constexpr quint8  kFlagLooping  = 0x04;
    static constexpr quint32 kMaxDurationMs = 10 * 60 * 1000; // 10 min hard cap
    static constexpr quint16 kMaxThumbBytes = 20 * 1024;       // 20 KB thumbnail cap

    [[nodiscard]] quint8 flags() const {
        quint8 f = kFlagIsVideo; // always set when this struct is used
        if (hasAudio)   f |= kFlagHasAudio;
        if (isLooping)  f |= kFlagLooping;
        return f;
    }

    [[nodiscard]] bool isValid() const {
        return encodedDurationMs > 0
            && encodedDurationMs <= kMaxDurationMs
            && thumbnailJpeg.size() <= kMaxThumbBytes;
    }
};

// ── Encoder configuration (target quality profile) ────────────────────────────
struct VideoEncodeConfig {
    int     videoBitrateKbps  = 1500;  // 1.5 Mbps — adequate for 720p30
    int     audioBitrateKbps  = 128;   // AAC-LC standard
    int     maxDurationSecs   = 10;    // hard cut after 10s
    quint64 maxFileSizeBytes  = 8 * 1024 * 1024; // 8 MB wire size limit
    int     widthPx           = 720;   // output width (height scales)
    int     frameRate         = 30;    // fps
    bool    recordAudio       = true;
    QString preferredContainer = QStringLiteral("video/mp4");

    // Platform-appropriate defaults.
    // postmarketOS/Alpine: use libx264 software encode (no VA-API guaranteed).
    // Asahi Linux: try VA-API H.264 first, fall back to libx264.
    // Both platforms have hardware H.264 decode via libavcodec/GStreamer.
    static VideoEncodeConfig defaultConfig() { return {}; }
};

// ── Reassembly context (streaming to disk) ────────────────────────────────────
// Replaces the in-memory QHash<int,QByteArray> for video chunk reassembly.
// Chunks are written to a temp file as they arrive; no full-blob RAM accumulation.
// Future implementation — not yet wired.

struct VideoReassemblyContext {
    QString   tempPath;         // e.g. ~/.local/.../tmp/recv_<snapId>.mp4.part
    QByteArray contentHash;     // SHA-256 of full plaintext — verified on completion
    quint32    totalChunks = 0;
    quint32    receivedChunks = 0;
    QSet<int>  receivedSet;     // which chunk indices have arrived

    [[nodiscard]] bool isComplete() const {
        return totalChunks > 0 && receivedChunks == totalChunks;
    }
    [[nodiscard]] qreal progress() const {
        return (totalChunks > 0)
            ? static_cast<qreal>(receivedChunks) / totalChunks
            : 0.0;
    }
};

} // namespace SnapClone::Core
