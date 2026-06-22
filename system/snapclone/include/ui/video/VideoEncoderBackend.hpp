#pragma once
// ─── ui/video/VideoEncoderBackend.hpp ────────────────────────────────────────
//
// Backend abstraction for video re-encoding. The transcoder picks one of
// these at runtime based on what's installed and what the user prefers.
//
// Current backends:
//   • GstSvtAv1Backend       — gst-launch + svtav1enc (current default)
//   • GstVp9Backend          — gst-launch + vp9enc (fallback)
//   • GstVp8Backend          — gst-launch + vp8enc (last-ditch)
//   • FfmpegLibav1Backend    — ffmpeg-cli + libsvtav1 (alt path, currently
//                               experimental; ffmpeg's libsvtav1 wrapper
//                               supports newer SVT-AV1 features that the
//                               gstreamer plugin lags behind on)
//   • ExternalCodecBackend   — placeholder for a custom codec binary
//                               currently in development; falls back to
//                               GstSvtAv1Backend when the binary isn't
//                               available
//
// The interface is intentionally narrow: a backend either knows how to
// build a command line for some external process (gst-launch, ffmpeg, or
// the future codec binary) given the input/output paths and quality knobs,
// OR it declares itself unavailable. The transcoder owns the QProcess
// lifecycle uniformly; backends just supply args.
//
// Why an interface instead of switch-on-enum: each backend has its own
// quality knob mappings, its own caps-filter requirements, its own
// stderr-progress-parse pattern. Putting all of that in a single function
// turns into a maze. With an interface each backend's logic stays in one
// file/struct and the transcoder is just a process runner.
//
// Why command-line external processes instead of linking libav directly:
//   • Memory isolation — encoder crashes don't take down the app
//   • Easier to swap libraries without rebuild
//   • The future codec binary will run as an external process anyway
//   • libavformat/libavcodec C ABI churn between major versions hurts
//
// ─────────────────────────────────────────────────────────────────────────────

#include "core/EncodeTier.hpp"

#include <QString>
#include <QStringList>
#include <QRegularExpression>
#include <memory>

namespace SnapClone::UI::Video {

// Pull EncodeTier into this namespace as a typing convenience so existing
// call sites don't have to say SnapClone::Core::Media::EncodeTier.
//
// Important: we do NOT redefine tierName() in this namespace. ADL would
// then find BOTH ::Core::Media::tierName and ::UI::Video::tierName when
// the argument type is EncodeTier (which resolves to the Core type via
// the alias), producing an ambiguous-overload compile error. Callers that
// need the name string should write `Core::Media::tierName(t)` explicitly.
using EncodeTier = SnapClone::Core::Media::EncodeTier;

/// Spec for a single transcode invocation. The transcoder fills this in
/// from caller args; the backend reads it to build its command line.
struct EncodeSpec {
    QString    inputPath;
    QString    outputPath;
    EncodeTier tier         = EncodeTier::Standard;
    int        targetWidth  = 0;     ///< 0 = source width; backend may clamp
    int        bitrateKbps  = 0;     ///< 0 = backend picks from tier
    bool       hasAudio     = true;  ///< from gst-discoverer probe

    /// V32.34+ face/saliency ROI boost. When valid (w>0 && h>0), the salient
    /// region (typically the face/subject) detected on a representative frame,
    /// in SOURCE pixel coordinates. The ffmpeg backend turns this into a
    /// static `addroi` filter so the encoder spends more bits inside the box
    /// (sharper face) and compresses the background harder. All-zero = no ROI
    /// (the saliency probe failed, was disabled, or the tier doesn't warrant
    /// it) → backends skip the addroi filter entirely. Only the ffmpeg backend
    /// honors it (gst-launch has no addroi equivalent).
    int        roiX = 0;
    int        roiY = 0;
    int        roiW = 0;             ///< 0 = no ROI
    int        roiH = 0;
    /// V32.34+ frame dims (source/display pixels) the ROI plan clamps its
    /// background-penalty + padded-face-boost rectangles against. 0 = unknown
    /// (the planner falls back to the ROI's own extent). Filled by the
    /// transcoder alongside roi* from the same probed display resolution.
    int        roiFrameW = 0;
    int        roiFrameH = 0;
};

/// Result of "can this backend run right now?" — used by the transcoder
/// to pick the first available backend in priority order.
struct BackendAvailability {
    bool    available = false;
    QString reason;       ///< Empty if available; human-readable why-not otherwise
};

/// Backend interface. Each concrete backend implements:
///   • name()           — display string for logs ("svtav1enc", "ffmpeg-libsvtav1")
///   • checkAvailable() — probe binaries / elements at startup
///   • binary()         — the program to exec (gst-launch-1.0, ffmpeg, etc.)
///   • buildArgs(spec)  — derive the argv list from an EncodeSpec
///   • progressPattern() — regex that pulls 0-100 from stderr (or empty if
///                         the backend doesn't emit progress and the caller
///                         should fall back to size-based estimation)
class IVideoEncoderBackend
{
public:
    virtual ~IVideoEncoderBackend() = default;

    /// V32.32: how the encoder uses the host CPU/GPU/dedicated hw.
    /// Used by the UI to surface an indicator during transcode so the
    /// user knows the system is working hard, and on which silicon.
    /// Default Cpu — backends that use the GPU or a dedicated encoder
    /// block override this.
    enum class Kind {
        Cpu,       ///< Software encoder running on the main CPU
        Gpu,       ///< GPU compute / shader-based encoder (Vulkan, OpenCL)
        Hardware,  ///< Dedicated video encoder block (Venus / VAAPI / nvenc / qsv)
    };

    [[nodiscard]] virtual QString name() const = 0;
    [[nodiscard]] virtual Kind kind() const { return Kind::Cpu; }
    [[nodiscard]] virtual BackendAvailability checkAvailable() const = 0;
    [[nodiscard]] virtual QString binary() const = 0;
    [[nodiscard]] virtual QStringList buildArgs(const EncodeSpec &spec) const = 0;

    /// The container/extension this backend's output MUST use, WITHOUT the dot
    /// (e.g. "webm", "mkv", "mp4"). The transcoder uses this to choose the
    /// output path's extension so the muxer and codec are compatible — e.g. AV1
    /// goes in webm, but H.266/VVC cannot (webm only carries VP8/VP9/AV1), so a
    /// VVC backend returns "mkv". Defaults to "webm" to preserve the existing
    /// behavior for every current backend.
    [[nodiscard]] virtual QString outputContainer() const {
        return QStringLiteral("webm");
    }

    /// Regex for parsing per-line progress messages from the encoder's
    /// stderr. The first capture group must be the percent (0-100).
    /// Returns an empty regex if the backend doesn't emit progress and
    /// the caller should rely on output-size-growth estimation instead.
    [[nodiscard]] virtual QRegularExpression progressPattern() const {
        return QRegularExpression(); // default: no native progress
    }
};

/// Returns the priority-ordered list of backends to try, picking the
/// first one whose checkAvailable() returns true. Order:
///
///   1. ExternalCodec  (your in-development codec — fast path when ready)
///   2. FFmpegLibSvtAv1 (newer AV1 features; production-grade)
///   3. GstSvtAv1      (current default; widely available)
///   4. GstVp9         (fallback)
///   5. GstVp8         (last-ditch)
///
/// Each backend's checkAvailable() probes the relevant binary/element.
/// The transcoder picks the first one that works, then sticks with it
/// for the session (we don't re-probe on every encode — that adds
/// 100-500ms to each invocation).
[[nodiscard]] std::vector<std::unique_ptr<IVideoEncoderBackend>>
allBackendsInPriorityOrder();

/// V32.34+: true if a usable H.266/VVC DECODER is available for playback
/// (opt-in via SNAPCLONE_ENABLE_VVC + ffmpeg built with libvvdec). The viewer
/// uses this to decide whether a received VVC-in-mkv snap can be played, so it
/// can surface a clear "no VVC decoder on this device" message instead of a
/// blank/black video when the decoder is absent.
[[nodiscard]] bool vvcDecodeAvailable();

} // namespace SnapClone::UI::Video
