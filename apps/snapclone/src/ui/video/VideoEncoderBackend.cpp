#include "ui/video/VideoEncoderBackend.hpp"
#include "ui/video/Av1Policy.hpp"   // V32.34+: AV1 CRF/hierarchy/temporal-filter strategy
#include <QStandardPaths>
#include <QProcess>
#include <QFile>
#include <QLoggingCategory>
#ifdef SNAPCLONE_RUST_SALIENCY
#  include "saliency/snapclone_saliency_ffi.h"  // V32.34+: sc_roi_plan for ROI bit-allocation
#endif

Q_LOGGING_CATEGORY(lcEncoder, "snapclone.video.encoder")

namespace SnapClone::UI::Video {
namespace {

// ─── shared helpers ─────────────────────────────────────────────────────────

/// Probe a gstreamer element by spawning gst-inspect-1.0. Returns true iff
/// the element is installed AND introspects cleanly.
bool gstElementExists(const QString &name)
{
    QProcess p;
    p.start(QStringLiteral("gst-inspect-1.0"), { name });
    if (!p.waitForFinished(2000)) { p.kill(); return false; }
    return p.exitCode() == 0;
}

/// Probe whether an executable exists in PATH. We use this for ffmpeg and
/// for the external codec binary. QStandardPaths::findExecutable is
/// cross-platform and avoids `which`.
bool binaryExists(const QString &name)
{
    return !QStandardPaths::findExecutable(name).isEmpty();
}

/// Map an EncodeTier to a target width clamp. Used by backends to drop the
/// resolution when the caller requested a Preview or Thumbnail tier but
/// passed targetWidth=0 (meaning "let the backend pick a width for this
/// tier"). Standard / HighQuality pass through whatever the caller set
/// (or source resolution if 0).
int clampWidthForTier(EncodeTier tier, int callerWidth)
{
    // If the caller explicitly asked for a width, honor it — they may know
    // something we don't (e.g. recipient screen size signaled out-of-band).
    if (callerWidth > 0) return callerWidth;
    switch (tier) {
        case EncodeTier::Thumbnail:   return 128;
        case EncodeTier::Preview:     return 480;
        case EncodeTier::Standard:    return 720;
        case EncodeTier::HighQuality: return 0;  // 0 = match source
    }
    return 0;
}

/// Map an EncodeTier to a bitrate (kbps). Used when caller passes
/// bitrateKbps=0 meaning "pick a sensible default for this tier."
int defaultBitrateKbps(EncodeTier tier, int callerKbps)
{
    if (callerKbps > 0) return callerKbps;
    switch (tier) {
        case EncodeTier::Thumbnail:   return 150;   // tiny clip
        case EncodeTier::Preview:     return 500;
        case EncodeTier::Standard:    return 1500;
        case EncodeTier::HighQuality: return 4000;
    }
    return 1500;
}

// ─── gstreamer-based backends ───────────────────────────────────────────────
//
// These all share the same gst-launch pipeline shape; only the encoder
// element and its args differ. We extract the shared pipeline-building
// logic into a free function to avoid stamping out the same string-glue
// in each backend.

QStringList buildGstPipelineArgs(const EncodeSpec &spec,
                                  const QString &encoderElement,
                                  const QStringList &encoderArgs)
{
    QStringList args;
    args << QStringLiteral("-e") << QStringLiteral("-m");

    // Muxer + sink declared FIRST so name=mux is in scope.
    args << QStringLiteral("webmmux") << QStringLiteral("name=mux")
         << QStringLiteral("!") << QStringLiteral("filesink")
         << QStringLiteral("location=") + spec.outputPath;

    // Source + demuxer+decoder in one element.
    //
    // V32.16: switched from `matroskademux name=d` + per-pad references
    // (d.video_0/d.audio_0) to `decodebin name=dec` from the top. The
    // matroskademux per-pad pattern consistently fails with
    // "not-negotiated (-4)" on FP5/Alpine, even with the V32.12 fix in
    // place (videoconvert-before-videoscale + width=[1,W]). This is the
    // same fix the viewer needed in V32.12; transcoder was missed.
    //
    // decodebin handles demux + decode internally and exposes plain
    // raw audio/video pads. dec. is then aliased twice — once to the
    // video chain, once to the audio chain. gst-launch wires up the
    // dynamic pads automatically (matching by media type when the only
    // upstream is decodebin).
    //
    // Side benefit: opusdec is no longer needed in the audio branch —
    // decodebin auto-plugs it. Removes one element of complexity.
    args << QStringLiteral("filesrc")
         << QStringLiteral("location=") + spec.inputPath
         << QStringLiteral("!") << QStringLiteral("decodebin")
         << QStringLiteral("name=dec");

    // Video branch — convert FIRST, then scale, then convert+caps for encoder.
    //
    // Why videoconvert BEFORE videoscale:
    //   On some platforms (notably postmarketOS/Alpine on FP5, but also
    //   any setup using hardware-accelerated decoders), `decodebin` may
    //   autoplug a decoder that emits frames in a HW-specific format
    //   (NV12/NV21 in DMABuf memory, etc.). `videoscale` is strict about
    //   input formats and rejects exotic ones — producing the dreaded
    //   "not-negotiated (-4)" error upstream because downstream caps
    //   couldn't be agreed.
    //
    //   `videoconvert` handles arbitrary input formats and emits CPU
    //   memory in a videoscale-friendly format. Putting it first means
    //   videoscale always sees a known-good input format.
    //
    //   The cost: an extra (cheap) conversion when decodebin already
    //   emits a videoscale-friendly format. Worth it for portability.
    //
    // After videoscale we have a second videoconvert because svtav1enc
    // (and most encoders) want I420 specifically — videoscale doesn't
    // change format, so we convert at the end.
    args << QStringLiteral("dec.")
         << QStringLiteral("!") << QStringLiteral("queue")
         << QStringLiteral("!") << QStringLiteral("videoconvert");
    const int w = clampWidthForTier(spec.tier, spec.targetWidth);
    if (w > 0) {
        // Width range caps `[1,W]` instead of exact `width=W`. Difference:
        //   - `width=720` is a HARD constraint. videoscale MUST output 720,
        //     even if source is 480 (requires UPSCALE). Some videoscale
        //     impls refuse upscale; even when they don't, upscaling for
        //     transcode wastes bytes.
        //   - `width=[1,720]` is "any width up to 720". videoscale picks
        //     min(source, 720) and scales DOWN only when needed. Source-
        //     smaller-than-target passes through unchanged.
        //
        // Without an explicit PAR, videoscale picks a height that preserves
        // source aspect ratio (which is what we wanted anyway). The output
        // ends up matching whatever shape the camera produced, scaled to
        // our target width or smaller.
        //
        // We previously included `pixel-aspect-ratio=1/1` here but that
        // triggered an integer-overflow path in gstvideoconvertscale.c's
        // fixate code for certain source resolutions (same bug that hit
        // the recorder's preview branch).
        args << QStringLiteral("!") << QStringLiteral("videoscale")
             << QStringLiteral("!")
             << QStringLiteral("video/x-raw,width=[1,%1]").arg(w);
    }
    args << QStringLiteral("!") << QStringLiteral("videoconvert")
         << QStringLiteral("!") << QStringLiteral("video/x-raw,format=I420")
         << QStringLiteral("!") << QStringLiteral("queue")
         << QStringLiteral("!") << encoderElement;
    args << encoderArgs;
    args << QStringLiteral("!") << QStringLiteral("mux.");

    // Audio branch: re-encode to known-good 96k stereo opus.
    // We previously used opusparse passthrough but it had edge cases
    // where webmmux refused the matroska-framed opus stream and the
    // resulting webm had silent audio. Re-encoding adds ~5% CPU but
    // produces a uniformly playable output. Mirrors the ffmpeg backend
    // policy — see VideoEncoderBackend.cpp::FfmpegLibSvtAv1Backend.
    //
    // V32.16: opusdec removed — decodebin auto-plugs decode, the dec.
    // pad emits already-decoded raw audio.
    if (spec.hasAudio) {
        args << QStringLiteral("dec.")
             << QStringLiteral("!") << QStringLiteral("queue")
             << QStringLiteral("!") << QStringLiteral("audioconvert")
             << QStringLiteral("!") << QStringLiteral("audioresample")
             << QStringLiteral("!") << QStringLiteral("audio/x-raw,rate=48000,channels=2")
             // V32.32: automatic +3dB makeup gain with a limiter behind it.
             // Recordings vary wildly in level — voice from arm's length
             // peaks near -15dBFS while music or close-mic'd voice peaks
             // near -3dBFS. Without normalization the user gets blasted
             // by loud snaps after a quiet one. +3dB is conservative
             // (won't damage already-loud recordings) and the limiter
             // at -0.4dBFS prevents the boost from clipping the encoder.
             //
             // volume=1.413 ≈ 10^(3/20) = +3dB linear gain.
             // audiodynamic with ratio=20 + threshold near 1.0 acts as
             // a soft limiter — anything above -0.4dBFS gets squashed
             // 20:1, anything below passes unchanged. opusenc internally
             // uses float so the limiter ceiling matters before encode.
             //
             // Future: replace with loudnorm-style integrated-loudness
             // targeting once the audio mode picker (mute/voice/music)
             // ships. For now, simple makeup+limit handles the common
             // "too quiet" complaint without making "too loud" worse.
             << QStringLiteral("!") << QStringLiteral("volume")
             << QStringLiteral("volume=1.413")
             << QStringLiteral("!") << QStringLiteral("audiodynamic")
             << QStringLiteral("characteristics=hard-knee")
             << QStringLiteral("mode=compressor")
             << QStringLiteral("threshold=0.95")
             << QStringLiteral("ratio=20.0")
             << QStringLiteral("!") << QStringLiteral("opusenc")
             << QStringLiteral("bitrate=96000")
             << QStringLiteral("!") << QStringLiteral("queue")
             << QStringLiteral("!") << QStringLiteral("mux.");
    }
    return args;
}

// ─── SVT-AV1 via gstreamer ──────────────────────────────────────────────────

class GstSvtAv1Backend : public IVideoEncoderBackend {
public:
    QString name() const override { return QStringLiteral("gst-svtav1enc"); }
    QString binary() const override { return QStringLiteral("gst-launch-1.0"); }

    BackendAvailability checkAvailable() const override {
        if (!binaryExists(QStringLiteral("gst-launch-1.0")))
            return { false, QStringLiteral("gst-launch-1.0 not in PATH") };
        if (!gstElementExists(QStringLiteral("svtav1enc")))
            return { false, QStringLiteral("svtav1enc element missing") };
        return { true, {} };
    }

    QStringList buildArgs(const EncodeSpec &spec) const override {
        // SVT-AV1 preset selection by tier:
        //   • Thumbnail/Preview: preset=13 (fastest) — speed dominates
        //   • Standard:          preset=10 (default balance)
        //   • HighQuality:       preset=8  (~2× slower, noticeably better quality)
        int preset = 10;
        switch (spec.tier) {
            case EncodeTier::Thumbnail:   preset = 13; break;
            case EncodeTier::Preview:     preset = 13; break;
            case EncodeTier::Standard:    preset = 10; break;
            case EncodeTier::HighQuality: preset = 8;  break;
        }
        const int kbps = defaultBitrateKbps(spec.tier, spec.bitrateKbps);
        const Video::Av1Plan plan =
            Video::av1PlanFor(spec.tier, /*seekable=*/true, /*fps=*/0);
        QStringList enc;
        enc << QStringLiteral("preset=%1").arg(preset)
            << QStringLiteral("target-bitrate=%1").arg(kbps);
        // Disable look-ahead and B-frames for thumbnail/preview tiers —
        // dramatic speedup, minimal quality cost on short clips.
        if (spec.tier == EncodeTier::Thumbnail || spec.tier == EncodeTier::Preview) {
            enc << QStringLiteral("parameters-string=lookahead=0:pred-struct=0");
        } else {
            // V32.34+ AV1 ADVANCED COMPRESSION (gst fallback path). The ffmpeg
            // backend (primary) switches to CRF for the reference-frame bit
            // concentration; here we are MORE CONSERVATIVE. The gst svtav1enc
            // element exposes target-bitrate as a property and raw SVT params
            // via parameters-string, and the interaction of a CRF param with
            // the bitrate property is version-dependent and can't be verified
            // in this environment — so we KEEP this fallback's bitrate
            // rate-control and only add the SAFE, composable wins:
            //   • keyint=30 — ~1s GOP for seekability (unchanged),
            //   • hierarchical-levels — deeper B-frame pyramid = more reference
            //     reuse = better compression even under bitrate RC,
            //   • enable-tf=1 — temporal filtering cleans reference frames so
            //     predictions off them are sharper.
            // These compose with target-bitrate without changing the RC mode,
            // so a wrong assumption can't silently break the fallback encode.
            QStringList ps;
            ps << QStringLiteral("keyint=%1").arg(plan.keyint);
            if (plan.hierarchicalLevels > 0)
                ps << QStringLiteral("hierarchical-levels=%1").arg(plan.hierarchicalLevels);
            ps << QStringLiteral("enable-tf=%1").arg(plan.temporalFiltering ? 1 : 0);
            enc << QStringLiteral("parameters-string=%1").arg(ps.join(QLatin1Char(':')));
        }
        return buildGstPipelineArgs(spec, QStringLiteral("svtav1enc"), enc);
    }
};

// ─── VP9 fallback via gstreamer ─────────────────────────────────────────────

class GstVp9Backend : public IVideoEncoderBackend {
public:
    QString name() const override { return QStringLiteral("gst-vp9enc"); }
    QString binary() const override { return QStringLiteral("gst-launch-1.0"); }

    BackendAvailability checkAvailable() const override {
        if (!binaryExists(QStringLiteral("gst-launch-1.0")))
            return { false, QStringLiteral("gst-launch-1.0 not in PATH") };
        if (!gstElementExists(QStringLiteral("vp9enc")))
            return { false, QStringLiteral("vp9enc element missing") };
        return { true, {} };
    }

    QStringList buildArgs(const EncodeSpec &spec) const override {
        const int kbps = defaultBitrateKbps(spec.tier, spec.bitrateKbps);
        int cpuUsed = 5;
        switch (spec.tier) {
            case EncodeTier::Thumbnail:   cpuUsed = 8; break;
            case EncodeTier::Preview:     cpuUsed = 8; break;
            case EncodeTier::Standard:    cpuUsed = 5; break;
            case EncodeTier::HighQuality: cpuUsed = 2; break;
        }
        QStringList enc;
        enc << QStringLiteral("target-bitrate=%1").arg(kbps * 1000)
            << QStringLiteral("cpu-used=%1").arg(cpuUsed)
            << QStringLiteral("deadline=1")
            // V32.34+ SEEKABILITY: keyframe at least every ~1s. vp9enc's
            // keyframe-max-dist is in FRAMES; the recorder captures at 30fps
            // (see VideoRecorder key-int-max=30 / video_gop_size=30), so 30 ≈
            // 1s. Without this the re-encode produces a long-GOP file that
            // scrubs coarsely. Frame-based (not time-based like the AV1 path)
            // because the vp9enc element has no time-keyint control.
            << QStringLiteral("keyframe-max-dist=30");
        return buildGstPipelineArgs(spec, QStringLiteral("vp9enc"), enc);
    }
};

// ─── VP8 last-ditch via gstreamer ───────────────────────────────────────────

class GstVp8Backend : public IVideoEncoderBackend {
public:
    QString name() const override { return QStringLiteral("gst-vp8enc"); }
    QString binary() const override { return QStringLiteral("gst-launch-1.0"); }

    BackendAvailability checkAvailable() const override {
        if (!binaryExists(QStringLiteral("gst-launch-1.0")))
            return { false, QStringLiteral("gst-launch-1.0 not in PATH") };
        if (!gstElementExists(QStringLiteral("vp8enc")))
            return { false, QStringLiteral("vp8enc element missing") };
        return { true, {} };
    }

    QStringList buildArgs(const EncodeSpec &spec) const override {
        const int kbps = defaultBitrateKbps(spec.tier, spec.bitrateKbps);
        QStringList enc;
        enc << QStringLiteral("target-bitrate=%1").arg(kbps * 1000)
            << QStringLiteral("cpu-used=16")
            << QStringLiteral("deadline=1")
            // V32.34+ SEEKABILITY: keyframe ~every 1s (30 frames @ 30fps
            // source). Same rationale as the VP9 backend — vp8enc uses the
            // frame-based keyframe-max-dist control.
            << QStringLiteral("keyframe-max-dist=30");
        return buildGstPipelineArgs(spec, QStringLiteral("vp8enc"), enc);
    }
};

// ─── FFmpeg + libsvtav1 ─────────────────────────────────────────────────────
//
// Alternate AV1 path via the ffmpeg CLI. Benefits over the gstreamer path:
//   • ffmpeg's libsvtav1 wrapper tracks upstream SVT-AV1 more aggressively;
//     features land months earlier than they appear in gst-plugins-rs's
//     svtav1enc element.
//   • ffmpeg emits accurate "frame=NNN ... time=HH:MM:SS.MMM" progress on
//     stderr, which lets us drive the UI progress bar precisely. (gst's
//     svtav1enc emits a config banner and then nothing until EOS.)
//   • Hardware acceleration paths (vaapi, qsv) are available behind the
//     same CLI — worth exploring on FP5 once the basic path is stable.
//
// Cost: ffmpeg has to be installed (not always present on minimal mobile
// builds — Alpine/postmarketOS sometimes ships only the libs, not the CLI).
// That's why this is positioned BEFORE the gstreamer fallback but the
// gstreamer path remains the workhorse.

class FfmpegLibSvtAv1Backend : public IVideoEncoderBackend {
public:
    QString name() const override { return QStringLiteral("ffmpeg-libsvtav1"); }
    QString binary() const override { return QStringLiteral("ffmpeg"); }

    BackendAvailability checkAvailable() const override {
        if (!binaryExists(QStringLiteral("ffmpeg")))
            return { false, QStringLiteral("ffmpeg not in PATH") };
        // Probe whether libsvtav1 is enabled in this ffmpeg build.
        // `ffmpeg -encoders 2>&1 | grep libsvtav1` is the canonical check.
        QProcess p;
        p.start(QStringLiteral("ffmpeg"),
                { QStringLiteral("-hide_banner"),
                  QStringLiteral("-encoders") });
        if (!p.waitForFinished(2000)) { p.kill();
            return { false, QStringLiteral("ffmpeg -encoders timed out") };
        }
        const QByteArray out = p.readAllStandardOutput()
                             + p.readAllStandardError();
        if (!out.contains("libsvtav1"))
            return { false, QStringLiteral("ffmpeg lacks libsvtav1") };
        return { true, {} };
    }

    QStringList buildArgs(const EncodeSpec &spec) const override {
        const int w = clampWidthForTier(spec.tier, spec.targetWidth);
        // ffmpeg -preset for libsvtav1: 0-13 same scale as the gst element.
        int preset = 10;
        switch (spec.tier) {
            case EncodeTier::Thumbnail:   preset = 13; break;
            case EncodeTier::Preview:     preset = 13; break;
            case EncodeTier::Standard:    preset = 10; break;
            case EncodeTier::HighQuality: preset = 8;  break;
        }
        // V32.34+ AV1 ADVANCED COMPRESSION: the encode strategy (CRF vs
        // bitrate, GOP, hierarchy, temporal filtering) comes from the policy
        // mirrored from the tested Rust snapclone-av1policy crate. User-facing
        // tiers use CONSTANT-QUALITY so SVT-AV1 can concentrate bits on
        // I-frames / reference frames (leaf frames borrow from high-quality
        // references → smaller file at equal perceived quality). The clip is
        // delivered, not scrubbed during encode, but we keep the GOP
        // scrub-friendly (seekable=true) so the existing seek behavior is
        // preserved — the gain here is the rate-control + hierarchy switch, not
        // a longer GOP (which would trade away seekability). A later "delivery
        // vs viewing" split can pass seekable=false for extra bytes.
        const Video::Av1Plan plan =
            Video::av1PlanFor(spec.tier, /*seekable=*/true, /*fps=*/0);
        QStringList args;
        args << QStringLiteral("-y")             // overwrite
             << QStringLiteral("-hide_banner")   // less log spam
             << QStringLiteral("-loglevel") << QStringLiteral("info")
             << QStringLiteral("-stats")         // progress lines on stderr
             << QStringLiteral("-i")  << spec.inputPath
             << QStringLiteral("-c:v") << QStringLiteral("libsvtav1")
             << QStringLiteral("-preset") << QString::number(preset);
        if (plan.useCrf) {
            // CRF mode: libsvtav1 enables constant-quality when -b:v is 0 and
            // -crf is set. This is what lets the encoder spend few bits on leaf
            // frames and many on references.
            args << QStringLiteral("-crf") << QString::number(plan.crf)
                 << QStringLiteral("-b:v") << QStringLiteral("0");
        } else {
            const int kbps = defaultBitrateKbps(spec.tier, spec.bitrateKbps);
            args << QStringLiteral("-b:v") << QStringLiteral("%1k").arg(kbps);
        }
        args << QStringLiteral("-pix_fmt") << QStringLiteral("yuv420p");
        // SVT-AV1 advanced params: hierarchical depth + temporal filtering on
        // the user tiers. These names (`hierarchical-levels`, `enable-tf`) are
        // stable across SVT-AV1 releases. We only emit them when non-default so
        // older SVT builds that lack one param aren't tripped on the tiny tiers.
        {
            QStringList svtParams;
            if (plan.hierarchicalLevels > 0) {
                svtParams << QStringLiteral("hierarchical-levels=%1")
                             .arg(plan.hierarchicalLevels);
            }
            svtParams << QStringLiteral("enable-tf=%1")
                         .arg(plan.temporalFiltering ? 1 : 0);
            if (!svtParams.isEmpty()) {
                args << QStringLiteral("-svtav1-params")
                     << svtParams.join(QLatin1Char(':'));
            }
        }
        // V32.34+ SEEKABILITY: force a keyframe (I-frame) roughly every 1
        // second of presentation time. Without this, libsvtav1 uses its
        // default long GOP — a closed keyframe only every few seconds — so
        // the exported .webm seeks coarsely (the player can only jump to the
        // nearest keyframe) and the first frame is slow to decode. The
        // RECORDING pipeline already keys every 30 frames (key-int-max=30 /
        // video_gop_size=30), but that intermediate file is re-encoded here
        // and the keyframe cadence does NOT carry across a transcode — the
        // AV1 encoder picks its own GOP from scratch. We force it back.
        //
        // Two complementary controls were considered; we use just the
        // robust one:
        //   • -force_key_frames expr:gte(t,n_forced) — places a keyframe at
        //     each whole-second mark. FPS-INDEPENDENT (resolves against the
        //     source's actual presentation timestamps, which is good because
        //     EncodeSpec doesn't carry fps) and authoritative: ffmpeg inserts
        //     these keyframes regardless of the encoder's own GOP decisions.
        // We deliberately do NOT also pass -svtav1-params keyint=…: it'd be
        // redundant (force_key_frames already guarantees the seek points) and
        // the param-string suffix forms vary by ffmpeg/SVT-AV1 version, so
        // adding it risks a parse error for no real gain. The encoder may
        // still insert extra keyframes of its own — that only HELPS seeking.
        // Thumbnails/previews are tiny and rarely scrubbed, so only spend the
        // extra keyframes (which cost bitrate) on the user-facing tiers.
        if (spec.tier == EncodeTier::Standard
            || spec.tier == EncodeTier::HighQuality) {
            args << QStringLiteral("-force_key_frames")
                 << QStringLiteral("expr:gte(t,n_forced)");
        }
        // V32.34+ FACE/SALIENCY ROI BIT-ALLOCATION (improved gains): instead of
        // a single addroi that only BOOSTS the face (qoffset=-1) and does
        // nothing to the background, we now ask the saliency crate for a
        // bit-allocation PLAN: a background penalty over the whole frame
        // (compress harder where the viewer isn't looking) PLUS a padded face
        // boost that protects the subject from that global degradation. Bits
        // move from background → face, so the file shrinks at equal perceived
        // quality — the actual point of face-aware compression, which the old
        // boost-only path never delivered. Directives apply in order (later
        // addroi overrides earlier in the overlap), so we emit background first,
        // face last. Padding/clamping/the dominate-face skip all live in the
        // tested Rust planner (sc_roi_plan). Only user-facing tiers get it.
        QStringList vfChain;
        const bool roiTier = (spec.tier == EncodeTier::Standard
                              || spec.tier == EncodeTier::HighQuality);
        if (roiTier && spec.roiW > 0 && spec.roiH > 0) {
#ifdef SNAPCLONE_RUST_SALIENCY
            // Frame dims the plan clamps against. Prefer the source dims the
            // transcoder resolved into the spec; fall back to the ROI's own
            // extent if unknown (the planner clamps either way).
            const uint32_t fw = spec.roiFrameW > 0
                ? static_cast<uint32_t>(spec.roiFrameW)
                : static_cast<uint32_t>(spec.roiX + spec.roiW);
            const uint32_t fh = spec.roiFrameH > 0
                ? static_cast<uint32_t>(spec.roiFrameH)
                : static_cast<uint32_t>(spec.roiY + spec.roiH);
            CRoiDirective dirs[4];
            const uint32_t n = sc_roi_plan(
                static_cast<uint32_t>(spec.roiX), static_cast<uint32_t>(spec.roiY),
                static_cast<uint32_t>(spec.roiW), static_cast<uint32_t>(spec.roiH),
                fw, fh, dirs, 4);
            for (uint32_t i = 0; i < n; ++i) {
                // ffmpeg addroi qoffset is a rational in [-1, 1]; the planner
                // gives centi-units. Format with 2 decimals so e.g. -30 → -0.30.
                const double qoff = dirs[i].qp_delta_centi / 100.0;
                vfChain << QStringLiteral("addroi=%1:%2:%3:%4:%5")
                           .arg(dirs[i].x).arg(dirs[i].y)
                           .arg(dirs[i].w).arg(dirs[i].h)
                           .arg(QString::number(qoff, 'f', 2));
            }
            if (n > 0) {
                qCInfo(lcEncoder) << "ROI plan:" << n << "directive(s) — bg penalty + face boost";
            }
#else
            // Saliency crate not built — fall back to the legacy single boost
            // (face protected, no background penalty). Still better than nothing.
            vfChain << QStringLiteral("addroi=%1:%2:%3:%4:-0.30")
                       .arg(spec.roiX).arg(spec.roiY)
                       .arg(spec.roiW).arg(spec.roiH);
#endif
        }
        if (w > 0) {
            // Aspect-preserving DOWNSCALE only (never upscale). The
            // ffmpeg scale filter accepts an expression for the width
            // operand: `min(iw,W)` means "min of input width and W".
            // For sources smaller than W, scale is a no-op identity.
            // For sources larger, scale to W.
            //
            // Why not upscale: it wastes bytes on transcode output and
            // can confuse the gstreamer-based viewer downstream (see
            // VideoEncoderBackend's gst-based variant for the parallel
            // `width=[1,W]` caps range that achieves the same end).
            //
            // -2 means "compute height to preserve aspect, divisible by 2"
            // (required by yuv420p chroma subsampling).
            vfChain << QStringLiteral("scale=min(iw\\,%1):-2").arg(w);
        }
        if (!vfChain.isEmpty()) {
            args << QStringLiteral("-vf")
                 << vfChain.join(QLatin1Char(','));
        }
        // Audio: re-encode to opus rather than -c:a copy. Reasons:
        //   • The recorder's opus stream is wrapped in matroska container
        //     packets. ffmpeg can usually re-mux those into webm, but
        //     edge-case audio config (mono variant, SR=48000, channel
        //     mapping) can produce silent-but-valid webm output on some
        //     players when copied byte-for-byte.
        //   • Re-encoding to a known-good 96kbps stereo opus is what the
        //     wider webm ecosystem (browsers, Qt's FFmpeg backend, VLC)
        //     handles best.
        //   • CPU cost is ~5% — negligible compared to AV1 encode.
        //   • If audio is corrupt or partial in the source, re-encode
        //     fails loudly rather than producing a silent-output file.
        if (spec.hasAudio) {
            args << QStringLiteral("-c:a") << QStringLiteral("libopus")
                 << QStringLiteral("-b:a") << QStringLiteral("96k")
                 << QStringLiteral("-ac")  << QStringLiteral("2")
                 << QStringLiteral("-ar")  << QStringLiteral("48000");
        } else {
            args << QStringLiteral("-an");
        }
        args << QStringLiteral("-f") << QStringLiteral("webm")
             << spec.outputPath;
        return args;
    }

    QRegularExpression progressPattern() const override {
        // ffmpeg emits lines like:
        //   frame=  120 fps= 80 q=23.0 size=  256kB time=00:00:04.00 ...
        // We can compute a percent if we know the input duration, but
        // that means passing duration into the pattern context.
        // Simpler: a "frame= NNN" capture; the transcoder converts to
        // percent using its already-known input duration.
        return QRegularExpression(QStringLiteral(R"(frame=\s*(\d+))"));
    }
};

// ─── External codec scaffold ────────────────────────────────────────────────
//
// Placeholder for an in-development codec binary that exposes a CLI
// compatible with our needs. The expected interface is:
//
//     snapcodec encode --in INPUT --out OUTPUT
//                      --tier {thumbnail|preview|standard|highquality}
//                      [--width N] [--bitrate K]
//
// When the binary is available on PATH, this backend takes priority over
// all others. Until then, checkAvailable() returns false and the chain
// falls through to FFmpeg or gstreamer.
//
// To enable: drop the `snapcodec` binary on PATH (or set the
// SNAPCLONE_CODEC environment variable to its absolute path). The
// override-by-env hook is here so the codec can be tested out-of-tree
// without an install step.

class ExternalCodecBackend : public IVideoEncoderBackend {
public:
    QString name() const override { return QStringLiteral("snapcodec"); }

    QString binary() const override {
        const QByteArray override = qgetenv("SNAPCLONE_CODEC");
        if (!override.isEmpty()) return QString::fromLocal8Bit(override);
        return QStringLiteral("snapcodec");
    }

    BackendAvailability checkAvailable() const override {
        const QString b = binary();
        // If it's an absolute path (override), check the file directly.
        if (b.startsWith(QLatin1Char('/'))) {
            if (!QFile::exists(b))
                return { false, QStringLiteral("SNAPCLONE_CODEC override path missing: %1").arg(b) };
        } else {
            if (!binaryExists(b))
                return { false, QStringLiteral("snapcodec not in PATH (set SNAPCLONE_CODEC=/path/to/binary to override)") };
        }
        // Optional: invoke `snapcodec --version` once to verify it actually
        // runs. We skip this for now to keep startup probe fast — if the
        // binary is broken the first encode will fail loudly.
        return { true, {} };
    }

    QStringList buildArgs(const EncodeSpec &spec) const override {
        const int w = clampWidthForTier(spec.tier, spec.targetWidth);
        const int kbps = defaultBitrateKbps(spec.tier, spec.bitrateKbps);
        QStringList args;
        args << QStringLiteral("encode")
             << QStringLiteral("--in")   << spec.inputPath
             << QStringLiteral("--out")  << spec.outputPath
             << QStringLiteral("--tier") << QString::fromLatin1(
                    SnapClone::Core::Media::tierName(spec.tier));
        if (w > 0)    args << QStringLiteral("--width")   << QString::number(w);
        if (kbps > 0) args << QStringLiteral("--bitrate") << QString::number(kbps);
        if (!spec.hasAudio) args << QStringLiteral("--no-audio");
        return args;
    }

    QRegularExpression progressPattern() const override {
        // Convention: the codec emits "progress: NN" lines on stderr.
        // If it doesn't, this regex won't match and the transcoder falls
        // back to size-based estimation, same as the gst backends.
        return QRegularExpression(QStringLiteral(R"(progress:\s*(\d+))"));
    }
};

} // anon namespace

// ─── Fraunhofer H.266 / VVC (opt-in, availability-probed) ────────────────────
//
// H.266/VVC via ffmpeg's libvvenc wrapper (Fraunhofer VVenC). This is OPT-IN
// and sits at the BOTTOM of the priority chain: it is only selected when
//   (a) ffmpeg is built WITH libvvenc (most distro ffmpeg builds are NOT), and
//   (b) it's explicitly enabled via the SNAPCLONE_ENABLE_VVC env var.
// So it never displaces the working AV1 path and is invisibly absent otherwise.
//
// LICENSING NOTE (surfaced deliberately, not silently baked in): the VVenC/
// VVdeC SOURCE is BSD-3-Clause-style (building/using is fine), but VVC is
// covered by patent pools (MPEG LA / Access Advance). For a self-hosted /
// experimental build this is almost certainly fine; commercial distribution of
// a VVC codec may carry royalty obligations. That's a project-owner business
// call — this backend stays off unless explicitly enabled.
//
// CONTAINER: VVC cannot go in webm (webm carries only VP8/VP9/AV1), so this
// backend returns "mkv" from outputContainer(); the transcoder picks the
// extension accordingly. Decode-side support is probed separately (see
// vvcDecodeAvailable() and the viewer).
class FfmpegVvcBackend : public IVideoEncoderBackend {
public:
    QString name() const override { return QStringLiteral("ffmpeg-libvvenc"); }
    QString binary() const override { return QStringLiteral("ffmpeg"); }
    QString outputContainer() const override { return QStringLiteral("mkv"); }

    BackendAvailability checkAvailable() const override {
        // Opt-in gate: never auto-select VVC unless the user asked for it.
        if (qEnvironmentVariableIsEmpty("SNAPCLONE_ENABLE_VVC"))
            return { false, QStringLiteral("VVC disabled (set SNAPCLONE_ENABLE_VVC=1)") };
        if (!binaryExists(QStringLiteral("ffmpeg")))
            return { false, QStringLiteral("ffmpeg not in PATH") };
        // Probe whether libvvenc is compiled into this ffmpeg.
        QProcess p;
        p.start(QStringLiteral("ffmpeg"),
                { QStringLiteral("-hide_banner"), QStringLiteral("-encoders") });
        if (!p.waitForFinished(2000)) { p.kill();
            return { false, QStringLiteral("ffmpeg -encoders timed out") };
        }
        const QByteArray out = p.readAllStandardOutput() + p.readAllStandardError();
        // ffmpeg lists the VVC encoder as "libvvenc" (Fraunhofer) — and some
        // builds also expose "vvc" via an internal wrapper. Accept either.
        if (!out.contains("libvvenc") && !out.contains("vvenc"))
            return { false, QStringLiteral("ffmpeg lacks libvvenc (VVC encoder)") };
        return { true, {} };
    }

    QStringList buildArgs(const EncodeSpec &spec) const override {
        const int w = clampWidthForTier(spec.tier, spec.targetWidth);
        // libvvenc preset ladder: faster→slower is
        // faster/fast/medium/slow/slower. Map tiers conservatively — VVC encode
        // is far heavier than AV1, so even "medium" is costly on a phone.
        QString preset = QStringLiteral("fast");
        switch (spec.tier) {
            case EncodeTier::Thumbnail:   preset = QStringLiteral("faster"); break;
            case EncodeTier::Preview:     preset = QStringLiteral("faster"); break;
            case EncodeTier::Standard:    preset = QStringLiteral("fast");   break;
            case EncodeTier::HighQuality: preset = QStringLiteral("medium"); break;
        }
        // VVC rate control: libvvenc supports constant-QP via -qp (0-63, lower =
        // better). Reuse the AV1 plan's CRF as a QP analog where it makes sense;
        // the scales differ but both are "smaller number = higher quality", and
        // this keeps tier behavior consistent. Bitrate mode falls back to -b:v.
        const Video::Av1Plan plan =
            Video::av1PlanFor(spec.tier, /*seekable=*/true, /*fps=*/0);
        QStringList args;
        args << QStringLiteral("-y")
             << QStringLiteral("-hide_banner")
             << QStringLiteral("-loglevel") << QStringLiteral("info")
             << QStringLiteral("-stats")
             << QStringLiteral("-i")  << spec.inputPath
             << QStringLiteral("-c:v") << QStringLiteral("libvvenc")
             << QStringLiteral("-preset") << preset;
        if (plan.useCrf) {
            // Map AV1 CRF (~0-63) onto VVC QP (0-63). They aren't identical
            // scales, but both are perceptual-quality knobs in the same range,
            // so the tier ladder stays monotonic. -qp selects constant-QP.
            args << QStringLiteral("-qp") << QString::number(plan.crf);
        } else {
            const int kbps = defaultBitrateKbps(spec.tier, spec.bitrateKbps);
            args << QStringLiteral("-b:v") << QStringLiteral("%1k").arg(kbps);
        }
        args << QStringLiteral("-pix_fmt") << QStringLiteral("yuv420p");
        // Seekability: force ~1s keyframes on user-facing tiers, same rationale
        // as the AV1 backend (a transcode resets GOP; the player seeks to
        // keyframes). force_key_frames is codec-agnostic in ffmpeg.
        if (spec.tier == EncodeTier::Standard
            || spec.tier == EncodeTier::HighQuality) {
            args << QStringLiteral("-force_key_frames")
                 << QStringLiteral("expr:gte(t,n_forced)");
        }
        // Scale (aspect-preserving downscale only) — identical to AV1 path. NOTE:
        // ROI/addroi is intentionally omitted: ffmpeg's addroi sets a generic
        // side-data qoffset that libvvenc does not currently consume, so it'd be
        // a no-op that only adds risk. If a future libvvenc exposes ROI, add it
        // here mirroring the AV1 sc_roi_plan block.
        if (w > 0) {
            args << QStringLiteral("-vf")
                 << QStringLiteral("scale=min(iw\\,%1):-2").arg(w);
        }
        if (spec.hasAudio) {
            // mkv carries opus fine; keep the same known-good 96k stereo opus.
            args << QStringLiteral("-c:a") << QStringLiteral("libopus")
                 << QStringLiteral("-b:a") << QStringLiteral("96k")
                 << QStringLiteral("-ac")  << QStringLiteral("2")
                 << QStringLiteral("-ar")  << QStringLiteral("48000");
        } else {
            args << QStringLiteral("-an");
        }
        // matroska container (VVC-compatible, unlike webm).
        args << QStringLiteral("-f") << QStringLiteral("matroska")
             << spec.outputPath;
        return args;
    }

    QRegularExpression progressPattern() const override {
        return QRegularExpression(QStringLiteral(R"(frame=\s*(\d+))"));
    }
};

// V32.34+: is a VVC DECODER available for playback? Probed the same opt-in way
// as the encoder. The viewer calls this to decide whether it can play a
// received .mkv that turns out to carry H.266. Both Qt's QMediaPlayer (ffmpeg
// backend) and the gst decodebin fallback can decode VVC IF the underlying
// ffmpeg/gst has libvvdec — otherwise playback of a VVC file fails and the
// viewer should surface a clear "no VVC decoder" message rather than a blank.
bool vvcDecodeAvailable()
{
    if (qEnvironmentVariableIsEmpty("SNAPCLONE_ENABLE_VVC"))
        return false;
    if (!binaryExists(QStringLiteral("ffmpeg")))
        return false;
    QProcess p;
    p.start(QStringLiteral("ffmpeg"),
            { QStringLiteral("-hide_banner"), QStringLiteral("-decoders") });
    if (!p.waitForFinished(2000)) { p.kill(); return false; }
    const QByteArray out = p.readAllStandardOutput() + p.readAllStandardError();
    // Fraunhofer decoder wrapper is "libvvdec"; internal "vvc" decoder also counts.
    return out.contains("libvvdec") || out.contains("vvdec")
        || out.contains(" vvc ");
}

std::vector<std::unique_ptr<IVideoEncoderBackend>>
allBackendsInPriorityOrder()
{
    std::vector<std::unique_ptr<IVideoEncoderBackend>> out;
    out.reserve(6);
    // Priority order — first available wins.
    out.emplace_back(std::make_unique<ExternalCodecBackend>());
    out.emplace_back(std::make_unique<FfmpegLibSvtAv1Backend>());
    out.emplace_back(std::make_unique<GstSvtAv1Backend>());
    out.emplace_back(std::make_unique<GstVp9Backend>());
    out.emplace_back(std::make_unique<GstVp8Backend>());
    // H.266/VVC last: opt-in (SNAPCLONE_ENABLE_VVC) + needs ffmpeg+libvvenc, so
    // checkAvailable() returns false on a normal build and the chain never
    // reaches it. Never the default; never displaces AV1.
    out.emplace_back(std::make_unique<FfmpegVvcBackend>());
    return out;
}

} // namespace SnapClone::UI::Video
