#pragma once

// ─── ui/camera/CameraBackend.hpp ─────────────────────────────────────────────
//
// Abstract camera capture backend.
//
// CameraWidget owns one CameraBackend at a time. Two implementations exist:
//
//   • QtCameraBackend     — Qt's QCamera + QMediaCaptureSession via the
//                           FFmpeg multimedia plugin. Used on desktops where
//                           Qt enumerates cameras correctly through V4L2 or
//                           gst-plugins-good-pipewire.
//
//   • GstLaunchBackend    — Spawns gst-launch-1.0 reading from PipeWire and
//                           pipes raw BGRx frames over stdout. Used on
//                           postmarketOS / Alpine where Qt's QCamera does
//                           not enumerate libcamera nodes through the bridge.
//
// Adding a third backend (direct libcamera C++ binding, v4l2 fallback, etc.)
// is "implement this interface" — no branching elsewhere in CameraWidget.
//
// Threading: all signals fire on the backend's thread (typically the GUI
// thread). Emit from a worker via Qt::QueuedConnection if needed.
//
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QImage>
#include <QString>
#include <QList>

namespace SnapClone::UI::Camera {

/// Backend-agnostic device descriptor.
/// Both QCameraDevice (.id() = QByteArray) and PipeWire node names map to this.
struct DeviceInfo {
    QString id;          ///< Backend-specific opaque handle (Qt device id, PW node name)
    QString displayName; ///< User-facing label ("Rear Camera", "USB Webcam")

    [[nodiscard]] bool isValid() const { return !id.isEmpty(); }
};

class CameraBackend : public QObject
{
    Q_OBJECT

public:
    explicit CameraBackend(QObject *parent = nullptr) : QObject(parent) {}
    ~CameraBackend() override = default;

    // ─── Lifecycle ───────────────────────────────────────────────────────────

    /// Start capture. `deviceId` selects a specific device (from
    /// availableDevices()). Empty string means "use the default / last-used".
    /// Returns false if startup failed synchronously (e.g. no devices).
    /// Asynchronous failures are reported via the error() signal.
    virtual bool start(const QString &deviceId = {}) = 0;

    /// Stop capture and release resources. Idempotent.
    virtual void stop() = 0;

    /// True if capture is currently producing frames.
    [[nodiscard]] virtual bool isActive() const = 0;

    // ─── Capture ─────────────────────────────────────────────────────────────

    /// Request a still capture. Result arrives asynchronously via
    /// frameCaptured(). Default impl just emits the most recent preview frame.
    virtual void requestCapture() = 0;

    /// Most recent preview frame (full resolution). Used for QR scanning
    /// without waiting for a separate capture.
    [[nodiscard]] virtual QImage lastFrame() const = 0;

    // ─── Device enumeration ──────────────────────────────────────────────────

    /// Currently known devices. May refresh on hotplug.
    [[nodiscard]] virtual QList<DeviceInfo> availableDevices() const = 0;

    /// Identifier of the most appropriate default device, or empty if none.
    /// Backends typically prefer "rear" / first-listed.
    [[nodiscard]] virtual QString defaultDeviceId() const = 0;

signals:
    /// Emitted at frame rate when a new preview frame is ready.
    /// Receivers should NOT keep references — copy if retention is needed.
    void framePreview(const QImage &frame);

    /// Emitted in response to requestCapture(). The image is detached
    /// (safe to retain).
    void frameCaptured(const QImage &frame);

    /// Device list changed (hotplug, virtual cam appeared/disappeared).
    void deviceListChanged();

    /// Backend reports a problem. Severity is opaque — the UI should log
    /// and possibly fall back to a different backend.
    void error(const QString &message);

    /// Emitted when isActive() transitions true → false unexpectedly
    /// (e.g. gst-launch process exited, QCamera went into error state).
    void stopped();

    /// V32.34+: Emitted exactly once per session, when the FIRST
    /// preview frame has been delivered. Distinct from framePreview
    /// (which fires per-frame) — this is the "the camera is
    /// confirmed working, not just opened" signal. Used by
    /// CameraHealthService to mark devices as in-use without
    /// needing per-frame hooks.
    ///
    /// Backends that can't distinguish "opened" from "delivering
    /// frames" (e.g. QtCameraBackend with QCamera::started signal)
    /// may emit this immediately after start(); the contract is
    /// "one signal per successful session", not "frame-accurate".
    void firstFrameReceived();

    /// V32.34+: Emitted when the backend internally swapped to a
    /// different node than the caller requested. This happens during
    /// "adaptive fallback" — e.g. caller asks for rear cam, that
    /// node fails to negotiate (libcamera misnamed it, hardware
    /// missing), the backend silently retries on the next available
    /// node and succeeds. The UI needs this signal to update its
    /// dropdown / banner / labels so the user isn't confused by
    /// "I asked for rear and got front".
    ///
    /// Receivers should NOT call setSelection back on the backend —
    /// that would loop. Instead reflect the new selection visually.
    void activeNodeChanged(const QString &requestedNode,
                           const QString &actualNode,
                           const QString &reason);
};

} // namespace SnapClone::UI::Camera
