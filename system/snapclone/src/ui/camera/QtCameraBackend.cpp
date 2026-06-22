#include "ui/camera/QtCameraBackend.hpp"
#include "ui/camera/FramePixelOps.hpp"   // V32.39+: shared SIMD YUV→RGB

#include <QCamera>
#include <QMediaCaptureSession>
#include <QImageCapture>
#include <QVideoSink>
#include <QVideoFrame>
#include <QCameraDevice>
#include <QLoggingCategory>

namespace SnapClone::UI::Camera {

namespace PixelOps = SnapClone::Camera::PixelOps;  // V32.39+: shared SIMD pixel ops

Q_LOGGING_CATEGORY(lcQtCam, "snapclone.camera.qt")

QtCameraBackend::QtCameraBackend(QObject *parent)
    : CameraBackend(parent)
    , m_mediaDevices(this)
{
    // Re-emit Qt's hotplug signal as our backend-agnostic one.
    connect(&m_mediaDevices, &QMediaDevices::videoInputsChanged,
            this, &CameraBackend::deviceListChanged);
}

QtCameraBackend::~QtCameraBackend()
{
    stop();
}

QList<DeviceInfo> QtCameraBackend::availableDevices() const
{
    QList<DeviceInfo> out;
    for (const QCameraDevice &dev : QMediaDevices::videoInputs()) {
        out.append({ QString::fromLatin1(dev.id()), dev.description() });
    }
    return out;
}

QString QtCameraBackend::defaultDeviceId() const
{
    const QCameraDevice def = QMediaDevices::defaultVideoInput();
    if (!def.isNull()) return QString::fromLatin1(def.id());
    const auto all = QMediaDevices::videoInputs();
    if (!all.isEmpty()) return QString::fromLatin1(all.first().id());
    return {};
}

bool QtCameraBackend::start(const QString &deviceId)
{
    // Resolve device: caller's hint, else default.
    const QString id = deviceId.isEmpty() ? defaultDeviceId() : deviceId;
    if (id.isEmpty()) {
        emit error(tr("No video input devices available"));
        return false;
    }

    // Already running with the same device? Just resume.
    if (m_session && m_camera && m_camera->cameraDevice().id() == id.toLatin1()) {
        if (!m_camera->isActive()) m_camera->start();
        return true;
    }

    buildPipeline(id.toLatin1());
    return m_camera && m_camera->isActive();
}

void QtCameraBackend::stop()
{
    // FULL release: not just QCamera::stop() (which leaves the V4L2 file
    // descriptor open and the USB bandwidth reservation held), but tear
    // down the entire QCamera/QSession/QVideoSink chain. The kernel then
    // processes VIDIOC_STREAMOFF + USB alt-setting reset, which releases
    // the periodic-bandwidth slot — without this, gst-launch's v4l2src
    // hits "Buffer pool activation failed" because the USB host controller
    // refuses STREAMON ("not enough bandwidth for new device state").
    //
    // The cost is: on resume, start() rebuilds the pipeline from scratch
    // (~50-200ms initialization). That's acceptable since we only release
    // when explicitly stopping for a recording, and recording is a
    // mode-switch the user already expects to take a moment.
    //
    // Why this is safer than QCamera::stop(): Qt's QCamera::stop() is
    // documented as halting frame production but the FFmpeg backend
    // doesn't always close the FD or call STREAMOFF synchronously — on
    // some kernels the FD survives the QCamera object until the next
    // event loop tick, on others it survives until QCamera is destroyed.
    if (m_camera && m_camera->isActive())
        m_camera->stop();
    // Disconnect signals BEFORE deleting so any in-flight frame events
    // don't reach a half-destroyed sink.
    if (m_videoSink) {
        m_videoSink->disconnect();
        m_videoSink->deleteLater();
        m_videoSink = nullptr;
    }
    m_imageCapture.reset();
    m_session.reset();
    m_camera.reset();
    emit stopped();
}

bool QtCameraBackend::isActive() const
{
    return m_camera && m_camera->isActive();
}

void QtCameraBackend::buildPipeline(const QByteArray &qtDeviceId)
{
    // Find the QCameraDevice that matches the id
    QCameraDevice target;
    for (const QCameraDevice &d : QMediaDevices::videoInputs()) {
        if (d.id() == qtDeviceId) { target = d; break; }
    }
    if (target.isNull()) {
        emit error(tr("Device id '%1' not found in QMediaDevices").arg(QString::fromLatin1(qtDeviceId)));
        return;
    }

    // Tear down previous pipeline if any
    if (m_camera) m_camera->stop();
    m_camera.reset();
    m_imageCapture.reset();
    if (m_videoSink) {
        m_videoSink->disconnect();
        m_videoSink->deleteLater();
        m_videoSink = nullptr;
    }
    m_session.reset();

    // Build new pipeline
    m_camera = std::make_unique<QCamera>(target);
    m_session = std::make_unique<QMediaCaptureSession>();
    m_imageCapture = std::make_unique<QImageCapture>();

    m_videoSink = new QVideoSink(this);
    m_session->setVideoSink(m_videoSink);
    m_session->setImageCapture(m_imageCapture.get());
    m_session->setCamera(m_camera.get());

    connect(m_videoSink, &QVideoSink::videoFrameChanged,
            this, &QtCameraBackend::onVideoFrame);

    connect(m_imageCapture.get(), &QImageCapture::imageCaptured,
            this, [this](int /*id*/, const QImage &image) {
        emit frameCaptured(image);
    });

    connect(m_camera.get(), &QCamera::errorOccurred,
            this, [this](QCamera::Error /*err*/, const QString &msg) {
        emit error(msg);
    });

    m_camera->start();
    qCInfo(lcQtCam) << "Started Qt camera:" << target.description();
}

void QtCameraBackend::onVideoFrame(const QVideoFrame &frame)
{
    if (!frame.isValid()) return;

    // Diagnostic: log the source pixel format ONCE so we can tell if Qt's
    // FFmpeg backend is handing us a format that toImage() then mishandles.
    // Some cameras (FaceTime HD on Asahi/aarch64) produce YUV frames that
    // toImage() converts incorrectly when V4L2_MEMORY_USERPTR fails and
    // stride alignment isn't honored — visible as a green-banded mess.
    static QVideoFrameFormat::PixelFormat s_lastLoggedFmt =
        QVideoFrameFormat::Format_Invalid;
    const auto srcFmt = frame.pixelFormat();
    if (srcFmt != s_lastLoggedFmt) {
        const bool haveExplicit =
            srcFmt == QVideoFrameFormat::Format_NV12 ||
            srcFmt == QVideoFrameFormat::Format_NV21 ||
            srcFmt == QVideoFrameFormat::Format_YUV420P;
        qCInfo(lcQtCam) << "Frame source format:" << srcFmt
                        << "size:" << frame.width() << "x" << frame.height()
                        << (haveExplicit ? "(explicit stride-safe path)"
                                         : "(FALLS THROUGH to toImage — if this "
                                           "camera shows tiling/skew, THIS format "
                                           "needs an explicit path)");
        s_lastLoggedFmt = srcFmt;
    }

    QImage img;

    // ── Explicit YUV conversion path (NV12 / NV21 / YUV420P) ─────────────────
    // Qt's frame.toImage() on aarch64 (Asahi desktop AND the Fairphone rear
    // camera) produces tiled/skewed output for planar/semi-planar YUV when the
    // V4L2 backend falls back to MMAP and the row STRIDE (bytesPerLine) isn't
    // honored — toImage() treats the Y-plane stride as if it were the RGB row
    // pitch, so every row is offset by (stride - width*bpp), giving the
    // diagonal "skiing" skew + the apparent vertical tiling in the device
    // photo. We do the BT.601 conversion ourselves, addressing every plane by
    // its real bytesPerLine, which is correct regardless of stride padding.
    //
    // NV12: plane0=Y, plane1=interleaved UV (U at even, V at odd).
    // NV21: plane0=Y, plane1=interleaved VU (V at even, U at odd) — UV swapped.
    // YUV420P (I420): plane0=Y, plane1=U (full plane), plane2=V (full plane).
    const bool isNV12 = (srcFmt == QVideoFrameFormat::Format_NV12);
    const bool isNV21 = (srcFmt == QVideoFrameFormat::Format_NV21);
    const bool isI420 = (srcFmt == QVideoFrameFormat::Format_YUV420P);
    if (isNV12 || isNV21 || isI420) {
        QVideoFrame vf(frame);  // Map needs non-const access
        if (vf.map(QVideoFrame::ReadOnly)) {
            const int w = vf.width();
            const int h = vf.height();
            const uchar *yPlane = vf.bits(0);
            const int yStride   = vf.bytesPerLine(0);
            // Chroma planes differ by layout.
            const uchar *cPlane0 = vf.bits(1);                 // UV (semi) or U (planar)
            const uchar *cPlane1 = isI420 ? vf.bits(2) : nullptr;  // V (planar only)
            const int cStride0   = vf.bytesPerLine(1);
            const int cStride1   = isI420 ? vf.bytesPerLine(2) : 0;

            const bool planesOk = yPlane && cPlane0 && yStride > 0
                                && cStride0 > 0 && w > 0 && h > 0
                                && (!isI420 || (cPlane1 && cStride1 > 0));
            if (planesOk) {
                img = QImage(w, h, QImage::Format_RGB888);
                // V32.39+: shared SIMD YUV→RGB (NEON on ARM; scalar elsewhere).
                // Identical BT.601 integer math to the old inline loop, now in
                // one optimized, self-tested place reused across backends.
                if (isI420) {
                    PixelOps::i420ToRgb888(img.bits(), img.bytesPerLine(),
                                           yPlane, yStride,
                                           cPlane0, cStride0, cPlane1, cStride1,
                                           w, h);
                } else {
                    PixelOps::nv12ToRgb888(img.bits(), img.bytesPerLine(),
                                           yPlane, yStride, cPlane0, cStride0,
                                           w, h, /*nv21=*/isNV21);
                }
            }
            vf.unmap();
        }
        // If map failed or planes were null, fall back to Qt's converter
        // (better than nothing, even if it might look bad).
        if (img.isNull()) img = frame.toImage();
    } else {
        img = frame.toImage();
    }

    if (img.isNull()) {
        qCWarning(lcQtCam) << "frame.toImage() returned null for fmt" << srcFmt;
        return;
    }

    // Stride-mismatch detection. When Qt's FFmpeg v4l2 backend can't get
    // USERPTR buffers (common on Asahi/aarch64 FaceTime HD), it falls back
    // to MMAP but doesn't always honor the driver's row-stride alignment,
    // producing frames that toImage() converts as if width=stride/bpp. The
    // visible result is heavy horizontal banding + green dominance — the
    // YUV chroma rows leaking into the luminance interpretation.
    //
    // We detect this by cross-checking img dimensions against the source
    // QVideoFrame's. If they don't match exactly, the conversion didn't
    // honor the format and the frame is unsafe to display.
    if (img.width() != frame.width() || img.height() != frame.height()) {
        // One log per dimension-mismatch session — don't spam
        static QSize s_lastBadSize;
        const QSize bad{img.width(), img.height()};
        if (bad != s_lastBadSize) {
            qCWarning(lcQtCam) << "Frame dimension mismatch: source"
                << frame.width() << "x" << frame.height()
                << "vs toImage" << img.width() << "x" << img.height()
                << "— likely stride misalignment, dropping frame";
            s_lastBadSize = bad;
        }
        return;  // skip this frame; the next may be better
    }

    // Sanity: if toImage produced an odd format we'll force-convert.
    if (img.format() != QImage::Format_RGB32 &&
        img.format() != QImage::Format_ARGB32 &&
        img.format() != QImage::Format_RGB888)
        img = img.convertToFormat(QImage::Format_RGB32);

    // YUV-as-RGB content check. When Qt's FFmpeg backend hands us frames
    // whose color conversion silently failed, the bytes pass through as if
    // they were RGB, producing the "green dominant, banded" visual we saw
    // in user screenshots on FaceTime HD over Asahi/aarch64.
    //
    // Detection: sample ~64 pixels across the image. If mean green is >2.5x
    // mean red AND mean blue, we have a YUV-bytes-as-RGB situation. Real
    // scenes (even green grass) don't produce this color skew because
    // proper RGB green is balanced by reflected light contributions to R
    // and B. False-positive risk is acceptably low — and even in rare
    // cases of legit green-dominant scenes, dropping a few frames is
    // less harmful than displaying garbled output.
    //
    // Skip this check when we did NV12 conversion ourselves above — we
    // know our converter produces correct RGB so the heuristic would only
    // false-positive on legitimate green-dominant scenes (foliage, etc.).
    if (srcFmt != QVideoFrameFormat::Format_NV12) {
        constexpr int kSamples = 8;
        qint64 sumR = 0, sumG = 0, sumB = 0;
        const int sw = img.width();
        const int sh = img.height();
        for (int y = 0; y < kSamples; ++y) {
            for (int x = 0; x < kSamples; ++x) {
                const QRgb p = img.pixel((x * sw) / kSamples,
                                          (y * sh) / kSamples);
                sumR += qRed(p); sumG += qGreen(p); sumB += qBlue(p);
            }
        }
        const qint64 n = kSamples * kSamples;
        // Explicit static_cast to silence -Wconversion: numerator is qint64 sum
        // of 8-bit channel values × n samples → max ≈ 255 × 4096 = 1M, well
        // within double's 53-bit mantissa.
        const double mR = static_cast<double>(sumR) / static_cast<double>(n);
        const double mG = static_cast<double>(sumG) / static_cast<double>(n);
        const double mB = static_cast<double>(sumB) / static_cast<double>(n);
        if (mG > 2.5 * mR + 1.0 && mG > 2.5 * mB + 1.0) {
            static int s_dropCount = 0;
            if (++s_dropCount % 30 == 1) {
                qCWarning(lcQtCam)
                    << "Skipping frame with YUV-as-RGB color skew (R/G/B ="
                    << int(mR) << "/" << int(mG) << "/" << int(mB)
                    << ") — Qt FFmpeg conversion likely failed for fmt"
                    << srcFmt << "(suppressed similar warnings)";
            }
            return;  // drop this frame
        }
    }

    m_lastFrame = img;
    emit framePreview(img);
}

void QtCameraBackend::requestCapture()
{
    if (m_imageCapture) {
        m_imageCapture->capture();
    } else if (!m_lastFrame.isNull()) {
        emit frameCaptured(m_lastFrame);
    } else {
        emit error(tr("No active capture pipeline"));
    }
}

} // namespace SnapClone::UI::Camera
