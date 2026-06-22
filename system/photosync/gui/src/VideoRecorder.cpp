// VideoRecorder.cpp - Implementation with FFmpeg
#include "VideoRecorder.h"
#include <QDebug>
#include <QFile>
#include <QFileInfo>
#include <QDateTime>

VideoRecorder::VideoRecorder(QObject *parent)
    : QObject(parent)
    , m_recording(false)
    , m_paused(false)
    , m_duration(0)
    , m_fileSize(0)
    , m_frameRate(30)
    , m_bitrate(5000)  // 5 Mbps default
    , m_formatContext(nullptr)
    , m_codecContext(nullptr)
    , m_videoStream(nullptr)
    , m_swsContext(nullptr)
    , m_rgbFrame(nullptr)
    , m_yuvFrame(nullptr)
    , m_packet(nullptr)
    , m_frameCount(0)
    , m_nextPts(0)
    , m_width(1920)
    , m_height(1080)
{
    qDebug() << "[VideoRecorder] Initialized";
    
    // Update timer for duration and file size
    m_updateTimer = new QTimer(this);
    connect(m_updateTimer, &QTimer::timeout, this, &VideoRecorder::updateMetadata);
}

VideoRecorder::~VideoRecorder()
{
    qDebug() << "[VideoRecorder] Destroying";
    if (m_recording) {
        stopRecording();
    }
    cleanupFFmpeg();
}

void VideoRecorder::startRecording(const QString &outputPath, VideoCodec codec, VideoQuality quality)
{
    if (m_recording) {
        qWarning() << "[VideoRecorder] Already recording";
        return;
    }
    
    qDebug() << "[VideoRecorder] ===== Starting video recording =====";
    qDebug() << "[VideoRecorder] Output path:" << outputPath;
    qDebug() << "[VideoRecorder] Codec:" << codec;
    qDebug() << "[VideoRecorder] Quality:" << quality;
    
    m_outputPath = outputPath;
    
    // Initialize FFmpeg encoder
    initFFmpeg(codec, quality);
    
    // Reset counters
    m_frameCount = 0;
    m_nextPts = 0;
    m_duration = 0;
    m_fileSize = 0;
    
    // Start recording
    m_recording = true;
    m_paused = false;
    m_recordingTimer.start();
    
    // Start update timer (1Hz)
    m_updateTimer->start(1000);
    
    emit recordingChanged();
    emit recordingStarted(outputPath);
    
    qDebug() << "[VideoRecorder] Recording started successfully";
}

void VideoRecorder::stopRecording()
{
    if (!m_recording) {
        qWarning() << "[VideoRecorder] Not recording";
        return;
    }
    
    qDebug() << "[VideoRecorder] ===== Stopping video recording =====";
    
    m_updateTimer->stop();
    
    // Flush encoder
    if (m_codecContext) {
        avcodec_send_frame(m_codecContext, nullptr);
        
        while (true) {
            int ret = avcodec_receive_packet(m_codecContext, m_packet);
            if (ret == AVERROR_EOF || ret == AVERROR(EAGAIN)) {
                break;
            }
            
            av_packet_rescale_ts(m_packet, m_codecContext->time_base, m_videoStream->time_base);
            m_packet->stream_index = m_videoStream->index;
            
            av_interleaved_write_frame(m_formatContext, m_packet);
            av_packet_unref(m_packet);
        }
    }
    
    // Write trailer
    if (m_formatContext) {
        av_write_trailer(m_formatContext);
    }
    
    // Get final file size
    QFileInfo fileInfo(m_outputPath);
    m_fileSize = fileInfo.size();
    
    int finalDuration = m_recordingTimer.elapsed() / 1000;
    
    qDebug() << "[VideoRecorder] Recording stopped";
    qDebug() << "[VideoRecorder] Duration:" << finalDuration << "seconds";
    qDebug() << "[VideoRecorder] Frames:" << m_frameCount;
    qDebug() << "[VideoRecorder] File size:" << m_fileSize << "bytes"
             << "(" << (m_fileSize / 1024.0 / 1024.0) << "MB)";
    
    m_recording = false;
    emit recordingChanged();
    emit recordingStopped(m_outputPath, finalDuration, m_fileSize);
    
    // Cleanup
    cleanupFFmpeg();
}

void VideoRecorder::pauseRecording()
{
    if (!m_recording || m_paused) return;
    
    qDebug() << "[VideoRecorder] Pausing recording";
    
    m_paused = true;
    emit recordingPaused();
}

void VideoRecorder::resumeRecording()
{
    if (!m_recording || !m_paused) return;
    
    qDebug() << "[VideoRecorder] Resuming recording";
    
    m_paused = false;
    emit recordingResumed();
}

void VideoRecorder::setFrameRate(int fps)
{
    if (m_recording) {
        qWarning() << "[VideoRecorder] Cannot change frame rate while recording";
        return;
    }
    
    m_frameRate = qBound(1, fps, 120);
    emit frameRateChanged();
    
    qDebug() << "[VideoRecorder] Frame rate set to:" << m_frameRate;
}

void VideoRecorder::setBitrate(int kbps)
{
    if (m_recording) {
        qWarning() << "[VideoRecorder] Cannot change bitrate while recording";
        return;
    }
    
    m_bitrate = qBound(500, kbps, 50000);
    emit bitrateChanged();
    
    qDebug() << "[VideoRecorder] Bitrate set to:" << m_bitrate << "kbps";
}

void VideoRecorder::initFFmpeg(VideoCodec codec, VideoQuality quality)
{
    qDebug() << "[VideoRecorder] Initializing FFmpeg encoder";
    
    // Set resolution and bitrate based on quality
    switch (quality) {
    case Low:
        m_width = 854; m_height = 480;
        m_bitrate = 1000;
        break;
    case Medium:
        m_width = 1280; m_height = 720;
        m_bitrate = 2000;
        break;
    case High:
        m_width = 1920; m_height = 1080;
        m_bitrate = 5000;
        break;
    case VeryHigh:
        m_width = 1920; m_height = 1080;
        m_bitrate = 10000;
        break;
    case Ultra:
        m_width = 3840; m_height = 2160;
        m_bitrate = 20000;
        break;
    }
    
    qDebug() << "[VideoRecorder] Resolution:" << m_width << "x" << m_height;
    qDebug() << "[VideoRecorder] Bitrate:" << m_bitrate << "kbps";
    
    // Allocate output format context
    avformat_alloc_output_context2(&m_formatContext, nullptr, nullptr, m_outputPath.toUtf8().constData());
    if (!m_formatContext) {
        emit error("Failed to create output context");
        return;
    }
    
    // Find encoder
    const AVCodec *avcodec = nullptr;
    QString codecName;
    
    switch (codec) {
    case H264:
        avcodec = avcodec_find_encoder_by_name("libx264");
        codecName = "H.264";
        break;
    case H265_HEVC:
        avcodec = avcodec_find_encoder_by_name("libx265");
        codecName = "H.265";
        break;
    case VP9:
        avcodec = avcodec_find_encoder_by_name("libvpx-vp9");
        codecName = "VP9";
        break;
    case AV1:
        avcodec = avcodec_find_encoder_by_name("libaom-av1");
        codecName = "AV1";
        break;
    }
    
    if (!avcodec) {
        qWarning() << "[VideoRecorder] Codec not found, falling back to H.264";
        avcodec = avcodec_find_encoder(AV_CODEC_ID_H264);
        codecName = "H.264 (fallback)";
    }
    
    qDebug() << "[VideoRecorder] Using codec:" << codecName;
    
    // Create video stream
    m_videoStream = avformat_new_stream(m_formatContext, avcodec);
    if (!m_videoStream) {
        emit error("Failed to create video stream");
        return;
    }
    
    // Allocate codec context
    m_codecContext = avcodec_alloc_context3(avcodec);
    if (!m_codecContext) {
        emit error("Failed to allocate codec context");
        return;
    }
    
    // Configure codec
    m_codecContext->codec_id = avcodec->id;
    m_codecContext->codec_type = AVMEDIA_TYPE_VIDEO;
    m_codecContext->width = m_width;
    m_codecContext->height = m_height;
    m_codecContext->time_base = AVRational{1, m_frameRate};
    m_codecContext->framerate = AVRational{m_frameRate, 1};
    m_codecContext->gop_size = m_frameRate;  // I-frame every second
    m_codecContext->max_b_frames = 2;
    m_codecContext->pix_fmt = AV_PIX_FMT_YUV420P;
    m_codecContext->bit_rate = m_bitrate * 1000;
    
    // H.264 specific options
    if (codec == H264) {
        av_opt_set(m_codecContext->priv_data, "preset", "medium", 0);
        av_opt_set(m_codecContext->priv_data, "tune", "zerolatency", 0);
    }
    
    // Open codec
    if (avcodec_open2(m_codecContext, avcodec, nullptr) < 0) {
        emit error("Failed to open codec");
        return;
    }
    
    // Copy codec parameters to stream
    avcodec_parameters_from_context(m_videoStream->codecpar, m_codecContext);
    m_videoStream->time_base = m_codecContext->time_base;
    
    // Open output file
    if (!(m_formatContext->oformat->flags & AVFMT_NOFILE)) {
        if (avio_open(&m_formatContext->pb, m_outputPath.toUtf8().constData(), AVIO_FLAG_WRITE) < 0) {
            emit error("Failed to open output file");
            return;
        }
    }
    
    // Write header
    if (avformat_write_header(m_formatContext, nullptr) < 0) {
        emit error("Failed to write header");
        return;
    }
    
    // Allocate frames
    m_yuvFrame = av_frame_alloc();
    m_yuvFrame->format = m_codecContext->pix_fmt;
    m_yuvFrame->width = m_width;
    m_yuvFrame->height = m_height;
    av_frame_get_buffer(m_yuvFrame, 0);
    
    // Allocate packet
    m_packet = av_packet_alloc();
    
    qDebug() << "[VideoRecorder] FFmpeg initialized successfully";
}

void VideoRecorder::cleanupFFmpeg()
{
    qDebug() << "[VideoRecorder] Cleaning up FFmpeg";
    
    if (m_packet) {
        av_packet_free(&m_packet);
        m_packet = nullptr;
    }
    
    if (m_yuvFrame) {
        av_frame_free(&m_yuvFrame);
        m_yuvFrame = nullptr;
    }
    
    if (m_rgbFrame) {
        av_frame_free(&m_rgbFrame);
        m_rgbFrame = nullptr;
    }
    
    if (m_swsContext) {
        sws_freeContext(m_swsContext);
        m_swsContext = nullptr;
    }
    
    if (m_codecContext) {
        avcodec_free_context(&m_codecContext);
        m_codecContext = nullptr;
    }
    
    if (m_formatContext) {
        if (!(m_formatContext->oformat->flags & AVFMT_NOFILE)) {
            avio_closep(&m_formatContext->pb);
        }
        avformat_free_context(m_formatContext);
        m_formatContext = nullptr;
    }
}

void VideoRecorder::processFrame(const FrameBuffer *buffer, const StreamConfiguration &config)
{
    if (!m_recording || m_paused) return;
    
    // Map frame buffer
    const FrameBuffer::Plane &plane = buffer->planes()[0];
    void *data = mmap(nullptr, plane.length, PROT_READ, MAP_SHARED, plane.fd.get(), 0);
    
    if (data == MAP_FAILED) {
        qWarning() << "[VideoRecorder] Failed to map frame buffer";
        return;
    }
    
    // Convert frame to YUV420P
    AVFrame *frame = convertFrame(data, config.size, config.pixelFormat);
    
    if (frame) {
        writeFrame(frame);
        m_frameCount++;
    }
    
    munmap(data, plane.length);
}

AVFrame* VideoRecorder::convertFrame(const void *data, const Size &size, PixelFormat format)
{
    // Initialize scaler if needed
    if (!m_swsContext) {
        AVPixelFormat srcFormat;
        
        // Map libcamera format to FFmpeg format
        if (format == formats::YUV420) {
            srcFormat = AV_PIX_FMT_YUV420P;
        } else if (format == formats::NV12) {
            srcFormat = AV_PIX_FMT_NV12;
        } else {
            srcFormat = AV_PIX_FMT_RGB24;  // Default
        }
        
        m_swsContext = sws_getContext(
            size.width, size.height, srcFormat,
            m_width, m_height, AV_PIX_FMT_YUV420P,
            SWS_BILINEAR, nullptr, nullptr, nullptr
        );
        
        if (!m_swsContext) {
            qWarning() << "[VideoRecorder] Failed to create scaler";
            return nullptr;
        }
    }
    
    // Prepare source frame
    uint8_t *srcData[4] = { (uint8_t*)data, nullptr, nullptr, nullptr };
    int srcLinesize[4] = { (int)size.width, 0, 0, 0 };
    
    // Scale to YUV frame
    sws_scale(m_swsContext, srcData, srcLinesize, 0, size.height,
              m_yuvFrame->data, m_yuvFrame->linesize);
    
    return m_yuvFrame;
}

void VideoRecorder::writeFrame(AVFrame *frame)
{
    // Set PTS
    frame->pts = m_nextPts++;
    
    // Send frame to encoder
    int ret = avcodec_send_frame(m_codecContext, frame);
    if (ret < 0) {
        qWarning() << "[VideoRecorder] Error sending frame to encoder";
        return;
    }
    
    // Receive encoded packets
    while (ret >= 0) {
        ret = avcodec_receive_packet(m_codecContext, m_packet);
        
        if (ret == AVERROR(EAGAIN) || ret == AVERROR_EOF) {
            break;
        } else if (ret < 0) {
            qWarning() << "[VideoRecorder] Error receiving packet from encoder";
            return;
        }
        
        // Rescale packet timestamps
        av_packet_rescale_ts(m_packet, m_codecContext->time_base, m_videoStream->time_base);
        m_packet->stream_index = m_videoStream->index;
        
        // Write packet
        ret = av_interleaved_write_frame(m_formatContext, m_packet);
        av_packet_unref(m_packet);
        
        if (ret < 0) {
            qWarning() << "[VideoRecorder] Error writing packet";
            return;
        }
    }
}

void VideoRecorder::updateMetadata()
{
    if (!m_recording) return;
    
    m_duration = m_recordingTimer.elapsed() / 1000;
    
    // Get current file size
    QFileInfo fileInfo(m_outputPath);
    m_fileSize = fileInfo.size();
    
    emit durationChanged();
    emit fileSizeChanged();
    
    // Log every 10 seconds
    if (m_duration % 10 == 0) {
        qDebug() << "[VideoRecorder] Recording:" << m_duration << "s,"
                 << m_frameCount << "frames,"
                 << (m_fileSize / 1024.0 / 1024.0) << "MB";
    }
}
