// CameraManager.h - Aggressive camera detection and management
// Searches all V4L2 devices, libcamera, and PipeWire sources

#ifndef CAMERAMANAGER_H
#define CAMERAMANAGER_H

#include <QObject>
#include <QVariantList>
#include <QVariantMap>
#include <QTimer>
#include <QThread>
#include <libcamera/libcamera.h>
#include <memory>
#include <vector>

using namespace libcamera;

class CameraScanner : public QObject
{
    Q_OBJECT
    
public:
    explicit CameraScanner(QObject *parent = nullptr);
    
public slots:
    void scanCameras();
    
signals:
    void camerasFound(QVariantList cameras);
    void scanProgress(int current, int total);
    
private:
    void scanV4L2Devices();
    void scanLibcamera();
    void scanPipeWire();
    QVariantMap getCameraCapabilities(const QString &device);
};

class CameraManager : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QVariantList cameras READ cameras NOTIFY camerasChanged)
    Q_PROPERTY(bool scanning READ scanning NOTIFY scanningChanged)
    Q_PROPERTY(QString activeCamera READ activeCamera NOTIFY activeCameraChanged)
    Q_PROPERTY(QVariantMap cameraProperties READ cameraProperties NOTIFY cameraPropertiesChanged)
    Q_PROPERTY(bool isRecording READ isRecording NOTIFY isRecordingChanged)
    Q_PROPERTY(int focusMode READ focusMode WRITE setFocusMode NOTIFY focusModeChanged)
    Q_PROPERTY(int whiteBalance READ whiteBalance WRITE setWhiteBalance NOTIFY whiteBalanceChanged)
    Q_PROPERTY(int exposure READ exposure WRITE setExposure NOTIFY exposureChanged)
    Q_PROPERTY(bool hasToF READ hasToF NOTIFY hasToFChanged)
    Q_PROPERTY(float tofDistance READ tofDistance NOTIFY tofDistanceChanged)
    
public:
    explicit CameraManager(QObject *parent = nullptr);
    ~CameraManager();
    
    // Properties
    QVariantList cameras() const { return m_cameras; }
    bool scanning() const { return m_scanning; }
    QString activeCamera() const { return m_activeCamera; }
    QVariantMap cameraProperties() const { return m_cameraProperties; }
    bool isRecording() const { return m_isRecording; }
    int focusMode() const { return m_focusMode; }
    int whiteBalance() const { return m_whiteBalance; }
    int exposure() const { return m_exposure; }
    bool hasToF() const { return m_hasToF; }
    float tofDistance() const { return m_tofDistance; }
    
    // Focus modes
    enum FocusMode {
        FocusAuto = 0,
        FocusManual = 1,
        FocusContinuous = 2,
        FocusMacro = 3
    };
    Q_ENUM(FocusMode)
    
    // White balance modes
    enum WhiteBalanceMode {
        WBAuto = 0,
        WBDaylight = 1,
        WBCloudy = 2,
        WBTungsten = 3,
        WBFluorescent = 4,
        WBManual = 5
    };
    Q_ENUM(WhiteBalanceMode)
    
public slots:
    void scanForCameras();
    void selectCamera(const QString &cameraId);
    void startPreview();
    void stopPreview();
    void capturePhoto();
    void startRecording();
    void stopRecording();
    void setFocusMode(int mode);
    void setWhiteBalance(int mode);
    void setExposure(int value);
    void setManualFocus(int distance);
    void enableHDR(bool enable);
    void enableNightMode(bool enable);
    void setZoom(float level);
    void switchCamera(); // Front/back toggle
    QVariantMap getCameraState() const; // Get current camera state for metadata
    
signals:
    void camerasChanged();
    void scanningChanged();
    void activeCameraChanged();
    void cameraPropertiesChanged();
    void isRecordingChanged();
    void focusModeChanged();
    void whiteBalanceChanged();
    void exposureChanged();
    void hasToFChanged();
    void tofDistanceChanged();
    void photoReady(QString path);
    void error(QString message);
    void previewFrame(QImage frame);
    
private slots:
    void onCamerasFound(QVariantList cameras);
    void onFrameReady();
    void updateToFDistance();
    
private:
    void initLibcamera();
    void configureCamera();
    void startCapture();
    void stopCapture();
    void processFrame(const FrameBuffer *buffer);
    void applyControlSettings();
    
    std::shared_ptr<CameraManager> m_cm;
    std::shared_ptr<Camera> m_camera;
    std::unique_ptr<CameraConfiguration> m_config;
    std::unique_ptr<Request> m_request;
    FrameBufferAllocator *m_allocator;
    
    QVariantList m_cameras;
    bool m_scanning;
    QString m_activeCamera;
    QVariantMap m_cameraProperties;
    bool m_isRecording;
    int m_focusMode;
    int m_whiteBalance;
    int m_exposure;
    bool m_hasToF;
    float m_tofDistance;
    
    QThread *m_scannerThread;
    CameraScanner *m_scanner;
    QTimer *m_tofUpdateTimer;
    
    QString m_outputPath;
    bool m_hdrEnabled;
    bool m_nightModeEnabled;
    float m_zoomLevel;
};

#endif // CAMERAMANAGER_H
