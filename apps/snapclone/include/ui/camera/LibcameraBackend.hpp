#pragma once

// ─── ui/camera/LibcameraBackend.hpp ──────────────────────────────────────────
//
// Direct libcamera C++ capture backend — the third CameraBackend implementation
// anticipated by CameraBackend.hpp's own comment ("direct libcamera C++
// binding … implement this interface"). Preferred over GstLaunchBackend on
// mobile Linux (postmarketOS / Fairphone 5), where the camera stack is
// libcamera-native and gst-launch through the bridge is flaky.
//
// The capture loop (generateConfiguration → FrameBufferAllocator → Request →
// requestCompleted → convert → emit) is adapted from KDE Plasma Camera
// (GPL-2.0-or-later). The pixel-format converter and the buffer-mmap helper in
// src/ui/camera/libcam/ are ported from that project — see those files' SPDX
// headers for attribution.
//
// Threading: libcamera delivers completed requests on its own internal thread.
// requestComplete() does nothing there but marshal to this object's (GUI)
// thread via QMetaObject::invokeMethod, so every CameraBackend signal still
// fires on the GUI thread per the interface contract. Running the convert on
// the GUI thread keeps this consistent with the other backends; if the
// viewfinder janks on-device, moving the loop to a dedicated QThread (as the
// reference does) is the optimization.
//
// Only compiled when SNAPCLONE_LIBCAMERA is defined (libcamera-dev present);
// CameraWidget::selectBackend() only references it under the same guard.
//
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/camera/CameraBackend.hpp"

#include <QImage>
#include <QList>
#include <QMutex>
#include <QSize>
#include <QString>

#include <atomic>
#include <map>
#include <memory>
#include <vector>

// libcamera's headers use `slots`, `emit` and `signals` as ordinary C++
// identifiers (e.g. Signal::slots(), Signal::emit()), which collide with Qt's
// keyword macros when this header is pulled into a TU that already included Qt
// (CameraWidget.cpp does). Undef them only for the libcamera includes, then
// restore — so the including TU keeps Qt's keywords. This TU itself uses the
// Q_-prefixed forms (Q_OBJECT / Q_EMIT), which are unaffected.
#pragma push_macro("slots")
#pragma push_macro("emit")
#pragma push_macro("signals")
#undef slots
#undef emit
#undef signals
#include <libcamera/libcamera.h>
#include <libcamera/framebuffer_allocator.h>
#pragma pop_macro("signals")
#pragma pop_macro("emit")
#pragma pop_macro("slots")

// The ported reference helpers (src/ui/camera/libcam/) are declared in the
// GLOBAL namespace by their upstream headers, so forward-declare them there and
// refer to them as ::Converter / ::Image below. Kept incomplete here so the
// heavy libcamera-internal headers stay out of this interface header.
class Converter;
class Image;

namespace SnapClone::UI::Camera {

class LibcameraBackend : public CameraBackend
{
    Q_OBJECT

public:
    explicit LibcameraBackend(QObject *parent = nullptr);
    ~LibcameraBackend() override;   // defined in .cpp (unique_ptr<incomplete>)

    bool start(const QString &deviceId = {}) override;
    void stop() override;
    [[nodiscard]] bool isActive() const override;

    void requestCapture() override;
    [[nodiscard]] QImage lastFrame() const override;

    [[nodiscard]] QList<DeviceInfo> availableDevices() const override;
    [[nodiscard]] QString defaultDeviceId() const override;

private:
    // libcamera lifecycle. All run on the GUI thread except onRequestCompleted.
    bool configureStream();   // generateConfiguration + format pick + validate + configure
    int  createRequests();    // allocator + map buffers + build requests
    void teardown();          // stop + free + release; idempotent

    void onRequestCompleted(libcamera::Request *request);  // libcamera thread — marshals only
    void processRequest(libcamera::Request *request, bool displayFrame);      // GUI thread — converts + emits (display) or just recycles
    void applyControls(libcamera::Request *request, bool initial);  // AWB/AE always; AF once

    static QString deviceDisplayName(const std::shared_ptr<libcamera::Camera> &cam);
    static const QList<libcamera::PixelFormat> &nativeFormats();

    std::unique_ptr<libcamera::CameraManager>       m_manager;
    std::shared_ptr<libcamera::Camera>              m_camera;
    std::unique_ptr<libcamera::CameraConfiguration> m_config;
    libcamera::Stream                              *m_stream = nullptr;
    std::unique_ptr<libcamera::FrameBufferAllocator> m_allocator;
    std::map<libcamera::FrameBuffer *, std::unique_ptr<::Image>> m_mapped;
    std::vector<std::unique_ptr<libcamera::Request>> m_requests;

    std::unique_ptr<::Converter> m_converter;
    libcamera::PixelFormat     m_format;
    QSize                      m_size;
    bool                       m_useConverter = false;  ///< true → software YUV/etc → RGB path
    int                        m_stride = 0;            ///< configured row pitch (bytes), incl. padding

    mutable QMutex m_frameMutex;
    QImage         m_lastFrame;

    QString m_activeId;
    bool    m_active = false;
    bool    m_firstFrameEmitted = false;

    // Bumped on every teardown(). A frame completion marshalled from the
    // libcamera thread captures the generation it was queued under; if a
    // camera switch (teardown + restart) has happened by the time the queued
    // call runs on the GUI thread, the generation no longer matches and the
    // stale request — already freed with the old session — is dropped instead
    // of dereferenced. Atomic because it's read on the libcamera thread.
    std::atomic<quint64> m_generation{0};
    // V32.39+: depth of preview frames queued to the GUI thread. onRequestCompleted
    // increments before posting; the GUI task decrements. When >0 after decrement,
    // newer frames are already queued, so this one is superseded — we recycle its
    // buffer but skip the expensive copy/scale/emit. Keeps the GUI thread free for
    // touch/gestures under camera load (the "double-tap choked by frame time" fix).
    std::atomic<int> m_pendingFrames{0};
};

} // namespace SnapClone::UI::Camera
