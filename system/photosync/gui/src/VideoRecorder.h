// VideoRecorder.h - Video recording with libcamera
#ifndef VIDEORECORDER_H
#define VIDEORECORDER_H

#include <QObject>
#include <QString>
#include <QTimer>
#include <QElapsedTimer>
#include <libcamera/libcamera.h>
#include <memory>

extern "C" {
#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/imgutils.h>
#include <libswscale/swscale.h>
}

using namespace libcamera;

class VideoRecorder : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool recording READ recording NOTIFY recordingChanged)
    Q_PROPERTY(int duration READ duration NOTIFY durationChanged)
    Q_PROPERTY(qint64 fileSize READ fileSize NOTIFY fileSizeChanged)
    Q_PROPERTY(int frameRate READ frameRate WRITE setFrameRate NOTIFY frameRateChanged)
    Q_PROPERTY(int bitrate READ bitrate WRITE setBitrate NOTIFY bitrateChanged)
    
public:
    explicit VideoRecorder(QObject *parent = nullptr);
    ~VideoRecorder();
    
    bool recording() const { return m_recording; }
    int duration() const { return m_duration; }
    qint64 fileSize() const { return m_fileSize; }
    int frameRate() const { return m_frameRate; }
    int bitrate() const { return m_bitrate; }
    
    enum VideoCodec {
        H264,
        H265_HEVC,
        VP9,
        AV1
    };
    Q_ENUM(VideoCodec)
    
    enum VideoQuality {
        Low,      // 480p, 1 Mbps
        Medium,   // 720p, 2 Mbps
        High,     // 1080p, 5 Mbps
        VeryHigh, // 1080p, 10 Mbps
        Ultra     // 4K, 20 Mbps
    };
    Q_ENUM(VideoQuality)
    
public slots:
    void startRecording(const QString &outputPath, VideoCodec codec = H264, VideoQuality quality = High);
    void stopRecording();
    void pauseRecording();
    void resumeRecording();
    void setFrameRate(int fps);
    void setBitrate(int kbps);
    
    // Called by CameraManager when frame is ready
    void processFrame(const FrameBuffer *buffer, const StreamConfiguration &config);
    
signals:
    void recordingChanged();
    void durationChanged();
    void fileSizeChanged();
    void frameRateChanged();
    void bitrateChanged();
    void recordingStarted(QString path);
    void recordingStopped(QString path, int duration, qint64 fileSize);
    void recordingPaused();
    void recordingResumed();
    void error(QString message);
    
private:
    void initFFmpeg(VideoCodec codec, VideoQuality quality);
    void cleanupFFmpeg();
    void writeFrame(AVFrame *frame);
    AVFrame* convertFrame(const void *data, const Size &size, PixelFormat format);
    void updateMetadata();
    
    bool m_recording;
    bool m_paused;
    int m_duration;  // seconds
    qint64 m_fileSize;
    int m_frameRate;
    int m_bitrate;  // kbps
    
    QString m_outputPath;
    QElapsedTimer m_recordingTimer;
    QTimer *m_updateTimer;
    
    // FFmpeg components
    AVFormatContext *m_formatContext;
    AVCodecContext *m_codecContext;
    AVStream *m_videoStream;
    SwsContext *m_swsContext;
    AVFrame *m_rgbFrame;
    AVFrame *m_yuvFrame;
    AVPacket *m_packet;
    
    int m_frameCount;
    int64_t m_nextPts;
    
    // Configuration
    int m_width;
    int m_height;
};

#endif // VIDEORECORDER_H
