// ─────────────────────────────────────────────────────────────────────────────
// CameraRegistry.hpp — single source of truth for camera metadata
//
// V32.32: consolidates camera discovery + per-camera capabilities into one
// place. Replaces:
//   - GstLaunchBackend's kKnown table (id+label only, no metadata)
//   - CameraWidget's `id.contains("front")` mirror heuristic
//   - Future PIP/composed-mode lookups that need to know which is which
//
// The registry knows about the four virtual-cam-* nodes the camera-bridge
// publishes on the FP5. Adding a sensor in the bridge means adding one
// entry here too — single point of edit. Future devices (FP6, librem 5,
// etc.) get their own entries OR the registry grows a runtime-discovery
// path that asks the bridge what it's publishing.
//
// Available() intersects the static table against actual pw-dump output
// so the dropdown reflects what's REALLY there, not what the bridge
// COULD have spawned.
// ─────────────────────────────────────────────────────────────────────────────

#pragma once
#include <QList>
#include <QString>
#include <QByteArray>
#include <QMutex>
#include <QElapsedTimer>
#include <optional>

namespace SnapClone::UI::Camera {

/// Physical facing of a sensor. Affects mirror-preview default,
/// "selfie" classification, and per-facing UI placement (front shutter
/// vs rear shutter on devices with separate controls).
enum class Facing {
    Rear,      ///< Primary back-facing camera (FP5 imx858)
    Front,     ///< Primary front-facing camera (selfie, FP5 s5kjn1)
    Tracking,  ///< Front sensor with face-tracking pipeline overlay
    Unknown,   ///< Couldn't determine — treat conservatively (no mirror)
};

/// Per-camera metadata. The registry returns these in a stable order
/// that defines the dropdown ordering: primary rear first, then any
/// ultrawide/secondary rears, then front, then tracking.
struct CameraInfo {
    QString virtualNodeId;   ///< PipeWire node name, e.g. "virtual-cam-rear"
    QString displayName;     ///< User-visible label, e.g. "Rear Camera"
    Facing  facing = Facing::Unknown;

    /// True if this camera has a voice-coil motor we can drive via
    /// VcmController. Currently only the FP5 main rear (imx858/dw9719).
    /// Front cam has a fixed-focus lens; ultrawide has no AF on FP5.
    bool    supportsFocus  = false;

    /// True if the preview should be horizontally flipped on screen
    /// (selfie convention: your motions match the preview). Captured
    /// photos and recorded videos are NOT mirrored — recipients see
    /// the real, unmirrored image.
    bool    mirrorPreview  = false;

    /// True if the camera-bridge is GUARANTEED to spawn this virtual
    /// camera whenever the bridge starts at all (rear, front, tracking).
    /// False if the bridge only spawns it conditionally (ultrawide,
    /// which requires libcamera to report a second back-facing sensor).
    /// Used by GstLaunchBackend for speculative dropdown listing — only
    /// baseline cameras get listed during the bridge-spawn grace window;
    /// non-baseline cameras require pw-dump confirmation before listing.
    bool    baseline       = false;
};

class CameraRegistry
{
public:
    static CameraRegistry &instance();

    /// All cameras the bridge could publish, filtered to those actually
    /// visible in pw-dump RIGHT NOW. Empty when the bridge hasn't
    /// started yet — the caller (GstLaunchBackend) speculatively shows
    /// baseline cameras during the spawn window, see availableSpeculative.
    [[nodiscard]] QList<CameraInfo> available() const;

    /// All cameras in the static table, regardless of pw-dump state.
    /// Used during the bridge-spawn window when pw-dump may be empty
    /// but the user needs SOMETHING in the dropdown.
    [[nodiscard]] QList<CameraInfo> allKnown() const;

    /// Look up a camera by its virtual node id. Returns empty optional
    /// if the id isn't in the static table (custom user-supplied node
    /// names aren't classified — they'd come back as Unknown facing).
    [[nodiscard]] std::optional<CameraInfo> byId(const QString &id) const;

    /// V32.38+: heuristic — does this camera model/label name a *user-facing*
    /// webcam (a built-in laptop cam, Apple FaceTime/iSight, a generic
    /// "HD Webcam", or the ov5640 UVC sensor)? Two uses: (a) classify an
    /// otherwise-"external" libcamera entry as front-facing so its self-view
    /// gets mirrored, and (b) on desktop, tell a built-in webcam from an
    /// external USB camera when neither id is in the static table. Case-
    /// insensitive substring match; static so non-member call sites can use it.
    [[nodiscard]] static bool isUserFacingWebcamModel(const QString &modelOrLabel);

    /// V32.42+: the front-facing rule list. Returns true if a camera with this
    /// model, libcamera location ("back"/"front"/"external"/""), and (optional)
    /// max capture height should be treated as user-facing by default — i.e.
    /// Facing::Front with a mirrored selfie preview. Rules, applied in order:
    ///   1. model names a known user-facing webcam (isUserFacingWebcamModel) —
    ///      e.g. Apple FaceTime/iSight, a generic webcam/UVC/USB camera.
    ///   2. location == "external": a USB/UVC webcam (on a phone or laptop, an
    ///      external camera is almost always the user-facing one).
    ///   3. max capture height ≤ 720 — a ≤720p sensor, overwhelmingly a webcam.
    ///      Pass 0 when the resolution isn't known yet to skip this rule (the
    ///      registry classifies before any stream opens, so it passes 0; rules 1
    ///      and 2 already front-face the typical ≤720p webcam, which is external).
    /// To add a default, extend kNeedles (rule 1) or add a clause here.
    [[nodiscard]] static bool shouldBeFrontFacing(const QString &model,
                                                  const QString &location,
                                                  int maxResolutionHeightPx = 0);

    /// Decide whether a camera's preview should be selfie-mirrored, given its
    /// backend id and display name. Virtual-node cameras (the libcamera/PipeWire
    /// path on FP5) carry typed mirrorPreview metadata, looked up by id. Cameras
    /// whose id isn't a known virtual node — e.g. a USB/UVC webcam opened through
    /// QtCameraBackend, whose id is a Qt device handle, not a "virtual-cam-*"
    /// name — fall back to the front-facing rule list applied to the display name
    /// (so "FaceTime HD Camera" et al. mirror correctly). This is the single
    /// decision point both the selector-changed and first-startup paths use.
    [[nodiscard]] bool mirrorForDevice(const QString &id,
                                       const QString &displayName) const;

    /// Primary rear-facing camera if available, else empty. Convenience
    /// for default-device selection.
    [[nodiscard]] std::optional<CameraInfo> primaryRear() const;

    /// V32.34+: multi-line diagnostic dump for the Settings debug panel.
    /// Includes:
    ///   • virtual-cam-* nodes visible in pw-dump
    ///   • raw libcamera_input.* nodes visible in pw-dump (with parsed i2c addrs)
    ///   • cam --list entries (if available — sensor model, path_id, location)
    ///   • applied overrides (env var + config file)
    ///   • final resolved classification (rear/front/ultrawide → which node)
    ///   • classification source (overrides / cam --list / heuristic)
    ///
    /// Output is human-readable. No PII. Safe to paste into bug reports.
    [[nodiscard]] QString dumpDiagnostics() const;

    /// V32.34+: invalidate the internal pw-dump cache so the next
    /// available()/dumpDiagnostics() call re-spawns pw-dump and reads
    /// fresh data. Called from Settings UI after the user edits
    /// camera_overrides.ini or plugs/unplugs hardware.
    ///
    /// Without this, the registry would return cached results for up
    /// to 500ms after the user's action — usually fine but visible
    /// when the Settings UI redraws a node list immediately after a
    /// change. The overhead is one extra pw-dump fork (~30ms on FP5).
    void invalidateCache();

private:
    CameraRegistry();
    /// Run pw-dump and return the raw output. Cached for kPwDumpCacheMs
    /// to avoid repeated subprocess spawns when both the virtual-cam
    /// scan AND the libcamera fallback scan want to look at it.
    [[nodiscard]] QString queryPwDumpRawCached() const;
    /// Run pw-dump and return all node.name values that start with
    /// "virtual-cam-". Cached for kPwDumpCacheMs to avoid repeated
    /// subprocess spawns during rapid availableDevices() calls.
    [[nodiscard]] QStringList queryPwDumpCached() const;
    /// V32.33+: fallback discovery for devices where the virtual-cam-*
    /// bridge isn't operational. Scans pw-dump output for
    /// libcamera_input.* nodes and applies heuristic facing classification
    /// (with per-device override via env var or
    /// ~/.config/snapclone/camera_overrides.ini).
    [[nodiscard]] QList<CameraInfo> libcameraFallback() const;

    /// V32.34+: pw-dump cache. Previously these were function-local
    /// statics inside queryPwDumpRawCached() — instance state moved
    /// here so invalidateCache() can reset them from outside. Mutable
    /// because they're modified from a const-qualified accessor (the
    /// cache itself is incidental implementation detail; the abstract
    /// contract is "return current state of pw-dump", which is const).
    mutable QMutex          m_pwDumpMu;
    mutable QElapsedTimer   m_pwDumpTimer;
    mutable QByteArray      m_pwDumpRaw;

    // V32.34+: classified-result cache. The raw pw-dump above is cached for
    // kPwDumpCacheMs, but libcameraFallback() still re-PARSES it and re-runs
    // the `cam --list` subprocess + classification on EVERY call, even within
    // one cache window. On FP5 (where the virtual-cam bridge is absent so
    // available() always lands in the fallback) a ~250ms-cadence caller turned
    // this into a re-enumeration storm: a `cam` spawn + 6 log lines four times
    // a second, continuously, while the camera page is open (battery/thermal
    // drain + log flood). We now cache the resolved device list keyed on the
    // SAME timer/window, and only log when the result actually CHANGES.
    mutable QElapsedTimer       m_classifiedTimer;
    mutable QList<CameraInfo>   m_classifiedCache;
    mutable bool                m_classifiedValid = false;
    mutable QString             m_lastClassifiedSig;  ///< for log-on-change
};

} // namespace SnapClone::UI::Camera
