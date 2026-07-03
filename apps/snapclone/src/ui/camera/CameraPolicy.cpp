#include "ui/camera/CameraPolicy.hpp"

#include <QSettings>

namespace SnapClone::UI::Camera {

namespace {
constexpr const char *kGroup = "camera/policy";

// Keys kept stable in QSettings — DO NOT rename without a migration.
// User installs will lose their tuned values if a key disappears.
constexpr const char *kKeyStartupMs        = "startupTimeoutMs";
constexpr const char *kKeyFirstFrameMs     = "firstFrameTimeoutMs";
constexpr const char *kKeyEarlyExitMs      = "earlyExitWindowMs";
constexpr const char *kKeyAutoRetry        = "autoRetryAlternate";
constexpr const char *kKeyProbeStartupMs   = "probeStartupTimeoutMs";
constexpr const char *kKeyProbeFirstFrameMs= "probeFirstFrameTimeoutMs";
constexpr const char *kKeyProbeEarlyExitMs = "probeEarlyExitWindowMs";
constexpr const char *kKeyCaptureWidth     = "captureWidth";
constexpr const char *kKeyCaptureHeight    = "captureHeight";
constexpr const char *kKeyPreviewMaxWidth  = "previewMaxWidth";
constexpr const char *kKeyPreviewMaxHeight = "previewMaxHeight";
constexpr const char *kKeyPreviewMaxFps    = "previewMaxFps";
constexpr const char *kKeyRotation         = "rotation";
constexpr const char *kKeyFramedropMs      = "framedropTimeoutMs";
constexpr const char *kKeyStderrBufBytes   = "stderrBufMaxBytes";
constexpr const char *kKeyLogFirstFrame    = "logFirstFrameInfo";
constexpr const char *kKeySimultaneousCap  = "simultaneousCapture";
// V32.34+: source-quality keys
constexpr const char *kKeySourceTargetWidth   = "sourceTargetWidth";
constexpr const char *kKeySourceTargetHeight  = "sourceTargetHeight";
constexpr const char *kKeySourceTargetFps     = "sourceTargetFps";
constexpr const char *kKeyLowQualityRatio     = "lowQualityRatio";
constexpr const char *kKeyAutoQualityRestart  = "autoQualityRestart";
constexpr const char *kKeyFrameRateDegRatio   = "frameRateDegradedRatio";
// V32.34+ GPU accel + tint fix
constexpr const char *kKeyPreferGpuConvert    = "preferGpuConvert";
constexpr const char *kKeyStrideAlignPx       = "strideAlignPx";
constexpr const char *kKeyGpuSkipDownloadRepack = "gpuSkipDownloadRepack";
constexpr const char *kKeyGpuSourceWidth      = "gpuSourceWidth";
constexpr const char *kKeyGpuSourceHeight     = "gpuSourceHeight";

// Clamp anything that comes back from QSettings to a sane range.
// Out-of-range values typically come from a corrupted INI or from
// an attacker who got write access to the user's config dir — we
// don't want to honor a literal "0 ms timeout" or "999999×999999
// capture resolution" because of either.
int clampMs(int v, int lo, int hi) {
    if (v < lo) return lo;
    if (v > hi) return hi;
    return v;
}
int clampDim(int v) {
    // Anything below 80px is below the smallest viewfinder we'd render;
    // anything above 4096 doesn't fit in mobile RAM at BGRx*4.
    if (v < 80)    return 80;
    if (v > 4096)  return 4096;
    return v;
}
} // namespace

CameraPolicyStore &CameraPolicyStore::instance()
{
    static CameraPolicyStore s;
    return s;
}

CameraPolicyStore::CameraPolicyStore()
{
    load();
}

void CameraPolicyStore::load()
{
    // Default-construct a CameraPolicy; QSettings then overrides only
    // the fields the user has explicitly set. Missing keys leave the
    // compile-time defaults in place — a new field added to CameraPolicy
    // gets its default value on existing installs without needing a
    // migration step.
    CameraPolicy p;
    QSettings s;
    s.beginGroup(QLatin1String(kGroup));
    p.startupTimeoutMs         = clampMs(s.value(kKeyStartupMs,        p.startupTimeoutMs).toInt(),         50, 60'000);
    p.firstFrameTimeoutMs      = clampMs(s.value(kKeyFirstFrameMs,     p.firstFrameTimeoutMs).toInt(),     100, 60'000);
    p.earlyExitWindowMs        = clampMs(s.value(kKeyEarlyExitMs,      p.earlyExitWindowMs).toInt(),       100, 30'000);
    p.autoRetryAlternate       =          s.value(kKeyAutoRetry,       p.autoRetryAlternate).toBool();
    p.probeStartupTimeoutMs    = clampMs(s.value(kKeyProbeStartupMs,   p.probeStartupTimeoutMs).toInt(),    50, 10'000);
    p.probeFirstFrameTimeoutMs = clampMs(s.value(kKeyProbeFirstFrameMs,p.probeFirstFrameTimeoutMs).toInt(), 50, 10'000);
    p.probeEarlyExitWindowMs   = clampMs(s.value(kKeyProbeEarlyExitMs, p.probeEarlyExitWindowMs).toInt(),   50, 10'000);
    p.captureWidth             = clampDim(s.value(kKeyCaptureWidth,    p.captureWidth).toInt());
    p.captureHeight            = clampDim(s.value(kKeyCaptureHeight,   p.captureHeight).toInt());
    // V32.39+ PERF: preview/capture frame cap. 0 = no cap; otherwise clamp to a
    // sane range. (Lower bound 0 to allow disabling; upper bound matches dims.)
    p.previewMaxWidth  = s.value(kKeyPreviewMaxWidth,  p.previewMaxWidth).toInt();
    if (p.previewMaxWidth  < 0)    p.previewMaxWidth  = 0;
    if (p.previewMaxWidth  > 4096) p.previewMaxWidth  = 4096;
    p.previewMaxHeight = s.value(kKeyPreviewMaxHeight, p.previewMaxHeight).toInt();
    if (p.previewMaxHeight < 0)    p.previewMaxHeight = 0;
    if (p.previewMaxHeight > 4096) p.previewMaxHeight = 4096;
    p.previewMaxFps    = s.value(kKeyPreviewMaxFps,    p.previewMaxFps).toInt();
    if (p.previewMaxFps < 0)       p.previewMaxFps = 0;
    if (p.previewMaxFps > 120)     p.previewMaxFps = 120;
    p.rotation                 =          s.value(kKeyRotation,        p.rotation).toString();
    p.framedropTimeoutMs       = clampMs(s.value(kKeyFramedropMs,      p.framedropTimeoutMs).toInt(),      300, 60'000);
    p.stderrBufMaxBytes        = clampMs(s.value(kKeyStderrBufBytes,   p.stderrBufMaxBytes).toInt(),       512, 65'536);
    p.logFirstFrameInfo        =          s.value(kKeyLogFirstFrame,   p.logFirstFrameInfo).toBool();
    p.simultaneousCaptureSupported = s.value(kKeySimultaneousCap, p.simultaneousCaptureSupported).toBool();
    // V32.34+: source-quality preferences. We accept 0 as "let
    // pipewiresrc negotiate freely" (the old behavior), so the lower
    // bound is 0 rather than the captureWidth/Height minimum.
    p.sourceTargetWidth   = s.value(kKeySourceTargetWidth,   p.sourceTargetWidth).toInt();
    if (p.sourceTargetWidth < 0)       p.sourceTargetWidth = 0;
    if (p.sourceTargetWidth > 4096)    p.sourceTargetWidth = 4096;
    p.sourceTargetHeight  = s.value(kKeySourceTargetHeight,  p.sourceTargetHeight).toInt();
    if (p.sourceTargetHeight < 0)      p.sourceTargetHeight = 0;
    if (p.sourceTargetHeight > 4096)   p.sourceTargetHeight = 4096;
    p.sourceTargetFps     = s.value(kKeySourceTargetFps,     p.sourceTargetFps).toInt();
    if (p.sourceTargetFps < 0)         p.sourceTargetFps = 0;
    if (p.sourceTargetFps > 120)       p.sourceTargetFps = 120;
    p.lowQualityRatio     = s.value(kKeyLowQualityRatio,     p.lowQualityRatio).toReal();
    if (p.lowQualityRatio < 0.1)       p.lowQualityRatio = 0.1;
    if (p.lowQualityRatio > 1.0)       p.lowQualityRatio = 1.0;
    p.autoQualityRestart  = s.value(kKeyAutoQualityRestart,  p.autoQualityRestart).toString().toLower();
    if (p.autoQualityRestart != QLatin1String("off")
     && p.autoQualityRestart != QLatin1String("first")
     && p.autoQualityRestart != QLatin1String("always")) {
        p.autoQualityRestart = QStringLiteral("first");
    }
    p.frameRateDegradedRatio = s.value(kKeyFrameRateDegRatio, p.frameRateDegradedRatio).toReal();
    if (p.frameRateDegradedRatio < 1.0) p.frameRateDegradedRatio = 1.0;
    if (p.frameRateDegradedRatio > 5.0) p.frameRateDegradedRatio = 5.0;
    // V32.34+ GPU accel + tint fix.
    p.preferGpuConvert = s.value(kKeyPreferGpuConvert, p.preferGpuConvert).toBool();
    p.strideAlignPx    = s.value(kKeyStrideAlignPx,    p.strideAlignPx).toInt();
    if (p.strideAlignPx < 1)  p.strideAlignPx = 1;
    if (p.strideAlignPx > 64) p.strideAlignPx = 64;
    p.gpuSkipDownloadRepack = s.value(kKeyGpuSkipDownloadRepack, p.gpuSkipDownloadRepack).toBool();
    p.gpuSourceWidth   = s.value(kKeyGpuSourceWidth,   p.gpuSourceWidth).toInt();
    if (p.gpuSourceWidth < 0)    p.gpuSourceWidth = 0;
    if (p.gpuSourceWidth > 4096) p.gpuSourceWidth = 4096;
    p.gpuSourceHeight  = s.value(kKeyGpuSourceHeight,  p.gpuSourceHeight).toInt();
    if (p.gpuSourceHeight < 0)    p.gpuSourceHeight = 0;
    if (p.gpuSourceHeight > 4096) p.gpuSourceHeight = 4096;
    s.endGroup();

    // V32.34+: one-time migration. The compile-time default for capture
    // dimensions changed from 480×640 to 1280×720. Installs that have
    // the old 480×640 PERSISTED look stuck at low resolution even after
    // upgrading. We can't tell whether persisted-as-old-default is "user
    // explicitly chose this" vs "user never touched it" — but the old
    // default was unreasonably low for modern hardware, and any user
    // who explicitly wanted that exact size can just re-enter it via
    // Settings. So we treat exactly-the-old-default as legacy and bump.
    //
    // Done OUTSIDE the group block so we can write back to persistent
    // storage if we migrate. Guarded by a versioned "migration done"
    // flag so we run it at most once per install.
    {
        QSettings ms;
        ms.beginGroup(QLatin1String(kGroup));
        const bool alreadyMigrated = ms.value(QStringLiteral("migrated_v32_34_capture_default"), false).toBool();
        ms.endGroup();
        if (!alreadyMigrated) {
            if (p.captureWidth == 480 && p.captureHeight == 640) {
                p.captureWidth  = 1280;
                p.captureHeight = 720;
                ms.beginGroup(QLatin1String(kGroup));
                ms.setValue(kKeyCaptureWidth,  p.captureWidth);
                ms.setValue(kKeyCaptureHeight, p.captureHeight);
                ms.endGroup();
            }
            ms.beginGroup(QLatin1String(kGroup));
            ms.setValue(QStringLiteral("migrated_v32_34_capture_default"), true);
            ms.endGroup();
        }
    }

    // V32.34+: one-time migration to FORCE preferGpuConvert OFF. The first
    // V32.34+ build shipped with preferGpuConvert defaulting to true, and
    // the GPU pipeline was malformed (gldownload couldn't link to fdsink),
    // so any install that ran that build has preferGpuConvert=true PERSISTED
    // and a dead camera. Flipping the compile-time default to false doesn't
    // help those installs — the persisted true wins. This migration resets
    // the persisted value to false exactly once. A user who wants to re-test
    // the (now-corrected) GPU path can set it true again afterward; this
    // migration won't fire a second time, so their choice sticks.
    {
        QSettings ms;
        ms.beginGroup(QLatin1String(kGroup));
        const bool gpuMigrated = ms.value(
            QStringLiteral("migrated_v32_34_gpu_off"), false).toBool();
        if (!gpuMigrated) {
            // Only touch it if it's currently true (don't clobber a user who
            // already turned it off, and don't write the key needlessly).
            if (ms.value(kKeyPreferGpuConvert, false).toBool()) {
                ms.setValue(kKeyPreferGpuConvert, false);
                p.preferGpuConvert = false;
            }
            ms.setValue(QStringLiteral("migrated_v32_34_gpu_off"), true);
        }
        ms.endGroup();
    }

    m_policy = p;
}

void CameraPolicyStore::save() const
{
    QSettings s;
    s.beginGroup(QLatin1String(kGroup));
    s.setValue(kKeyStartupMs,         m_policy.startupTimeoutMs);
    s.setValue(kKeyFirstFrameMs,      m_policy.firstFrameTimeoutMs);
    s.setValue(kKeyEarlyExitMs,       m_policy.earlyExitWindowMs);
    s.setValue(kKeyAutoRetry,         m_policy.autoRetryAlternate);
    s.setValue(kKeyProbeStartupMs,    m_policy.probeStartupTimeoutMs);
    s.setValue(kKeyProbeFirstFrameMs, m_policy.probeFirstFrameTimeoutMs);
    s.setValue(kKeyProbeEarlyExitMs,  m_policy.probeEarlyExitWindowMs);
    s.setValue(kKeyCaptureWidth,      m_policy.captureWidth);
    s.setValue(kKeyCaptureHeight,     m_policy.captureHeight);
    s.setValue(kKeyRotation,          m_policy.rotation);
    s.setValue(kKeyFramedropMs,       m_policy.framedropTimeoutMs);
    s.setValue(kKeyStderrBufBytes,    m_policy.stderrBufMaxBytes);
    s.setValue(kKeyLogFirstFrame,     m_policy.logFirstFrameInfo);
    s.setValue(kKeySimultaneousCap,   m_policy.simultaneousCaptureSupported);
    s.setValue(kKeySourceTargetWidth,   m_policy.sourceTargetWidth);
    s.setValue(kKeySourceTargetHeight,  m_policy.sourceTargetHeight);
    s.setValue(kKeySourceTargetFps,     m_policy.sourceTargetFps);
    s.setValue(kKeyLowQualityRatio,     m_policy.lowQualityRatio);
    s.setValue(kKeyAutoQualityRestart,  m_policy.autoQualityRestart);
    s.setValue(kKeyFrameRateDegRatio,   m_policy.frameRateDegradedRatio);
    s.setValue(kKeyPreferGpuConvert,    m_policy.preferGpuConvert);
    s.setValue(kKeyStrideAlignPx,       m_policy.strideAlignPx);
    s.setValue(kKeyGpuSkipDownloadRepack, m_policy.gpuSkipDownloadRepack);
    s.setValue(kKeyGpuSourceWidth,      m_policy.gpuSourceWidth);
    s.setValue(kKeyGpuSourceHeight,     m_policy.gpuSourceHeight);
    s.endGroup();
    s.sync();
}

void CameraPolicyStore::set(const CameraPolicy &p)
{
    // Defensive: clamp values from the caller too. Settings UI can
    // construct a CameraPolicy with arbitrary numeric input from the
    // user; we don't want a stray 0 there crashing the backend with
    // a 0-ms wait that returns immediately on every check.
    CameraPolicy clamped = p;
    clamped.startupTimeoutMs         = clampMs(p.startupTimeoutMs,         50, 60'000);
    clamped.firstFrameTimeoutMs      = clampMs(p.firstFrameTimeoutMs,      100, 60'000);
    clamped.earlyExitWindowMs        = clampMs(p.earlyExitWindowMs,        100, 30'000);
    clamped.probeStartupTimeoutMs    = clampMs(p.probeStartupTimeoutMs,    50, 10'000);
    clamped.probeFirstFrameTimeoutMs = clampMs(p.probeFirstFrameTimeoutMs, 50, 10'000);
    clamped.probeEarlyExitWindowMs   = clampMs(p.probeEarlyExitWindowMs,   50, 10'000);
    clamped.captureWidth             = clampDim(p.captureWidth);
    clamped.captureHeight            = clampDim(p.captureHeight);
    clamped.framedropTimeoutMs       = clampMs(p.framedropTimeoutMs,       300, 60'000);
    clamped.stderrBufMaxBytes        = clampMs(p.stderrBufMaxBytes,        512, 65'536);

    m_policy = clamped;
    save();
    emit changed();
}

void CameraPolicyStore::resetToDefaults()
{
    m_policy = CameraPolicy{};
    save();
    emit changed();
}

} // namespace SnapClone::UI::Camera
