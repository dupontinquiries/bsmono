// ─────────────────────────────────────────────────────────────────────────────
// VcmController.hpp — voice-coil motor focus control for FP5 / DW9719
//
// V32.32: thin C++ wrapper around the V4L2 ioctl path that talks to the
// dw9719 VCM driver on the FP5's rear camera (imx858). The sensor is
// streamed via the camera-bridge (libcamera/PipeWire); this controller
// is a SIDE CHANNEL — it doesn't touch frames, only the lens actuator.
//
// Power sequencing notes (critical):
//   - DW9800K only responds on i2c after AFVDD_2P8 rail is on.
//   - That rail powers up when the sensor starts streaming.
//   - The dw9719 driver typically probed BEFORE the rail was up, so
//     it may need to be rebound after streaming starts.
//   - We try setPosition() first; if it returns EBUSY/ENODEV/etc.,
//     we attempt a one-shot rebind (sysfs writes need root) and retry.
//   - If rebind also fails (no root), we silently fall through and
//     focus is unavailable for the session — the user can still take
//     photos, just no AF.
//
// Discovery: the subdev path /dev/v4l-subdev28 is not stable across
// reboots. open() scans subdev 0..40 looking for one named "dw9719
// 17-000e" and uses that. Caches the result for the session.
// ─────────────────────────────────────────────────────────────────────────────

#pragma once
#include <QString>

namespace SnapClone::UI::Camera {

class VcmController
{
public:
    VcmController();
    ~VcmController();

    /// Discover and open the dw9719 subdev. Returns true if the device
    /// was found, opened, and reports a valid focus_absolute range.
    /// May fail silently on devices without a VCM (e.g. front camera).
    bool open();

    /// Close the underlying fd. Safe to call multiple times.
    void close();

    /// True if open() succeeded and we have a usable fd.
    bool isOpen() const { return m_fd >= 0; }

    /// Move the lens to an absolute DAC position. Clamped to [minPos, maxPos].
    /// Returns true if the ioctl succeeded.
    bool setPosition(int pos);

    /// Convenience: 0.0 = infinity focus, 1.0 = closest focus (macro).
    bool setNormalized(float pos);

    /// Attempt to rebind the dw9719 driver. Required after the camera
    /// streams for ~1.5s to bring up the AFVDD_2P8 rail. Returns true
    /// if the sysfs writes succeeded (which usually means root or a
    /// permissive udev rule is in place). After a successful rebind,
    /// the subdev path may have changed — call open() again.
    bool rebindAfterStreamStart();

    int minPos()  const { return m_min; }
    int maxPos()  const { return m_max; }
    int current() const { return m_current; }

    /// The subdev path we're using, for diagnostics. Empty until open().
    QString subdevPath() const { return m_subdevPath; }

private:
    /// Scan /dev/v4l-subdev0..40 for one whose /sys name contains "dw9719".
    /// Returns the path on success.
    static QString findDw9719Subdev();

    int     m_fd          = -1;
    int     m_min         = 0;
    int     m_max         = 1023;
    int     m_current     = 0;
    QString m_subdevPath;
};

} // namespace SnapClone::UI::Camera
