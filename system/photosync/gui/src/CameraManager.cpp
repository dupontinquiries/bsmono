// CameraManager.cpp - Implementation with aggressive camera detection
#include "CameraManager.h"
#include <QDir>
#include <QFile>
#include <QDebug>
#include <QDateTime>
#include <QStandardPaths>
#include <QtConcurrent>
#include <linux/videodev2.h>
#include <sys/ioctl.h>
#include <fcntl.h>
#include <unistd.h>

CameraScanner::CameraScanner(QObject *parent)
    : QObject(parent)
{
}

void CameraScanner::scanCameras()
{
    QVariantList foundCameras;
    
    // 1. Scan V4L2 devices (most comprehensive)
    qDebug() << "Scanning V4L2 devices...";
    for (int i = 0; i < 64; i++) {
        QString devPath = QString("/dev/video%1").arg(i);
        
        int fd = ::open(devPath.toUtf8().constData(), O_RDONLY);
        if (fd < 0) continue;
        
        struct v4l2_capability cap;
        if (ioctl(fd, VIDIOC_QUERYCAP, &cap) == 0) {
            // Only include video capture devices
            if (cap.device_caps & V4L2_CAP_VIDEO_CAPTURE) {
                QVariantMap camera;
                camera["id"] = devPath;
                camera["name"] = QString::fromUtf8((const char*)cap.card);
                camera["driver"] = QString::fromUtf8((const char*)cap.driver);
                camera["bus"] = QString::fromUtf8((const char*)cap.bus_info);
                camera["type"] = "v4l2";
                camera["capabilities"] = getCameraCapabilities(devPath);
                
                foundCameras.append(camera);
                qDebug() << "Found V4L2 camera:" << camera["name"];
            }
        }
        ::close(fd);
    }
    
    // 2. Scan libcamera devices
    qDebug() << "Scanning libcamera devices...";
    scanLibcamera();
    
    // 3. Scan PipeWire sources (if available)
    qDebug() << "Scanning PipeWire sources...";
    scanPipeWire();
    
    emit camerasFound(foundCameras);
}

QVariantMap CameraScanner::getCameraCapabilities(const QString &device)
{
    QVariantMap caps;
    
    int fd = ::open(device.toUtf8().constData(), O_RDONLY);
    if (fd < 0) return caps;
    
    // Query formats
    QStringList formats;
    struct v4l2_fmtdesc fmt;
    memset(&fmt, 0, sizeof(fmt));
    fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    
    while (ioctl(fd, VIDIOC_ENUM_FMT, &fmt) == 0) {
        formats << QString::fromUtf8((const char*)fmt.description);
        fmt.index++;
    }
    caps["formats"] = formats;
    
    // Query resolutions
    QVariantList resolutions;
    struct v4l2_frmsizeenum frmsize;
    memset(&frmsize, 0, sizeof(frmsize));
    frmsize.pixel_format = V4L2_PIX_FMT_YUYV;
    
    while (ioctl(fd, VIDIOC_ENUM_FRAMESIZES, &frmsize) == 0) {
        if (frmsize.type == V4L2_FRMSIZE_TYPE_DISCRETE) {
            QVariantMap res;
            res["width"] = frmsize.discrete.width;
            res["height"] = frmsize.discrete.height;
            resolutions.append(res);
        }
        frmsize.index++;
    }
    caps["resolutions"] = resolutions;
    
    // Query controls (focus, white balance, etc.)
    QVariantList controls;
    struct v4l2_queryctrl qctrl;
    memset(&qctrl, 0, sizeof(qctrl));
    qctrl.id = V4L2_CTRL_FLAG_NEXT_CTRL;
    
    while (ioctl(fd, VIDIOC_QUERYCTRL, &qctrl) == 0) {
        if (!(qctrl.flags & V4L2_CTRL_FLAG_DISABLED)) {
            QVariantMap ctrl;
            ctrl["name"] = QString::fromUtf8((const char*)qctrl.name);
            ctrl["id"] = qctrl.id;
            ctrl["min"] = qctrl.minimum;
            ctrl["max"] = qctrl.maximum;
            ctrl["step"] = qctrl.step;
            ctrl["default"] = qctrl.default_value;
            controls.append(ctrl);
        }
        qctrl.id |= V4L2_CTRL_FLAG_NEXT_CTRL;
    }
    caps["controls"] = controls;
    
    // Check for ToF (Time-of-Flight) support
    struct v4l2_queryctrl tof_ctrl;
    memset(&tof_ctrl, 0, sizeof(tof_ctrl));
    tof_ctrl.id = V4L2_CID_3A_LOCK; // Check for advanced features
    caps["hasToF"] = (ioctl(fd, VIDIOC_QUERYCTRL, &tof_ctrl) == 0);
    
    ::close(fd);
    return caps;
}

void CameraScanner::scanLibcamera()
{
    // Scan using libcamera API
    auto cm = std::make_unique<libcamera::CameraManager>();
    cm->start();
    
    for (const auto &camera : cm->cameras()) {
        QVariantMap camInfo;
        camInfo["id"] = QString::fromStdString(camera->id());
        camInfo["name"] = QString::fromStdString(camera->id());
        camInfo["type"] = "libcamera";
        
        // Get properties
        const ControlInfoMap &controls = camera->controls();
        QVariantList supportedControls;
        
        for (const auto &[id, info] : controls) {
            QVariantMap ctrl;
            ctrl["id"] = id;
            ctrl["name"] = QString::fromStdString(id.name());
            supportedControls.append(ctrl);
        }
        
        camInfo["controls"] = supportedControls;
        
        qDebug() << "Found libcamera device:" << camInfo["name"];
    }
    
    cm->stop();
}

void CameraScanner::scanPipeWire()
{
    // Check if PipeWire is available
    QFile pwCheck("/run/pipewire-0");
    if (!pwCheck.exists()) {
        qDebug() << "PipeWire not available";
        return;
    }
    
    // TODO: Integrate with PipeWire API to enumerate camera sources
    qDebug() << "PipeWire scanning not yet implemented";
}

// ============================================================================
// CameraManager Implementation
// ============================================================================

CameraManager::CameraManager(QObject *parent)
    : QObject(parent)
    , m_scanning(false)
    , m_isRecording(false)
    , m_focusMode(FocusAuto)
    , m_whiteBalance(WBAuto)
    , m_exposure(0)
    , m_hasToF(false)
    , m_tofDistance(0.0f)
    , m_allocator(nullptr)
    , m_hdrEnabled(false)
    , m_nightModeEnabled(false)
    , m_zoomLevel(1.0f)
{
    // Set output path
    m_outputPath = QStandardPaths::writableLocation(QStandardPaths::PicturesLocation) + "/PhotoSync";
    QDir().mkpath(m_outputPath);
    
    // Initialize scanner thread
    m_scannerThread = new QThread(this);
    m_scanner = new CameraScanner();
    m_scanner->moveToThread(m_scannerThread);
    
    connect(m_scannerThread, &QThread::started, m_scanner, &CameraScanner::scanCameras);
    connect(m_scanner, &CameraScanner::camerasFound, this, &CameraManager::onCamerasFound);
    
    // ToF update timer
    m_tofUpdateTimer = new QTimer(this);
    connect(m_tofUpdateTimer, &QTimer::timeout, this, &CameraManager::updateToFDistance);
    
    // Auto-scan on startup
    QTimer::singleShot(100, this, &CameraManager::scanForCameras);
}

CameraManager::~CameraManager()
{
    stopCapture();
    if (m_scannerThread) {
        m_scannerThread->quit();
        m_scannerThread->wait();
    }
}

void CameraManager::scanForCameras()
{
    m_scanning = true;
    emit scanningChanged();
    
    m_scannerThread->start();
}

void CameraManager::onCamerasFound(QVariantList cameras)
{
    m_cameras = cameras;
    m_scanning = false;
    
    emit camerasChanged();
    emit scanningChanged();
    
    qDebug() << "Found" << cameras.size() << "cameras";
    
    // Auto-select first camera
    if (!cameras.isEmpty()) {
        QString firstCameraId = cameras[0].toMap()["id"].toString();
        selectCamera(firstCameraId);
    }
}

void CameraManager::selectCamera(const QString &cameraId)
{
    qDebug() << "Selecting camera:" << cameraId;
    
    stopCapture();
    
    m_activeCamera = cameraId;
    emit activeCameraChanged();
    
    // Initialize libcamera for this device
    initLibcamera();
    
    // Get camera properties
    for (const auto &cam : m_cameras) {
        QVariantMap camMap = cam.toMap();
        if (camMap["id"].toString() == cameraId) {
            m_cameraProperties = camMap;
            m_hasToF = camMap.value("capabilities").toMap().value("hasToF", false).toBool();
            emit cameraPropertiesChanged();
            emit hasToFChanged();
            break;
        }
    }
}

void CameraManager::initLibcamera()
{
    m_cm = std::make_shared<libcamera::CameraManager>();
    m_cm->start();
    
    for (const auto &cam : m_cm->cameras()) {
        if (QString::fromStdString(cam->id()) == m_activeCamera ||
            cam->id().find(m_activeCamera.toStdString()) != std::string::npos) {
            m_camera = cam;
            break;
        }
    }
    
    if (!m_camera) {
        emit error("Failed to open camera: " + m_activeCamera);
        return;
    }
    
    if (m_camera->acquire() < 0) {
        emit error("Failed to acquire camera");
        return;
    }
    
    configureCamera();
}

void CameraManager::configureCamera()
{
    m_config = m_camera->generateConfiguration({StreamRole::Viewfinder});
    
    if (!m_config) {
        emit error("Failed to generate camera configuration");
        return;
    }
    
    // Configure viewfinder stream
    StreamConfiguration &cfg = m_config->at(0);
    cfg.pixelFormat = formats::YUV420;
    cfg.size = Size(1920, 1080); // Start with 1080p
    
    if (m_config->validate() == CameraConfiguration::Invalid) {
        emit error("Camera configuration invalid");
        return;
    }
    
    if (m_camera->configure(m_config.get()) < 0) {
        emit error("Failed to configure camera");
        return;
    }
    
    // Allocate buffers
    m_allocator = new FrameBufferAllocator(m_camera);
    
    for (StreamConfiguration &cfg : *m_config) {
        int ret = m_allocator->allocate(cfg.stream());
        if (ret < 0) {
            emit error("Failed to allocate buffers");
            return;
        }
    }
}

void CameraManager::startPreview()
{
    if (!m_camera) {
        emit error("No camera selected");
        return;
    }
    
    startCapture();
    
    if (m_hasToF) {
        m_tofUpdateTimer->start(100); // Update ToF at 10Hz
    }
}

void CameraManager::stopPreview()
{
    stopCapture();
    m_tofUpdateTimer->stop();
}

void CameraManager::startCapture()
{
    if (m_camera->start() < 0) {
        emit error("Failed to start camera");
        return;
    }
    
    // Set up request
    const StreamConfiguration &cfg = m_config->at(0);
    Stream *stream = cfg.stream();
    const std::vector<std::unique_ptr<FrameBuffer>> &buffers = m_allocator->buffers(stream);
    
    // Create and queue request
    m_request = m_camera->createRequest();
    m_request->addBuffer(stream, buffers[0].get());
    
    // Apply control settings
    applyControlSettings();
    
    m_camera->queueRequest(m_request.get());
}

void CameraManager::stopCapture()
{
    if (m_camera) {
        m_camera->stop();
        m_camera->release();
    }
}

void CameraManager::capturePhoto()
{
    qDebug() << "Capturing photo...";
    
    QString timestamp = QDateTime::currentDateTime().toString("yyyyMMdd_HHmmss");
    QString filename = m_outputPath + "/IMG_" + timestamp + ".jpg";
    
    // TODO: Capture high-res photo and save
    // For now, emit signal with path
    emit photoReady(filename);
}

void CameraManager::setFocusMode(int mode)
{
    m_focusMode = mode;
    emit focusModeChanged();
    applyControlSettings();
}

void CameraManager::setWhiteBalance(int mode)
{
    m_whiteBalance = mode;
    emit whiteBalanceChanged();
    applyControlSettings();
}

void CameraManager::setExposure(int value)
{
    m_exposure = value;
    emit exposureChanged();
    applyControlSettings();
}

void CameraManager::applyControlSettings()
{
    if (!m_request) return;
    
    ControlList &controls = m_request->controls();
    
    // Set focus mode
    switch (m_focusMode) {
    case FocusAuto:
        controls.set(controls::AfMode, controls::AfModeAuto);
        break;
    case FocusManual:
        controls.set(controls::AfMode, controls::AfModeManual);
        break;
    case FocusContinuous:
        controls.set(controls::AfMode, controls::AfModeContinuous);
        break;
    case FocusMacro:
        controls.set(controls::AfMode, controls::AfModeAuto);
        controls.set(controls::AfRange, controls::AfRangeMacro);
        break;
    }
    
    // Set white balance
    switch (m_whiteBalance) {
    case WBAuto:
        controls.set(controls::AwbMode, controls::AwbAuto);
        break;
    case WBDaylight:
        controls.set(controls::AwbMode, controls::AwbDaylight);
        break;
    case WBCloudy:
        controls.set(controls::AwbMode, controls::AwbCloudy);
        break;
    case WBTungsten:
        controls.set(controls::AwbMode, controls::AwbTungsten);
        break;
    case WBFluorescent:
        controls.set(controls::AwbMode, controls::AwbFluorescent);
        break;
    }
    
    // Set exposure compensation
    controls.set(controls::ExposureValue, static_cast<float>(m_exposure) / 10.0f);
    
    // HDR mode
    if (m_hdrEnabled) {
        controls.set(controls::HdrMode, controls::HdrModeMultiExposure);
    }
}

void CameraManager::updateToFDistance()
{
    // Query ToF sensor for distance
    // This would integrate with actual ToF hardware
    // For now, simulated
    m_tofDistance = 1.5f; // meters
    emit tofDistanceChanged();
}

void CameraManager::switchCamera()
{
    // Toggle between front and back cameras
    int currentIdx = -1;
    for (int i = 0; i < m_cameras.size(); i++) {
        if (m_cameras[i].toMap()["id"].toString() == m_activeCamera) {
            currentIdx = i;
            break;
        }
    }
    
    if (currentIdx >= 0 && m_cameras.size() > 1) {
        int nextIdx = (currentIdx + 1) % m_cameras.size();
        selectCamera(m_cameras[nextIdx].toMap()["id"].toString());
    }
}

void CameraManager::enableHDR(bool enable)
{
    m_hdrEnabled = enable;
    applyControlSettings();
}

void CameraManager::enableNightMode(bool enable)
{
    m_nightModeEnabled = enable;
    // Adjust exposure and ISO for night mode
    if (enable) {
        setExposure(30); // Increase exposure
    } else {
        setExposure(0);
    }
}

void CameraManager::setZoom(float level)
{
    m_zoomLevel = qBound(1.0f, level, 10.0f);
    // Apply digital zoom to crop region
    if (m_request) {
        // TODO: Set crop rectangle based on zoom level
    }
}

QVariantMap CameraManager::getCameraState() const
{
    QVariantMap state;
    
    // Camera identification
    state["cameraId"] = m_activeCamera;
    
    // Find camera name and driver from cameras list
    for (const auto &cam : m_cameras) {
        QVariantMap camMap = cam.toMap();
        if (camMap["id"].toString() == m_activeCamera) {
            state["cameraName"] = camMap["name"];
            state["cameraDriver"] = camMap.value("driver", "unknown");
            break;
        }
    }
    
    // Camera settings
    state["focusMode"] = m_focusMode;
    state["whiteBalance"] = m_whiteBalance;
    state["exposure"] = m_exposure;
    state["zoom"] = m_zoomLevel;
    state["hdrEnabled"] = m_hdrEnabled;
    state["nightModeEnabled"] = m_nightModeEnabled;
    state["tofDistance"] = m_tofDistance;
    
    qDebug() << "[CameraManager] Camera state:" << state;
    
    return state;
}

void CameraManager::startRecording()
{
    m_isRecording = true;
    emit isRecordingChanged();
    // TODO: Start video recording
}

void CameraManager::stopRecording()
{
    m_isRecording = false;
    emit isRecordingChanged();
    // TODO: Stop video recording
}
