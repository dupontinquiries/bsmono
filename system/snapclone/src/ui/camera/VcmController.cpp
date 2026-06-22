#include "ui/camera/VcmController.hpp"

#include <QFile>
#include <QFileInfo>
#include <QLoggingCategory>
#include <QProcess>
#include <QThread>

#include <algorithm>
#include <fcntl.h>
#include <linux/videodev2.h>
#include <sys/ioctl.h>
#include <unistd.h>
#include <errno.h>
#include <cstring>

namespace SnapClone::UI::Camera {

Q_LOGGING_CATEGORY(lcVcm, "snapclone.ui.camera.vcm")

// V32.32: retry-on-EINTR ioctl wrapper. Standard pattern for V4L2 —
// signal delivery during ioctl returns EINTR and we want to transparently
// retry rather than treating it as a real error.
static int xioctl(int fd, unsigned long req, void *arg)
{
    int r;
    do { r = ::ioctl(fd, req, arg); } while (r == -1 && errno == EINTR);
    return r;
}

VcmController::VcmController() = default;

VcmController::~VcmController()
{
    close();
}

QString VcmController::findDw9719Subdev()
{
    // Scan /sys/class/video4linux/v4l-subdev<N>/name for "dw9719".
    // Range 0..40 covers FP5 (subdev28 typical) with headroom.
    for (int i = 0; i < 40; ++i) {
        const QString sysPath =
            QStringLiteral("/sys/class/video4linux/v4l-subdev%1/name").arg(i);
        QFile f(sysPath);
        if (!f.exists()) continue;
        if (!f.open(QIODevice::ReadOnly | QIODevice::Text)) continue;
        const QByteArray name = f.readAll().trimmed();
        f.close();
        if (name.contains("dw9719")) {
            return QStringLiteral("/dev/v4l-subdev%1").arg(i);
        }
    }
    return QString();
}

bool VcmController::open()
{
    if (m_fd >= 0) return true;  // already open

    m_subdevPath = findDw9719Subdev();
    if (m_subdevPath.isEmpty()) {
        qCWarning(lcVcm) << "No dw9719 subdev found in /sys/class/video4linux";
        return false;
    }

    m_fd = ::open(m_subdevPath.toUtf8().constData(), O_RDWR);
    if (m_fd < 0) {
        qCWarning(lcVcm) << "Cannot open" << m_subdevPath
                         << ":" << ::strerror(errno);
        return false;
    }

    // Read the focus_absolute control range from the driver. If the
    // query fails we fall back to a generous default range — better than
    // refusing to operate.
    struct v4l2_queryctrl qctrl{};
    qctrl.id = V4L2_CID_FOCUS_ABSOLUTE;
    if (xioctl(m_fd, VIDIOC_QUERYCTRL, &qctrl) == 0) {
        m_min = qctrl.minimum;
        m_max = qctrl.maximum;
        qCInfo(lcVcm) << "Opened VCM" << m_subdevPath
                      << "range" << m_min << ".." << m_max;
    } else {
        qCWarning(lcVcm) << "VIDIOC_QUERYCTRL failed; using default range";
        m_min = 0;
        m_max = 1023;
    }
    m_current = (m_min + m_max) / 2;  // assume midpoint until we move it
    return true;
}

void VcmController::close()
{
    if (m_fd >= 0) {
        ::close(m_fd);
        m_fd = -1;
    }
    m_subdevPath.clear();
}

bool VcmController::setPosition(int pos)
{
    if (m_fd < 0) return false;
    pos = std::clamp(pos, m_min, m_max);

    struct v4l2_control ctrl{};
    ctrl.id    = V4L2_CID_FOCUS_ABSOLUTE;
    ctrl.value = pos;
    if (xioctl(m_fd, VIDIOC_S_CTRL, &ctrl) < 0) {
        // Only log on the FIRST failure — once the VCM is wedged it
        // typically stays that way until rebind, and we don't need
        // spam at 30 sweeps/second.
        static bool warned = false;
        if (!warned) {
            qCWarning(lcVcm) << "VIDIOC_S_CTRL failed:" << ::strerror(errno);
            warned = true;
        }
        return false;
    }
    m_current = pos;
    return true;
}

bool VcmController::setNormalized(float pos)
{
    pos = std::clamp(pos, 0.0f, 1.0f);
    return setPosition(m_min + static_cast<int>(pos * (m_max - m_min)));
}

bool VcmController::rebindAfterStreamStart()
{
    // The dw9719 driver may have probed before the AFVDD_2P8 rail was
    // up (rail powers up when the sensor starts streaming). Unbinding
    // and re-binding the driver makes it probe again, this time with
    // power available.
    //
    // Requires write access to /sys/bus/i2c/drivers/dw9719/ — typically
    // root or a permissive udev rule. We don't escalate privileges; we
    // just try and report the outcome. If rebind fails, the user can
    // still use the camera, just without AF.

    const QString i2cAddr = QStringLiteral("17-000e");
    const QString unbindPath =
        QStringLiteral("/sys/bus/i2c/drivers/dw9719/unbind");
    const QString bindPath =
        QStringLiteral("/sys/bus/i2c/drivers/dw9719/bind");

    auto writeOne = [](const QString &path, const QString &val) -> bool {
        QFile f(path);
        if (!f.open(QIODevice::WriteOnly)) {
            qCWarning(lcVcm) << "Cannot write to" << path
                             << "— need root or udev rule;"
                             << "errno:" << f.errorString();
            return false;
        }
        const QByteArray bytes = val.toUtf8();
        const qint64 wrote = f.write(bytes);
        f.close();
        return wrote == bytes.size();
    };

    // Close our current fd before rebinding — the kernel will recreate
    // the subdev with a possibly-different minor number.
    close();

    if (!writeOne(unbindPath, i2cAddr)) {
        // Unbind itself failed — driver state unchanged, nothing to recover.
        return false;
    }
    QThread::msleep(300);

    // V32.38k: the driver is now UNBOUND. If we bail here on a transient bind
    // failure we'd leave the VCM driver detached entirely — strictly worse than
    // before (no AF AND the subdev gone) until something rebinds it. So retry
    // the bind a few times before giving up, and on total failure log loudly
    // that the device is left unbound (manual rebind / reboot needed).
    bool bound = false;
    for (int attempt = 0; attempt < 3 && !bound; ++attempt) {
        if (attempt > 0) QThread::msleep(200);
        bound = writeOne(bindPath, i2cAddr);
    }
    if (!bound) {
        qCWarning(lcVcm)
            << "dw9719 rebind FAILED to re-bind after unbind — the VCM driver "
               "is now detached. AF unavailable until a manual rebind or reboot. "
               "Check permissions on" << bindPath;
        return false;
    }
    QThread::msleep(300);

    // Re-discover and open the (possibly new) subdev.
    return open();
}

} // namespace SnapClone::UI::Camera
