// SPDX-FileCopyrightText: 2025 Andrew Wang
// SPDX-FileCopyrightText: 2025 Devin Lin <devin@kde.org>
// SPDX-License-Identifier: GPL-2.0-or-later
//
// LibcameraBackend — direct libcamera C++ CameraBackend for SnapClone.
//
// The viewfinder capture loop is adapted from KDE Plasma Camera's Worker
// (src/plasmacamera/worker.cpp, GPL-2.0-or-later); the pixel-format converter
// and buffer-mmap helper under src/ui/camera/libcam/ are ported from the same
// project. This file fits that proven libcamera flow to SnapClone's
// CameraBackend interface and runs it on the GUI thread (see the header).

#include "ui/camera/LibcameraBackend.hpp"

#include "libcam/converter.h"
#include "libcam/image.h"

#include <QDebug>
#include <QStringList>
#include <QSettings>

#include <algorithm>

#include <libcamera/control_ids.h>
#include <libcamera/formats.h>
#include <libcamera/property_ids.h>

using namespace libcamera;

namespace SnapClone::UI::Camera {

// libcamera 0.7 returns std::string_view from several accessors (id(),
// PixelFormat/ColorSpace::toString(), the Model property) where 0.2 returned
// std::string. This converts either to QString without assuming which — both
// expose data()/size() — so the backend stays buildable across libcamera
// versions. Pass size explicitly since string_view isn't NUL-terminated.
template <typename S>
static inline QString lcQString(const S &s)
{
    return QString::fromUtf8(s.data(), static_cast<qsizetype>(s.size()));
}

// RGB pixel formats QImage can wrap directly (no software conversion needed).
// Mirrors Plasma Camera's NATIVE_FORMATS table.
static const QMap<PixelFormat, QImage::Format> kNativeFormats{
    {formats::ABGR8888, QImage::Format_RGBX8888},
    {formats::XBGR8888, QImage::Format_RGBX8888},
    {formats::ARGB8888, QImage::Format_RGB32},
    {formats::XRGB8888, QImage::Format_RGB32},
    {formats::RGB888,   QImage::Format_BGR888},
    {formats::BGR888,   QImage::Format_RGB888},
    {formats::RGB565,   QImage::Format_RGB16},
};

const QList<PixelFormat> &LibcameraBackend::nativeFormats()
{
    static const QList<PixelFormat> f = kNativeFormats.keys();
    return f;
}

LibcameraBackend::LibcameraBackend(QObject *parent)
    : CameraBackend(parent)
    , m_converter(std::make_unique<Converter>())
{
    m_manager = std::make_unique<CameraManager>();
    if (int ret = m_manager->start(); ret < 0) {
        qWarning() << "[libcamera] CameraManager::start() failed:" << ret;
        // Leave m_manager null → availableDevices() reports none → selectBackend()
        // falls through to the next backend. (Desktops without a libcamera
        // pipeline handler land here, which is fine.)
        m_manager.reset();
    } else {
        // One-time enumeration dump: which sensors libcamera actually sees and
        // how each is tagged. This is the fix-vs-fallback question for the front
        // camera: if FRONT is absent here, the FP5's libcamera has no front-
        // sensor pipeline and only gst (via the camera-bridge) can drive it; if
        // FRONT is present but location=unknown, it's a tagging issue we can fix.
        const auto cams = m_manager->cameras();
        qInfo().noquote() << "[libcamera] enumerated" << cams.size() << "camera(s):";
        for (const std::shared_ptr<libcamera::Camera> &c : cams) {
            QString loc = QStringLiteral("unknown");
            if (auto l = c->properties().get(properties::Location)) {
                switch (*l) {
                case properties::CameraLocationFront:    loc = QStringLiteral("FRONT"); break;
                case properties::CameraLocationBack:     loc = QStringLiteral("BACK"); break;
                case properties::CameraLocationExternal: loc = QStringLiteral("EXTERNAL"); break;
                default: break;
                }
            }
            QString model;
            if (auto m = c->properties().get(properties::Model))
                model = lcQString(*m);
            qInfo().noquote() << "[libcamera]   -" << lcQString(c->id())
                              << "location=" << loc << "model=" << model;
        }
    }
}

LibcameraBackend::~LibcameraBackend()
{
    teardown();
    if (m_manager)
        m_manager->stop();
}

QString LibcameraBackend::deviceDisplayName(const std::shared_ptr<libcamera::Camera> &cam)
{
    const ControlList &props = cam->properties();
    QString label;

    if (auto loc = props.get(properties::Location)) {
        switch (*loc) {
        case properties::CameraLocationFront:    label = QStringLiteral("Front Camera"); break;
        case properties::CameraLocationBack:     label = QStringLiteral("Rear Camera"); break;
        case properties::CameraLocationExternal: label = QStringLiteral("External Camera"); break;
        default: break;
        }
    }
    if (auto model = props.get(properties::Model)) {
        const QString m = lcQString(*model);
        label = label.isEmpty() ? m : (label + QStringLiteral(" (") + m + QStringLiteral(")"));
    }
    if (label.isEmpty())
        label = lcQString(cam->id());
    return label;
}

QList<DeviceInfo> LibcameraBackend::availableDevices() const
{
    QList<DeviceInfo> out;
    if (!m_manager)
        return out;
    for (const std::shared_ptr<libcamera::Camera> &cam : m_manager->cameras())
        out.push_back(DeviceInfo{lcQString(cam->id()), deviceDisplayName(cam)});
    return out;
}

QString LibcameraBackend::defaultDeviceId() const
{
    if (!m_manager)
        return {};
    const auto cams = m_manager->cameras();
    // Prefer a rear/back sensor; else the first enumerated camera.
    for (const auto &cam : cams) {
        auto loc = cam->properties().get(properties::Location);
        if (loc && *loc == properties::CameraLocationBack)
            return lcQString(cam->id());
    }
    return cams.empty() ? QString() : lcQString(cams.front()->id());
}

bool LibcameraBackend::start(const QString &deviceId)
{
    if (!m_manager)
        return false;

    if (m_active) {
        if (deviceId.isEmpty() || deviceId == m_activeId)
            return true;   // already running the requested device
        teardown();        // switching devices
    }

    const QString id = deviceId.isEmpty() ? defaultDeviceId() : deviceId;
    if (id.isEmpty())
        return false;

    m_camera = m_manager->get(id.toStdString());
    if (!m_camera) {
        Q_EMIT error(QStringLiteral("libcamera: no such camera '%1'").arg(id));
        return false;
    }
    if (m_camera->acquire() < 0) {
        Q_EMIT error(QStringLiteral("libcamera: failed to acquire '%1' (in use?)").arg(id));
        m_camera.reset();
        return false;
    }

    if (!configureStream()) { teardown(); return false; }
    if (createRequests() < 0) {
        Q_EMIT error(QStringLiteral("libcamera: could not allocate/build capture requests"));
        teardown();
        return false;
    }

    m_camera->requestCompleted.connect(this, &LibcameraBackend::onRequestCompleted);

    if (m_camera->start() < 0) {
        Q_EMIT error(QStringLiteral("libcamera: camera->start() failed"));
        teardown();
        return false;
    }
    for (std::unique_ptr<Request> &req : m_requests) {
        applyControls(req.get(), true);
        if (m_camera->queueRequest(req.get()) < 0) {
            Q_EMIT error(QStringLiteral("libcamera: queueRequest() failed"));
            teardown();
            return false;
        }
    }

    m_activeId = id;
    m_active = true;
    m_firstFrameEmitted = false;
    return true;
}

bool LibcameraBackend::configureStream()
{
    m_config = m_camera->generateConfiguration({StreamRole::Viewfinder});
    if (!m_config || m_config->empty()) {
        Q_EMIT error(QStringLiteral("libcamera: no viewfinder configuration available"));
        return false;
    }

    StreamConfiguration &cfg = m_config->at(0);

    // Prefer a format QImage can wrap with zero-copy; otherwise the software
    // Converter handles it (YUV/MJPEG/etc). Set SNAPCLONE_LIBCAMERA_SOFTWARE_CONVERT
    // to skip the zero-copy path entirely and always convert — a one-build A/B
    // test for whether a colour tint comes from the native QImage byte-order
    // mapping vs the YUV→RGB conversion.
    const bool forceConvert = qEnvironmentVariableIsSet("SNAPCLONE_LIBCAMERA_SOFTWARE_CONVERT");
    const std::vector<PixelFormat> avail = cfg.formats().pixelformats();
    if (!forceConvert) {
        for (const PixelFormat &want : nativeFormats()) {
            if (std::find(avail.begin(), avail.end(), want) != avail.end()) {
                cfg.pixelFormat = want;
                break;
            }
        }
    }

    // Ask for a preview-sized stream so the ISP downscales in hardware, instead
    // of us CPU-copying + scaling a full-sensor frame (e.g. 2040x1536 ≈ 12.5 MB)
    // on the GUI thread every frame — the main reason the direct path felt
    // slower than gst, which negotiated 1280x720 in its own pipeline. validate()
    // snaps this to the nearest size the pipeline actually supports (the result
    // is logged below). Default 1280x960 keeps the sensor's 4:3 aspect and stays
    // sharper than gst's 720p. Tunable via [camera] libcameraPreviewWidth/Height
    // in SnapClone.conf; set either to 0 to keep libcamera's full-res default.
    {
        QSettings s;
        const int pw = s.value(QStringLiteral("camera/libcameraPreviewWidth"), 1280).toInt();
        const int ph = s.value(QStringLiteral("camera/libcameraPreviewHeight"), 960).toInt();
        if (pw > 0 && ph > 0)
            cfg.size = libcamera::Size(static_cast<unsigned>(pw), static_cast<unsigned>(ph));
    }

    if (m_config->validate() == CameraConfiguration::Invalid) {
        Q_EMIT error(QStringLiteral("libcamera: invalid camera configuration"));
        return false;
    }

    // V32.39+: some FP5 sensors (notably the front s5kjn1) expose NO viewfinder
    // downscale — validate() snaps our 1280×960 request all the way up to the
    // full sensor resolution (logs show 8152×6144 ≈ 50 MP). Allocating capture
    // buffers at that size fails outright ("could not allocate/build capture
    // requests") and, even if it didn't, converting a ~200 MB frame per tick is
    // hopeless. Rather than attempt it and emit a confusing allocation error,
    // decline the direct path for this sensor with a clear message. The
    // GStreamer (camera-bridge) backend CAN scale it — Settings → Camera lets
    // the user pick "GStreamer" for this camera. The rear (validates to
    // 2040×1536) stays well under the cap and is unaffected.
    {
        const unsigned cw = m_config->at(0).size.width;
        const unsigned ch = m_config->at(0).size.height;
        constexpr unsigned kMaxPreviewDim = 4096;
        if (cw > kMaxPreviewDim || ch > kMaxPreviewDim) {
            Q_EMIT error(QStringLiteral("libcamera: this sensor has no preview-sized mode "
                                        "(validated to %1\u00d7%2); select the GStreamer backend "
                                        "for this camera in Settings \u2192 Camera")
                             .arg(cw).arg(ch));
            return false;
        }
    }

    if (m_camera->configure(m_config.get()) < 0) {
        Q_EMIT error(QStringLiteral("libcamera: configure() failed"));
        return false;
    }

    m_stream = cfg.stream();
    m_format = cfg.pixelFormat;
    m_size = QSize(static_cast<int>(cfg.size.width), static_cast<int>(cfg.size.height));
    m_useConverter = forceConvert || !kNativeFormats.contains(m_format);
    m_stride = static_cast<int>(cfg.stride);

    if (m_useConverter) {
        const int ret = m_converter->configure(m_format, m_size, cfg.stride,
                                                cfg.colorSpace.value_or(ColorSpace::Sycc));
        if (ret < 0) {
            Q_EMIT error(QStringLiteral("libcamera: no converter for format %1")
                             .arg(lcQString(m_format.toString())));
            return false;
        }
    }

    // Diagnostic: exactly what got negotiated, per camera. A whole-session
    // colour tint that changes between cameras or launches is almost always
    // here — the chosen pixel format, the colour space (which drives the
    // YUV→RGB matrix), and whether we're zero-copy or converting.
    QStringList offered;
    for (const PixelFormat &pf : avail)
        offered << lcQString(pf.toString());
    qInfo().noquote() << "[libcamera]" << lcQString(m_camera->id())
                      << "format=" << lcQString(m_format.toString())
                      << QStringLiteral("size=%1x%2").arg(m_size.width()).arg(m_size.height())
                      << "stride=" << cfg.stride
                      << "colorspace=" << lcQString(ColorSpace::toString(cfg.colorSpace))
                      << "path=" << (m_useConverter ? "software-convert" : "native-zerocopy");
    qInfo().noquote() << "[libcamera] offered formats:" << offered.join(QStringLiteral(", "));
    // Which auto-3A controls this pipeline actually advertises. If AfMode /
    // AeEnable show 0 here, the FP5's libcamera pipeline doesn't expose
    // autofocus / auto-exposure (common on the simple/soft-ISP path), which is
    // why they wouldn't engage no matter what we request.
    {
        const ControlInfoMap &cc = m_camera->controls();
        qInfo().noquote() << "[libcamera] 3A advertised:"
                          << "AeEnable=" << (cc.count(&controls::AeEnable) ? 1 : 0)
                          << "AwbEnable=" << (cc.count(&controls::AwbEnable) ? 1 : 0)
                          << "AfMode=" << (cc.count(&controls::AfMode) ? 1 : 0)
                          << "LensPosition=" << (cc.count(&controls::LensPosition) ? 1 : 0);
    }
    return true;
}

int LibcameraBackend::createRequests()
{
    m_allocator = std::make_unique<FrameBufferAllocator>(m_camera);
    if (m_allocator->allocate(m_stream) < 0)
        return -1;

    for (const std::unique_ptr<FrameBuffer> &buffer : m_allocator->buffers(m_stream)) {
        std::unique_ptr<::Image> image = ::Image::fromFrameBuffer(buffer.get(), ::Image::MapMode::ReadOnly);
        if (!image)
            return -1;
        m_mapped[buffer.get()] = std::move(image);

        std::unique_ptr<Request> request = m_camera->createRequest();
        if (!request)
            return -1;
        if (request->addBuffer(m_stream, buffer.get()) < 0)
            return -1;
        m_requests.push_back(std::move(request));
    }
    return 0;
}

void LibcameraBackend::applyControls(Request *request, bool initial)
{
    // Make sure the ISP's auto white-balance / auto-exposure are running.
    // Without these, some pipelines leave the sensor at a fixed white balance,
    // giving a permanent colour cast that tracks the ambient light at startup
    // (warm light → red, fluorescent / LED → green). AWB/AE are sticky modes,
    // so re-asserting them each frame is harmless and keeps them on. Only set
    // controls the camera actually advertises, so we never poison a request.
    if (!m_camera)
        return;
    ControlList &ctrls = request->controls();
    const ControlInfoMap &avail = m_camera->controls();
    if (avail.count(&controls::AwbEnable))
        ctrls.set(controls::AwbEnable, true);
    if (avail.count(&controls::AeEnable))
        ctrls.set(controls::AeEnable, true);
    // Continuous autofocus, set ONCE on the opening requests: re-asserting the
    // same AF mode every frame can restart the focus search on some pipelines
    // (endless hunting), so it goes only on the initial queue.
    if (initial && avail.count(&controls::AfMode))
        ctrls.set(controls::AfMode, controls::AfModeContinuous);
}

// libcamera's thread. Do nothing heavy here — just hop to the GUI thread.
void LibcameraBackend::onRequestCompleted(Request *request)
{
    if (request->status() == Request::RequestCancelled)
        return;
    // Capture the session generation now; if a teardown/switch happens before
    // this runs on the GUI thread, the request below will have been freed, so
    // the GUI-side check drops it rather than touching a dangling pointer.
    const quint64 gen = m_generation.load();
    // V32.39+: track how many preview frames are queued to the GUI thread. If
    // frames complete faster than the GUI can draw them (the camera page is
    // busy, a gesture is in flight), the queue backs up and touch events wait
    // behind frame events — that's the double-tap "choked by frame time" under
    // direct libcamera (gst hides this by doing the work in a subprocess). We
    // still post every request (its buffer must be recycled), but the GUI task
    // skips the heavy copy/scale/emit for any frame that already has newer ones
    // queued behind it — only the freshest frame is actually drawn.
    m_pendingFrames.fetch_add(1, std::memory_order_relaxed);
    QMetaObject::invokeMethod(
        this,
        [this, request, gen]() {
            const int behind = m_pendingFrames.fetch_sub(1, std::memory_order_relaxed) - 1;
            if (gen != m_generation.load())
                return;  // stale completion from a torn-down/switched session
            processRequest(request, /*displayFrame=*/behind == 0);
        },
        Qt::QueuedConnection);
}

// GUI thread. `displayFrame` is false when newer frames are already queued —
// then we skip the copy/scale/emit and only recycle the buffer, so the GUI
// thread isn't spent rendering frames that are about to be replaced.
void LibcameraBackend::processRequest(Request *request, bool displayFrame)
{
    // Guard FIRST: stop()/teardown() may have run between completion and this
    // queued call, freeing `request`. Do not dereference it if we're inactive.
    if (!m_active)
        return;

    const auto &buffers = request->buffers();
    auto it = buffers.find(m_stream);
    if (it != buffers.end()) {
        FrameBuffer *buffer = it->second;
        const FrameMetadata &meta = buffer->metadata();

        // Only present GOOD frames. libcamera can complete a request whose
        // buffer is FrameError / FrameCancelled, or not fully filled (zero
        // bytesused) — those carry stale or uninitialized data, which is what
        // shows up as the intermittent solid red / green overlays. Skip them
        // (the request is re-queued below) instead of converting garbage.
        const bool good = meta.status == FrameMetadata::FrameSuccess
                          && !meta.planes().empty()
                          && meta.planes()[0].bytesused > 0
                          && m_size.height() > 0;
        // find (not operator[]) so a buffer somehow absent from m_mapped can't
        // silently insert a null entry and crash on the deref below.
        auto mit = m_mapped.find(buffer);
        ::Image *image = (mit != m_mapped.end()) ? mit->second.get() : nullptr;
        if (displayFrame && good && image) {
            const size_t used = meta.planes()[0].bytesused;

            QImage frame;
            if (!m_useConverter) {
                // bytesPerLine MUST be the configured row pitch (m_stride),
                // which includes any alignment padding the ISP added. Deriving
                // it from bytesused/height breaks when the driver reports a
                // tight bytesused for a padded buffer: the pitch comes out too
                // short, every row starts a few bytes early, and the frame
                // walks diagonally / repeats — the "too short → tiling" bug.
                // Fall back to the old estimate only if stride is unknown.
                const qsizetype bpl = m_stride > 0
                    ? static_cast<qsizetype>(m_stride)
                    : static_cast<qsizetype>(used) / m_size.height();
                frame = QImage(image->data(0).data(),
                               m_size.width(), m_size.height(),
                               bpl,
                               kNativeFormats[m_format])
                            .copy();
            } else {
                frame = QImage(m_size, QImage::Format_RGB32);
                m_converter->convert(image, used, &frame);
            }

            {
                QMutexLocker lk(&m_frameMutex);
                m_lastFrame = frame;
            }
            if (!m_firstFrameEmitted) {
                m_firstFrameEmitted = true;
                Q_EMIT firstFrameReceived();
            }
            Q_EMIT framePreview(frame);
        }
    }

    // Recycle the request for the next frame (re-applying the 3A controls).
    if (m_active && m_camera) {
        request->reuse(Request::ReuseBuffers);
        applyControls(request, false);
        m_camera->queueRequest(request);
    }
}

void LibcameraBackend::teardown()
{
    // Invalidate any frame completions still queued from this session: their
    // captured generation will no longer match, so they self-drop on the GUI
    // thread instead of dereferencing the requests we free just below.
    ++m_generation;

    if (m_camera) {
        m_camera->stop();
        m_camera->requestCompleted.disconnect(this);
    }
    m_requests.clear();
    m_config.reset();
    if (m_allocator) {
        if (m_stream)
            m_allocator->free(m_stream);
        m_allocator.reset();
    }
    m_mapped.clear();
    m_stream = nullptr;
    if (m_camera) {
        m_camera->release();
        m_camera.reset();
    }
    m_active = false;

    // Drop the last frame so lastFrame()/requestCapture() can't hand back the
    // previous camera's image in the gap between a switch and its first frame —
    // in a view-once app, capturing the camera you just switched away from
    // (e.g. the selfie cam you flipped off) is a real leak.
    {
        QMutexLocker lk(&m_frameMutex);
        m_lastFrame = QImage();
    }
    m_firstFrameEmitted = false;
}

void LibcameraBackend::stop()
{
    if (!m_active && !m_camera)
        return;
    const bool was = m_active;
    teardown();
    if (was)
        Q_EMIT stopped();
}

bool LibcameraBackend::isActive() const
{
    return m_active;
}

void LibcameraBackend::requestCapture()
{
    // The viewfinder stream is already at the sensor's preview resolution.
    // Switching to a StillCapture config means restarting the camera, which is
    // too slow for a snap — so capture the most recent preview frame, matching
    // the CameraBackend default contract.
    Q_EMIT frameCaptured(lastFrame());
}

QImage LibcameraBackend::lastFrame() const
{
    QMutexLocker lk(&m_frameMutex);
    return m_lastFrame;
}

} // namespace SnapClone::UI::Camera
