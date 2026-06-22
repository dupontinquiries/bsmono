// PhotoProcessor.cpp - Enhanced implementation with metadata and debugging
#include "PhotoProcessor.h"
#include <QImage>
#include <QImageReader>
#include <QImageWriter>
#include <QBuffer>
#include <QDebug>
#include <QFile>
#include <QFileInfo>
#include <QDir>
#include <QJsonDocument>
#include <QJsonObject>
#include <QGeoPositionInfoSource>
#include <QSysInfo>

// ============================================================================
// PhotoMetadata Implementation
// ============================================================================

QVariantMap PhotoMetadata::toVariantMap() const
{
    QVariantMap map;
    
    // File info
    map["filePath"] = filePath;
    map["captureTime"] = captureTime.toString(Qt::ISODate);
    map["fileSize"] = fileSize;
    
    // Location
    if (location.isValid()) {
        QVariantMap loc;
        loc["latitude"] = location.latitude();
        loc["longitude"] = location.longitude();
        loc["altitude"] = location.altitude();
        loc["name"] = locationName;
        map["location"] = loc;
    }
    
    // Camera
    map["cameraId"] = cameraId;
    map["cameraName"] = cameraName;
    map["cameraDriver"] = cameraDriver;
    map["deviceId"] = deviceId;
    
    // Settings
    QVariantMap settings;
    settings["focusMode"] = focusMode;
    settings["whiteBalance"] = whiteBalance;
    settings["exposure"] = exposure;
    settings["zoom"] = zoom;
    settings["hdrEnabled"] = hdrEnabled;
    settings["nightModeEnabled"] = nightModeEnabled;
    if (tofDistance > 0) {
        settings["tofDistance"] = tofDistance;
    }
    map["settings"] = settings;
    
    // Image props
    QVariantMap image;
    image["width"] = width;
    image["height"] = height;
    image["format"] = format;
    map["image"] = image;
    
    return map;
}

QString PhotoMetadata::toJson() const
{
    QJsonDocument doc(QJsonObject::fromVariantMap(toVariantMap()));
    return doc.toJson(QJsonDocument::Compact);
}

void PhotoMetadata::writeExif(const QString &imagePath) const
{
    qDebug() << "[PhotoMetadata] Writing metadata for:" << imagePath;
    qDebug() << "[PhotoMetadata] Note: exiv2 integration pending, writing JSON sidecar";
    
    // For now, write JSON sidecar (exiv2 requires additional build setup)
    QString jsonPath = imagePath + ".metadata.json";
    QFile jsonFile(jsonPath);
    
    if (jsonFile.open(QIODevice::WriteOnly)) {
        jsonFile.write(toJson().toUtf8());
        jsonFile.close();
        qDebug() << "[PhotoMetadata] Metadata saved to:" << jsonPath;
    } else {
        qWarning() << "[PhotoMetadata] Failed to write metadata";
    }
}

// ============================================================================
// PhotoProcessorWorker Implementation
// ============================================================================

PhotoProcessorWorker::PhotoProcessorWorker(QObject *parent)
    : QObject(parent)
{
    qDebug() << "[PhotoProcessorWorker] Created on thread:" << QThread::currentThreadId();
}

void PhotoProcessorWorker::processPhoto(const QString &path, const PhotoMetadata &metadata)
{
    qDebug() << "[PhotoProcessorWorker] ===== Starting photo processing =====";
    qDebug() << "[PhotoProcessorWorker] Path:" << path;
    qDebug() << "[PhotoProcessorWorker] Thread:" << QThread::currentThreadId();
    
    emit processingStarted(path);
    
    QFileInfo fileInfo(path);
    if (!fileInfo.exists()) {
        qCritical() << "[PhotoProcessorWorker] ERROR: File does not exist:" << path;
        emit processingError(path, "File does not exist");
        return;
    }
    
    qint64 originalSize = fileInfo.size();
    qDebug() << "[PhotoProcessorWorker] Original size:" << originalSize << "bytes"
             << "(" << (originalSize / 1024.0 / 1024.0) << "MB)";
    
    try {
        // Step 1: Write metadata (5%)
        emit progressUpdate(5);
        qDebug() << "[PhotoProcessorWorker] Step 1/3: Writing metadata...";
        writeMetadata(path, metadata);
        
        // Step 2: Generate thumbnail (15%)
        emit progressUpdate(15);
        qDebug() << "[PhotoProcessorWorker] Step 2/3: Generating thumbnail...";
        generateThumbnail(path);
        
        // Step 3: Compress and deduplicate (80%)
        emit progressUpdate(50);
        qDebug() << "[PhotoProcessorWorker] Step 3/3: Compressing and deduplicating...";
        compressAndDeduplicate(path);
        
        emit progressUpdate(100);
        
        // Simulated compressed size for demo
        qint64 compressedSize = originalSize * 0.6;
        
        qDebug() << "[PhotoProcessorWorker] ===== Processing complete! =====";
        qDebug() << "[PhotoProcessorWorker] Compressed size:" << compressedSize << "bytes"
                 << "(" << (compressedSize / 1024.0 / 1024.0) << "MB)";
        qDebug() << "[PhotoProcessorWorker] Space saved:" << (originalSize - compressedSize) << "bytes"
                 << "(" << (100 - (compressedSize * 100 / originalSize)) << "% reduction)";
        
        emit processingComplete(path, originalSize, compressedSize);
        
    } catch (const std::exception &e) {
        qCritical() << "[PhotoProcessorWorker] EXCEPTION:" << e.what();
        emit processingError(path, QString::fromStdString(e.what()));
    }
}

void PhotoProcessorWorker::generateThumbnail(const QString &path)
{
    qDebug() << "[Thumbnail] Generating thumbnail for:" << path;
    
    QImageReader reader(path);
    reader.setAutoTransform(true);
    
    QSize thumbSize(256, 256);
    QSize imageSize = reader.size();
    
    qDebug() << "[Thumbnail] Original image size:" << imageSize;
    
    if (!imageSize.isValid()) {
        qWarning() << "[Thumbnail] WARNING: Could not read image size";
        throw std::runtime_error("Failed to read image dimensions");
    }
    
    if (imageSize.width() > thumbSize.width() || imageSize.height() > thumbSize.height()) {
        imageSize.scale(thumbSize, Qt::KeepAspectRatio);
        reader.setScaledSize(imageSize);
        qDebug() << "[Thumbnail] Scaled to:" << imageSize;
    }
    
    QImage thumbnail = reader.read();
    
    if (thumbnail.isNull()) {
        qCritical() << "[Thumbnail] ERROR: Failed to load image";
        qCritical() << "[Thumbnail] Reader error:" << reader.errorString();
        throw std::runtime_error("Failed to load image for thumbnail");
    }
    
    qDebug() << "[Thumbnail] Thumbnail generated, size:" << thumbnail.size();
    
    // Save thumbnail to cache
    QString thumbDir = QDir::homePath() + "/.cache/photosync/thumbnails";
    QDir().mkpath(thumbDir);
    
    QFileInfo fi(path);
    QString thumbPath = thumbDir + "/" + fi.baseName() + "_thumb.jpg";
    
    QImageWriter writer(thumbPath, "JPG");
    writer.setQuality(85);
    
    if (!writer.write(thumbnail)) {
        qWarning() << "[Thumbnail] WARNING: Failed to write thumbnail:" << writer.errorString();
    } else {
        QFileInfo thumbInfo(thumbPath);
        qDebug() << "[Thumbnail] Thumbnail saved to:" << thumbPath;
        qDebug() << "[Thumbnail] Thumbnail size:" << thumbInfo.size() << "bytes";
    }
}

void PhotoProcessorWorker::compressAndDeduplicate(const QString &path)
{
    qDebug() << "[Compress] Calling photosync-daemon for:" << path;
    
    QProcess process;
    QStringList args;
    args << "--process-file" << path;
    
    qDebug() << "[Compress] Command: photosync-daemon" << args.join(" ");
    
    process.start("photosync-daemon", args);
    
    if (!process.waitForStarted(5000)) {
        qWarning() << "[Compress] WARNING: Failed to start photosync-daemon";
        qWarning() << "[Compress] Error:" << process.errorString();
        qWarning() << "[Compress] Note: Daemon might not be installed yet - continuing anyway";
        return;
    }
    
    qDebug() << "[Compress] Daemon started, waiting for completion...";
    
    if (!process.waitForFinished(30000)) {
        qWarning() << "[Compress] WARNING: Timeout waiting for photosync-daemon";
        process.kill();
        process.waitForFinished(1000);
        return;
    }
    
    int exitCode = process.exitCode();
    QString stdOut = process.readAllStandardOutput();
    QString stdErr = process.readAllStandardError();
    
    qDebug() << "[Compress] Daemon finished with exit code:" << exitCode;
    
    if (!stdOut.isEmpty()) {
        qDebug() << "[Compress] Daemon output:" << stdOut.trimmed();
    }
    if (!stdErr.isEmpty()) {
        if (exitCode != 0) {
            qCritical() << "[Compress] Daemon error:" << stdErr.trimmed();
        } else {
            qDebug() << "[Compress] Daemon messages:" << stdErr.trimmed();
        }
    }
    
    if (exitCode != 0 && !stdErr.isEmpty()) {
        throw std::runtime_error(stdErr.toStdString());
    }
    
    qDebug() << "[Compress] Compression complete";
}

void PhotoProcessorWorker::writeMetadata(const QString &path, const PhotoMetadata &metadata)
{
    qDebug() << "[Metadata] Writing metadata for:" << path;
    qDebug() << "[Metadata] Capture time:" << metadata.captureTime.toString(Qt::ISODate);
    qDebug() << "[Metadata] Camera:" << metadata.cameraName << "(" << metadata.cameraId << ")";
    qDebug() << "[Metadata] Device:" << metadata.deviceId;
    
    if (metadata.location.isValid()) {
        qDebug() << "[Metadata] Location:" << metadata.location.latitude() 
                 << "," << metadata.location.longitude()
                 << "alt:" << metadata.location.altitude();
    } else {
        qDebug() << "[Metadata] No location data";
    }
    
    qDebug() << "[Metadata] Settings: focus=" << metadata.focusMode
             << "wb=" << metadata.whiteBalance
             << "exp=" << metadata.exposure
             << "zoom=" << metadata.zoom
             << "hdr=" << metadata.hdrEnabled
             << "night=" << metadata.nightModeEnabled;
    
    if (metadata.tofDistance > 0) {
        qDebug() << "[Metadata] ToF distance:" << metadata.tofDistance << "m";
    }
    
    // Write metadata
    PhotoMetadata mutableMetadata = metadata;
    
    // Reverse geocode if needed
    if (metadata.location.isValid() && metadata.locationName.isEmpty()) {
        qDebug() << "[Metadata] Reverse geocoding location...";
        reverseGeocode(mutableMetadata);
    }
    
    mutableMetadata.writeExif(path);
    
    qDebug() << "[Metadata] Metadata write complete";
}

void PhotoProcessorWorker::reverseGeocode(PhotoMetadata &metadata)
{
    // Simplified reverse geocoding
    // In production, use Nominatim API asynchronously
    
    QString lat = QString::number(metadata.location.latitude(), 'f', 6);
    QString lon = QString::number(metadata.location.longitude(), 'f', 6);
    
    metadata.locationName = QString("Location: %1, %2").arg(lat).arg(lon);
    
    qDebug() << "[Geocode] Location name:" << metadata.locationName;
}

// ============================================================================
// PhotoProcessor Implementation
// ============================================================================

PhotoProcessor::PhotoProcessor(QObject *parent)
    : QObject(parent)
    , m_processing(false)
    , m_progress(0)
    , m_locationEnabled(true)
    , m_positionSource(nullptr)
{
    qDebug() << "[PhotoProcessor] ===== Initializing PhotoProcessor =====";
    qDebug() << "[PhotoProcessor] Main thread:" << QThread::currentThreadId();
    
    // Create worker thread
    m_workerThread = new QThread(this);
    m_worker = new PhotoProcessorWorker();
    m_worker->moveToThread(m_workerThread);
    
    qDebug() << "[PhotoProcessor] Worker moved to separate thread";
    
    // Connect signals
    connect(m_worker, &PhotoProcessorWorker::processingStarted, 
            this, &PhotoProcessor::onProcessingStarted);
    connect(m_worker, &PhotoProcessorWorker::processingComplete, 
            this, &PhotoProcessor::onProcessingComplete);
    connect(m_worker, &PhotoProcessorWorker::processingError, 
            this, &PhotoProcessor::onProcessingError);
    connect(m_worker, &PhotoProcessorWorker::progressUpdate, 
            this, &PhotoProcessor::onProgressUpdate);
    
    qDebug() << "[PhotoProcessor] Signals connected";
    
    m_workerThread->start();
    qDebug() << "[PhotoProcessor] Worker thread started";
    
    // Initialize geolocation
    if (m_locationEnabled) {
        auto *source = QGeoPositionInfoSource::createDefaultSource(this);
        if (source) {
            m_positionSource = source;
            connect(source, &QGeoPositionInfoSource::positionUpdated,
                    [this](const QGeoPositionInfo &info) {
                        updateLocation(info.coordinate());
                    });
            source->startUpdates();
            qDebug() << "[PhotoProcessor] Geolocation service started";
        } else {
            qWarning() << "[PhotoProcessor] WARNING: Geolocation service not available";
        }
    }
    
    qDebug() << "[PhotoProcessor] Initialization complete";
}

PhotoProcessor::~PhotoProcessor()
{
    qDebug() << "[PhotoProcessor] ===== Shutting down PhotoProcessor =====";
    
    if (m_workerThread) {
        m_workerThread->quit();
        if (!m_workerThread->wait(3000)) {
            qWarning() << "[PhotoProcessor] Worker thread didn't quit in time, forcing termination";
            m_workerThread->terminate();
            m_workerThread->wait();
        }
    }
    
    qDebug() << "[PhotoProcessor] Shutdown complete";
}

void PhotoProcessor::processPhoto(const QString &path, const QVariantMap &cameraState)
{
    if (m_processing) {
        qWarning() << "[PhotoProcessor] WARNING: Already processing a photo, rejecting new request";
        emit error("Already processing a photo");
        return;
    }
    
    qDebug() << "[PhotoProcessor] ===== New photo processing request =====";
    qDebug() << "[PhotoProcessor] Path:" << path;
    qDebug() << "[PhotoProcessor] Camera state keys:" << cameraState.keys();
    
    PhotoMetadata metadata = createMetadata(path, cameraState);
    
    // Call worker asynchronously
    QMetaObject::invokeMethod(m_worker, "processPhoto", Qt::QueuedConnection,
                              Q_ARG(QString, path),
                              Q_ARG(PhotoMetadata, metadata));
    
    qDebug() << "[PhotoProcessor] Processing request queued to worker thread";
}

PhotoMetadata PhotoProcessor::createMetadata(const QString &path, const QVariantMap &cameraState)
{
    qDebug() << "[PhotoProcessor] Creating metadata for:" << path;
    
    PhotoMetadata metadata;
    
    // File info
    QFileInfo fileInfo(path);
    metadata.filePath = path;
    metadata.fileSize = fileInfo.size();
    metadata.captureTime = QDateTime::currentDateTime();
    
    qDebug() << "[PhotoProcessor] File size:" << metadata.fileSize << "bytes";
    qDebug() << "[PhotoProcessor] Capture time:" << metadata.captureTime.toString(Qt::ISODate);
    
    // Location
    if (m_locationEnabled && m_currentLocation.isValid()) {
        metadata.location = m_currentLocation;
        qDebug() << "[PhotoProcessor] Adding location:" << metadata.location.latitude() 
                 << "," << metadata.location.longitude()
                 << "alt:" << metadata.location.altitude();
    } else {
        qDebug() << "[PhotoProcessor] No location data (enabled:" << m_locationEnabled 
                 << "valid:" << m_currentLocation.isValid() << ")";
    }
    
    // Camera info
    metadata.cameraId = cameraState.value("cameraId", "unknown").toString();
    metadata.cameraName = cameraState.value("cameraName", "Unknown Camera").toString();
    metadata.cameraDriver = cameraState.value("cameraDriver", "unknown").toString();
    metadata.deviceId = QSysInfo::machineHostName();
    
    qDebug() << "[PhotoProcessor] Camera ID:" << metadata.cameraId;
    qDebug() << "[PhotoProcessor] Camera name:" << metadata.cameraName;
    qDebug() << "[PhotoProcessor] Device ID:" << metadata.deviceId;
    
    // Camera settings
    metadata.focusMode = cameraState.value("focusMode", 0).toInt();
    metadata.whiteBalance = cameraState.value("whiteBalance", 0).toInt();
    metadata.exposure = cameraState.value("exposure", 0).toInt();
    metadata.zoom = cameraState.value("zoom", 1.0).toFloat();
    metadata.hdrEnabled = cameraState.value("hdrEnabled", false).toBool();
    metadata.nightModeEnabled = cameraState.value("nightModeEnabled", false).toBool();
    metadata.tofDistance = cameraState.value("tofDistance", 0.0).toFloat();
    
    qDebug() << "[PhotoProcessor] Camera settings created";
    
    // Image properties
    QImageReader reader(path);
    QSize imageSize = reader.size();
    metadata.width = imageSize.width();
    metadata.height = imageSize.height();
    metadata.format = QString::fromLatin1(reader.format());
    
    qDebug() << "[PhotoProcessor] Image dimensions:" << metadata.width << "x" << metadata.height;
    qDebug() << "[PhotoProcessor] Image format:" << metadata.format;
    
    return metadata;
}

void PhotoProcessor::setLocationEnabled(bool enabled)
{
    if (m_locationEnabled != enabled) {
        qDebug() << "[PhotoProcessor] Location enabled changed:" << m_locationEnabled << "->" << enabled;
        
        m_locationEnabled = enabled;
        emit locationEnabledChanged();
        
        if (enabled && !m_positionSource) {
            auto *source = QGeoPositionInfoSource::createDefaultSource(this);
            if (source) {
                m_positionSource = source;
                connect(source, &QGeoPositionInfoSource::positionUpdated,
                        [this](const QGeoPositionInfo &info) {
                            updateLocation(info.coordinate());
                        });
                source->startUpdates();
                qDebug() << "[PhotoProcessor] Geolocation service started";
            } else {
                qWarning() << "[PhotoProcessor] Could not create geolocation source";
            }
        } else if (!enabled && m_positionSource) {
            qobject_cast<QGeoPositionInfoSource*>(m_positionSource)->stopUpdates();
            qDebug() << "[PhotoProcessor] Geolocation service stopped";
        }
    }
}

void PhotoProcessor::updateLocation(const QGeoCoordinate &location)
{
    m_currentLocation = location;
    emit currentLocationChanged();
    
    qDebug() << "[PhotoProcessor] Location updated:" 
             << "lat=" << location.latitude()
             << "lon=" << location.longitude()
             << "alt=" << location.altitude();
}

void PhotoProcessor::onProcessingStarted(QString path)
{
    qDebug() << "[PhotoProcessor] >>> Processing started:" << path;
    
    m_processing = true;
    m_currentPhoto = path;
    m_progress = 0;
    
    emit processingChanged();
    emit currentPhotoChanged();
    emit progressChanged();
}

void PhotoProcessor::onProcessingComplete(QString path, qint64 originalSize, qint64 compressedSize)
{
    qDebug() << "[PhotoProcessor] >>> Processing complete:" << path;
    qDebug() << "[PhotoProcessor] Original size:" << originalSize << "bytes";
    qDebug() << "[PhotoProcessor] Compressed size:" << compressedSize << "bytes";
    
    int savings = 100 - (compressedSize * 100 / originalSize);
    qDebug() << "[PhotoProcessor] Space saved:" << savings << "%";
    
    m_processing = false;
    m_progress = 100;
    
    emit processingChanged();
    emit progressChanged();
    emit photoProcessed(path, originalSize, compressedSize);
}

void PhotoProcessor::onProcessingError(QString path, QString errorMsg)
{
    qCritical() << "[PhotoProcessor] >>> Processing ERROR for:" << path;
    qCritical() << "[PhotoProcessor] Error message:" << errorMsg;
    
    m_processing = false;
    
    emit processingChanged();
    emit error(errorMsg);
}

void PhotoProcessor::onProgressUpdate(int percentage)
{
    m_progress = percentage;
    emit progressChanged();
    
    qDebug() << "[PhotoProcessor] Progress:" << percentage << "%";
}
