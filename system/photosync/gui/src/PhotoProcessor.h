// PhotoProcessor.h - Processes captured photos through PhotoSync system
#ifndef PHOTOPROCESSOR_H
#define PHOTOPROCESSOR_H

#include <QObject>
#include <QString>
#include <QProcess>
#include <QThread>
#include <QVariantMap>
#include <QGeoCoordinate>
#include <QDateTime>

struct PhotoMetadata {
    QString filePath;
    QDateTime captureTime;
    QGeoCoordinate location;
    QString cameraId;
    QString cameraName;
    QString cameraDriver;
    
    // Camera settings at capture
    int focusMode;
    int whiteBalance;
    int exposure;
    float zoom;
    bool hdrEnabled;
    bool nightModeEnabled;
    float tofDistance;
    
    // Image properties
    int width;
    int height;
    QString format;
    qint64 fileSize;
    
    // Computed
    QString locationName; // Reverse geocoded
    QString deviceId;
    
    QVariantMap toVariantMap() const;
    QString toJson() const;
    void writeExif(const QString &imagePath) const;
};

class PhotoProcessorWorker : public QObject
{
    Q_OBJECT
    
public:
    explicit PhotoProcessorWorker(QObject *parent = nullptr);
    
public slots:
    void processPhoto(const QString &path, const PhotoMetadata &metadata);
    
signals:
    void processingStarted(QString path);
    void processingComplete(QString path, qint64 originalSize, qint64 compressedSize);
    void processingError(QString path, QString error);
    void progressUpdate(int percentage);
    
private:
    void generateThumbnail(const QString &path);
    void compressAndDeduplicate(const QString &path);
    void writeMetadata(const QString &path, const PhotoMetadata &metadata);
    void reverseGeocode(PhotoMetadata &metadata);
};

class PhotoProcessor : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool processing READ processing NOTIFY processingChanged)
    Q_PROPERTY(int progress READ progress NOTIFY progressChanged)
    Q_PROPERTY(QString currentPhoto READ currentPhoto NOTIFY currentPhotoChanged)
    Q_PROPERTY(bool locationEnabled READ locationEnabled WRITE setLocationEnabled NOTIFY locationEnabledChanged)
    Q_PROPERTY(QGeoCoordinate currentLocation READ currentLocation NOTIFY currentLocationChanged)
    
public:
    explicit PhotoProcessor(QObject *parent = nullptr);
    ~PhotoProcessor();
    
    bool processing() const { return m_processing; }
    int progress() const { return m_progress; }
    QString currentPhoto() const { return m_currentPhoto; }
    bool locationEnabled() const { return m_locationEnabled; }
    QGeoCoordinate currentLocation() const { return m_currentLocation; }
    
public slots:
    void processPhoto(const QString &path, const QVariantMap &cameraState);
    void setLocationEnabled(bool enabled);
    void updateLocation(const QGeoCoordinate &location);
    
signals:
    void processingChanged();
    void progressChanged();
    void currentPhotoChanged();
    void locationEnabledChanged();
    void currentLocationChanged();
    void photoProcessed(QString path, qint64 originalSize, qint64 compressedSize);
    void error(QString message);
    
private slots:
    void onProcessingStarted(QString path);
    void onProcessingComplete(QString path, qint64 originalSize, qint64 compressedSize);
    void onProcessingError(QString path, QString error);
    void onProgressUpdate(int percentage);
    
private:
    PhotoMetadata createMetadata(const QString &path, const QVariantMap &cameraState);
    
    bool m_processing;
    int m_progress;
    QString m_currentPhoto;
    bool m_locationEnabled;
    QGeoCoordinate m_currentLocation;
    
    QThread *m_workerThread;
    PhotoProcessorWorker *m_worker;
    QObject *m_positionSource; // QGeoPositionInfoSource
};

#endif // PHOTOPROCESSOR_H
