#pragma once

// QtCameraBackend — wraps QCamera + QMediaCaptureSession + QImageCapture.
// Used on platforms where Qt's multimedia backend correctly enumerates
// video devices (most Linux desktops with V4L2, macOS, Windows, and
// occasionally Wayland/PipeWire setups with gst-plugins-good-pipewire).

#include "ui/camera/CameraBackend.hpp"

#include <QMediaDevices>
#include <memory>

QT_BEGIN_NAMESPACE
class QCamera;
class QMediaCaptureSession;
class QImageCapture;
class QVideoSink;
class QVideoFrame;
QT_END_NAMESPACE

namespace SnapClone::UI::Camera {

class QtCameraBackend : public CameraBackend
{
    Q_OBJECT
public:
    explicit QtCameraBackend(QObject *parent = nullptr);
    ~QtCameraBackend() override;

    bool start(const QString &deviceId = {}) override;
    void stop() override;
    [[nodiscard]] bool isActive() const override;
    void requestCapture() override;
    [[nodiscard]] QImage lastFrame() const override { return m_lastFrame; }
    [[nodiscard]] QList<DeviceInfo> availableDevices() const override;
    [[nodiscard]] QString defaultDeviceId() const override;

private:
    void buildPipeline(const QByteArray &qtDeviceId);
    void onVideoFrame(const QVideoFrame &frame);

    std::unique_ptr<QCamera>              m_camera;
    std::unique_ptr<QMediaCaptureSession> m_session;
    std::unique_ptr<QImageCapture>        m_imageCapture;
    QVideoSink                           *m_videoSink = nullptr;
    QMediaDevices                         m_mediaDevices;
    QImage                                m_lastFrame;
};

} // namespace SnapClone::UI::Camera
