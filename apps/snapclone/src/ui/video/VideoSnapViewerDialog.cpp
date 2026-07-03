#include "ui/video/VideoSnapViewerDialog.hpp"
#include "ui/video/VideoEncoderBackend.hpp"  // V32.34+: vvcDecodeAvailable()
#include "ui/Theme.hpp"

#include <QMediaPlayer>
#include <QVideoWidget>
#include <QVideoSink>
#include <QVideoFrame>
#include <QAudioOutput>
#include <QSlider>
#include <QPushButton>
#include <QLabel>
#include <QHBoxLayout>
#include <QVBoxLayout>
#include <QUrl>
#include <QFileDialog>
#include <QStandardPaths>
#include <QFile>
#include <QFileInfo>
#include <QDir>
#include <QDateTime>
#include <QLoggingCategory>
#include <QMessageBox>
#include <QProcess>
#include <QTimer>
#include <QImage>
#include <QPixmap>
#include <QCoreApplication>
#include <QSettings>  // V32.34+ audio: playback speech-boost preference
#include <QMutex>     // V32.34+ audio: element-probe cache guard
#include <QHash>      // V32.34+ audio: element-probe memo
#include <cmath>      // V32.34+ audio: std::pow for dB→linear pre-attenuation
#ifdef SNAPCLONE_RUST_AUDIO
#  include "audio/snapclone_audio_ffi.h"  // V32.34+: VAD speech detection
#endif

namespace SnapClone::UI::Video {

Q_LOGGING_CATEGORY(lcVidView, "snapclone.video.viewer")

namespace {
#ifdef SNAPCLONE_RUST_AUDIO
// Forward decl — defined below buildSpeechBoostSegment, but called from it.
bool fileContainsSpeech(const QString &path, int headSeconds = 3);
#endif
QString formatTime(qint64 ms)
{
    if (ms < 0) ms = 0;
    const int sec = static_cast<int>(ms / 1000);
    const int m = sec / 60;
    const int s = sec % 60;
    return QStringLiteral("%1:%2").arg(m, 2, 10, QChar('0')).arg(s, 2, 10, QChar('0'));
}

/// Probe whether gst-launch-1.0 is available on PATH. Cached after first
/// call (process lifetime) so we don't pay the QStandardPaths cost on
/// every viewer open.
bool gstLaunchAvailable()
{
    static bool s_checked = false;
    static bool s_available = false;
    if (!s_checked) {
        s_checked = true;
        s_available = !QStandardPaths::findExecutable(
            QStringLiteral("gst-launch-1.0")).isEmpty();
    }
    return s_available;
}

/// Per-process flag: has Qt's QMediaPlayer failed at least once during
/// this session? If yes, skip it on subsequent snaps and go straight to
/// gst-launch — saves ~500 ms of "black screen then error then fallback"
/// per snap and produces a clean user experience.
///
/// We only flip this ONE WAY (false → true). Even if Qt media starts
/// working again mid-session for whatever reason, we stay on gst for
/// consistency. The user can restart the app to retry Qt.
bool &qtMediaBrokenFlag()
{
    static bool s_broken = false;
    return s_broken;
}

/// Probe whether the file has an audio track. Returns true (assume audio
/// present) on probe failure — better to attempt the audio branch and
/// fail loudly than silently drop audio when it IS there.
///
/// Without this check, building a pipeline with `d.audio_0 ! ...` for a
/// file with no audio track would leave that branch waiting forever in
/// PREROLL — same freeze pattern the transcoder hits without the check.
///
/// CALLED FROM THE UI THREAD on the snap-open hot path, so the timeout
/// is kept short (800 ms). For a local file gst-discoverer typically
/// completes in <100 ms; we give it some slack for slow storage but not
/// 4 seconds (which would be a visible UI freeze).
bool probeHasAudio(const QString &path)
{
    if (QStandardPaths::findExecutable(QStringLiteral("gst-discoverer-1.0")).isEmpty()) {
        // No discoverer binary; assume audio present.
        return true;
    }
    QProcess disc;
    disc.start(QStringLiteral("gst-discoverer-1.0"),
               { QStringLiteral("-v"), path });
    if (!disc.waitForFinished(800)) {
        // Disconnect before kill to avoid the destructor-during-running
        // warning Qt emits. Kill is async but the stack-local disc going
        // out of scope WOULD print the warning otherwise.
        disc.QObject::disconnect();
        disc.kill();
        disc.waitForFinished(200);
        return true;  // timed out — assume audio
    }
    const QByteArray out = disc.readAllStandardOutput();
    const bool seenAudio =
        out.contains("Audio:")       ||
        out.contains("audio-codec:") ||
        out.contains("audio/x-")     ||
        out.contains("Audio stream");
    const bool seenVideo =
        out.contains("Video:") || out.contains("video-codec:");
    // Positive video evidence + no audio evidence = no audio track.
    if (seenVideo && !seenAudio) return false;
    return true;
}

/// V32.34+ audio: probe-once whether a gst element is installed, memoized.
/// Used for the optional playback-side speech-boost EQ. Mirrors the
/// camera/recorder element-detection pattern.
bool playbackElementAvailable(const QString &element)
{
    static QMutex mu;
    static QHash<QString, bool> cache;
    QMutexLocker lock(&mu);
    const auto it = cache.constFind(element);
    if (it != cache.constEnd()) return it.value();
    bool present = false;
    const QString inspect = QStandardPaths::findExecutable(
        QStringLiteral("gst-inspect-1.0"));
    if (!inspect.isEmpty()) {
        QProcess p;
        p.start(inspect, { element });
        if (p.waitForFinished(2000)) {
            present = (p.exitStatus() == QProcess::NormalExit
                       && p.exitCode() == 0);
        } else {
            p.kill();
            p.waitForFinished(500);
        }
    }
    cache.insert(element, present);
    return present;
}

/// V32.34+ audio: build the optional playback-side speech-boost segment.
///
/// Per the audio design brief, speech boost is a PER-VIEWER accessibility
/// feature applied at PLAYBACK — the recording stays clean + normalized,
/// and each viewer chooses their own boost. This keeps a deaf-friendly
/// default from making everyone else's copy sound processed.
///
/// Reads QSettings audio/speechBoost: "off" (default) / "mild" / "strong".
/// When not "off" AND equalizer-3bands is installed, returns a segment
/// that lifts the mid band (≈300Hz–3.4kHz, where speech intelligibility
/// lives) while gently cutting the low band (rumble/handling noise).
/// Returned list starts and ends with "!" so it splices between
/// audioresample and autoaudiosink. Empty list when off/unavailable.
///
/// equalizer-3bands bands: band0≈100Hz (low), band1≈1.1kHz (mid),
/// band2≈11kHz (high). Gains in dB, range roughly ±12. We boost mid,
/// trim low, leave high alone.
QStringList buildSpeechBoostSegment(const QString &filePath)
{
    QSettings s;
    const QString mode =
        s.value(QStringLiteral("audio/speechBoost"), QStringLiteral("off"))
            .toString().toLower();
    if (mode == QLatin1String("off") || mode.isEmpty())
        return {};
    if (!playbackElementAvailable(QStringLiteral("equalizer-3bands"))) {
        qCInfo(lcVidView) << "Speech boost requested (" << mode
                          << ") but equalizer-3bands unavailable — skipping";
        return {};
    }
#ifdef SNAPCLONE_RUST_AUDIO
    // V32.34+ v2: VAD-gate the boost. Only colour the audio when the clip
    // actually contains speech — music / ambient clips are left flat so the
    // boost EQ doesn't muddy them. When the audio crate isn't compiled in,
    // this gate is skipped and the boost applies whenever enabled (the v1
    // static-EQ behavior). The "auto" pseudo-mode below opts into gating;
    // explicit mild/strong always apply (user asked for it unconditionally).
    const bool gateOnSpeech =
        s.value(QStringLiteral("audio/speechBoostAutoOnly"), true).toBool();
    if (gateOnSpeech && !filePath.isEmpty()) {
        if (!fileContainsSpeech(filePath)) {
            qCInfo(lcVidView) << "Speech boost: no speech detected in clip — "
                                 "leaving audio flat";
            return {};
        }
    }
#else
    Q_UNUSED(filePath);
#endif
    // V32.34+ anti-clipping speech boost chain.
    //
    // The old chain was a bare `equalizer-3bands` mid-band lift. Raising a
    // band's gain pushes peaks past 0 dBFS on already-loud speech → hard
    // clip → the harsh crackle the user heard at high volume. A naive
    // FFT-domain boost would clip the SAME way (and add overlap-add
    // windowing artifacts), so more bins isn't the fix; headroom + a
    // limiter is. The standard speech-enhancement chain:
    //
    //   volume (pre-attenuate)  →  equalizer-3bands (band lift)  →
    //   audiodynamic (limiter, soft-knee, ceiling just under 0 dBFS)
    //
    //   1. Pre-attenuate by ~the boost amount so the post-EQ peak lands
    //      around where it started instead of above the ceiling. This
    //      alone removes most clipping; the perceived speech level still
    //      rises because the mid band is lifted RELATIVE to the rest.
    //   2. The EQ does the actual speech-band emphasis (unchanged intent).
    //   3. A soft-knee limiter catches any residual transient peaks and
    //      rounds them off smoothly instead of letting them square-wave
    //      clip. With a soft knee the onset is gradual, so it's inaudible
    //      on normal speech and only engages on the loudest moments.
    //
    // The limiter is the part that actually guarantees no hard clipping;
    // the pre-attenuation just keeps the limiter from working so hard that
    // it audibly pumps. Both gated on element availability so a minimal
    // pmOS image without audiodynamic still gets the (pre-attenuated) EQ.
    const double midGain = (mode == QLatin1String("strong")) ? 8.0 : 4.0;
    const double lowCut  = (mode == QLatin1String("strong")) ? -4.0 : -2.0;
    // Pre-attenuation: pull down by ~70% of the boost so there's headroom
    // for the lifted band without killing the loudness gain entirely.
    // linear = 10^(-dB/20). For +4dB boost → attenuate ~2.8dB → 0.72×;
    // for +8dB → ~5.6dB → 0.52×.
    const double preAttenDb = -0.7 * midGain;
    const double preAttenLin = std::pow(10.0, preAttenDb / 20.0);
    qCInfo(lcVidView) << "Speech boost ENABLED:" << mode
                      << "(mid +" << midGain << "dB, low" << lowCut
                      << "dB, pre-atten" << preAttenDb << "dB + limiter)";

    QStringList seg;
    // 1. Pre-attenuate (volume element is always present — core).
    seg << QStringLiteral("!") << QStringLiteral("volume")
        << QStringLiteral("volume=") + QString::number(preAttenLin, 'f', 4);
    // 2. Speech-band EQ lift.
    seg << QStringLiteral("!") << QStringLiteral("equalizer-3bands")
        << QStringLiteral("band0=") + QString::number(lowCut, 'f', 1)
        << QStringLiteral("band1=") + QString::number(midGain, 'f', 1)
        << QStringLiteral("band2=0.0");
    // 3. Soft-knee limiter to catch residual peaks (if available).
    if (playbackElementAvailable(QStringLiteral("audiodynamic"))) {
        // compressor mode, soft-knee, high ratio near the ceiling = limiter.
        // threshold 0.85 (≈ -1.4 dBFS) so it only grabs the top of the range;
        // ratio 0.3 (audiodynamic uses 0..1 where lower = more compression).
        seg << QStringLiteral("!") << QStringLiteral("audiodynamic")
            << QStringLiteral("mode=compressor")
            << QStringLiteral("characteristics=soft-knee")
            << QStringLiteral("ratio=0.3")
            << QStringLiteral("threshold=0.85");
    } else {
        qCInfo(lcVidView) << "audiodynamic unavailable — boost relies on "
                             "pre-attenuation alone for clip avoidance";
    }
    return seg;
}

#ifdef SNAPCLONE_RUST_AUDIO
/// V32.34+ audio: VAD-gated speech detection. Decodes a short HEAD segment
/// of the file to raw mono S16LE PCM via a one-shot gst-launch, then runs
/// it through the snapclone-audio streaming VAD. Returns true if speech was
/// convincingly present in the sampled window.
///
/// Why a head sample and not the whole file: we only need "does this clip
/// contain speech worth boosting", and decoding the first few seconds is
/// fast and bounded. If the clip opens with music and speech starts later
/// we might miss it — acceptable for v1; a fuller scan (or in-process
/// per-frame gating, the real v2) is filed in the TODO.
///
/// The decode is to a memory buffer via `fdsink` would be ideal, but for
/// simplicity and to reuse the existing subprocess pattern we decode to a
/// temp .raw file, read it, score it, delete it. ~200–400ms on FP5 for a
/// 3-second window; runs once before playback starts.
bool fileContainsSpeech(const QString &path, int headSeconds)
{
    const QString gst = QStandardPaths::findExecutable(
        QStringLiteral("gst-launch-1.0"));
    if (gst.isEmpty()) return false;

    const QString rawPath =
        QStandardPaths::writableLocation(QStandardPaths::TempLocation)
        + QStringLiteral("/snapclone-vadprobe-")
        + QString::number(QCoreApplication::applicationPid())
        + QStringLiteral("-")
        + QString::number(QDateTime::currentMSecsSinceEpoch())
        + QStringLiteral(".raw");

    // Decode → mono S16LE @ 48k, capped at headSeconds via num-buffers is
    // awkward (buffer size varies), so we use `identity` + a hard process
    // timeout instead, and just read whatever landed.
    constexpr int kSampleRate = 48000;
    QStringList args;
    args << QStringLiteral("filesrc")
         << QStringLiteral("location=") + path
         << QStringLiteral("!") << QStringLiteral("decodebin")
         << QStringLiteral("!") << QStringLiteral("audioconvert")
         << QStringLiteral("!") << QStringLiteral("audioresample")
         << QStringLiteral("!")
         << QStringLiteral("audio/x-raw,format=S16LE,channels=1,rate=%1")
                .arg(kSampleRate)
         << QStringLiteral("!") << QStringLiteral("filesink")
         << QStringLiteral("location=") + rawPath;

    QProcess p;
    p.start(gst, args);
    // Bound the decode: headSeconds of audio decodes much faster than
    // real-time, but cap the wall-clock so a pathological file can't hang
    // playback startup. 2s wall-clock is plenty to decode 3s of audio.
    if (!p.waitForFinished(2000)) {
        p.kill();
        p.waitForFinished(500);
    }

    QFile f(rawPath);
    bool speech = false;
    if (f.open(QIODevice::ReadOnly)) {
        // Read up to headSeconds worth of samples; ignore the rest.
        const qint64 maxBytes =
            qint64(kSampleRate) * headSeconds * qint64(sizeof(qint16));
        const QByteArray pcm = f.read(maxBytes);
        f.close();
        const int nSamples = int(pcm.size() / qint64(sizeof(qint16)));
        if (nSamples > 0) {
            const qint16 *samples =
                reinterpret_cast<const qint16 *>(pcm.constData());
            // Stream 20ms frames through the VAD; declare speech if any
            // frame's smoothed likelihood crosses a confident threshold.
            VadDetector *vad = sc_audio_vad_new(kSampleRate);
            if (vad) {
                const int frameLen = kSampleRate / 50; // 20ms
                float peak = 0.0f;
                for (int off = 0; off + frameLen <= nSamples; off += frameLen) {
                    const float l = sc_audio_vad_process(
                        vad, samples + off, size_t(frameLen));
                    if (l > peak) peak = l;
                }
                sc_audio_vad_free(vad);
                speech = (peak > 0.55f);
                qCInfo(lcVidView) << "VAD speech probe: peak likelihood"
                                  << peak << "→" << (speech ? "SPEECH" : "no speech");
            }
        }
    }
    QFile::remove(rawPath);
    return speech;
}
#endif // SNAPCLONE_RUST_AUDIO
} // anon

VideoSnapViewerDialog::VideoSnapViewerDialog(const SnapClone::Core::Snap &snap,
                                              QWidget *parent)
    : MobileDialog(parent)
    , m_snap(snap)
{
    setWindowTitle(tr("Video Snap"));
    buildUi();

    if (m_snap.localPath.isEmpty() || !QFile::exists(m_snap.localPath)) {
        showHardFailureOverlay(tr("Video file not available on disk."));
        if (m_playBtn)    m_playBtn->setEnabled(false);
        if (m_saveBtn)    m_saveBtn->setEnabled(false);
        if (m_seekSlider) m_seekSlider->setEnabled(false);
        return;
    }

    // Backend selection:
    //   1. gst-launch available (Linux/postmarketOS/Fedora-Asahi): use it.
    //      Reliable across the codec mix we ship (.webm AV1+Opus from the
    //      transcoder, .mkv H.264+Opus from the recorder fallback). Doesn't
    //      hit the Qt6+FFmpeg7+ sws_getContext negative-dstH bug. Sidesteps
    //      Qt's video-backend entirely — pipeline produces JPEG frames we
    //      paint ourselves, audio goes to autoaudiosink (PipeWire / PulseAudio).
    //
    //   2. No gst-launch (Windows/macOS, or stripped Linux): fall back to
    //      QMediaPlayer + QVideoWidget. Has all the Qt-media issues but is
    //      our only option there.
    //
    // The Qt-media path can also be requested explicitly by setting
    // SNAPCLONE_VIEWER_BACKEND=qt in the environment — useful for testing.
    const QByteArray forced = qgetenv("SNAPCLONE_VIEWER_BACKEND");
    const bool preferQt = (forced == "qt");
    const bool preferGst = (forced == "gst") || (forced.isEmpty() && gstLaunchAvailable());

    if (preferGst && !preferQt) {
        qCInfo(lcVidView) << "Using gst-launch playback backend";
        if (!startGstPlayback()) {
            qCWarning(lcVidView) << "gst-launch start failed — falling back to Qt media";
            startQtMediaPlayback();
        }
        return;
    }
    if (qtMediaBrokenFlag() && gstLaunchAvailable()) {
        qCInfo(lcVidView) << "Qt media previously failed this session "
                             "— using gst-launch backend directly";
        if (!startGstPlayback()) {
            showHardFailureOverlay(tr("Both Qt media and gst-launch are unavailable."));
        }
        return;
    }
    startQtMediaPlayback();
}

VideoSnapViewerDialog::~VideoSnapViewerDialog()
{
    teardownQtMedia();
    teardownGst();
}

void VideoSnapViewerDialog::buildUi()
{
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);

    // The QVideoWidget is created up-front for the Qt backend. If we fail
    // over to gst, we hide it and add a QLabel in the same slot — see
    // startGstPlayback().
    //
    // CRITICAL: KeepAspectRatio (Qt default but set explicitly for clarity).
    // Without this, QVideoWidget would stretch any source to fill the
    // widget. For mixed-aspect sources (3:4 from FP5 portrait camera vs
    // 9:16 desktop) on a 9:16 viewer panel, the 3:4 source would render
    // vertically elongated.
    m_video = new QVideoWidget(this);
    m_video->setAspectRatioMode(Qt::KeepAspectRatio);
    m_video->setStyleSheet(QStringLiteral("background: #000;"));
    m_video->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Expanding);
    m_video->setMinimumHeight(400);
    root->addWidget(m_video, 1);

    // Controls bar — shared between backends. Some controls are disabled
    // when the gst backend is active (no in-process pause/seek for a
    // gst-launch playback pipeline; we'd have to kill/restart).
    auto *controls = new QHBoxLayout;
    controls->setContentsMargins(Theme::SPACE_SM, Theme::SPACE_XS,
                                  Theme::SPACE_SM, Theme::SPACE_SM);

    // Buttons use ASCII text labels rather than unicode glyphs (⏸ ▶ ⤓ ✕).
    // Reason: Plasma Mobile's default font (Noto Sans on postmarketOS)
    // doesn't always cover the full Miscellaneous Symbols / Arrows
    // ranges, so glyphs render as missing-character boxes. ASCII is
    // universally available. We use short labels ("Pause"/"Play"/"Save"/
    // "Close") rather than icons — slightly larger buttons but readable
    // on every platform.
    m_playBtn = new QPushButton(tr("Pause"), this);
    m_playBtn->setMinimumHeight(Theme::TOUCH_ICON);
    m_playBtn->setMinimumWidth(Theme::TOUCH_ICON * 2);
    connect(m_playBtn, &QPushButton::clicked,
            this, &VideoSnapViewerDialog::onPlayPauseToggle);
    controls->addWidget(m_playBtn);

    m_seekSlider = new QSlider(Qt::Horizontal, this);
    m_seekSlider->setRange(0, 1000);
    connect(m_seekSlider, &QSlider::sliderMoved,
            this, &VideoSnapViewerDialog::onSeek);
    controls->addWidget(m_seekSlider, 1);

    m_timeLabel = new QLabel(QStringLiteral("00:00 / 00:00"), this);
    m_timeLabel->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 12px; min-width: 90px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    controls->addWidget(m_timeLabel);

    m_saveBtn = new QPushButton(tr("Save"), this);
    m_saveBtn->setMinimumHeight(Theme::TOUCH_ICON);
    m_saveBtn->setMinimumWidth(Theme::TOUCH_ICON * 2);
    m_saveBtn->setToolTip(tr("Save to disk"));
    connect(m_saveBtn, &QPushButton::clicked,
            this, &VideoSnapViewerDialog::onSaveClicked);
    controls->addWidget(m_saveBtn);

    auto *closeBtn = new QPushButton(tr("Close"), this);
    closeBtn->setMinimumHeight(Theme::TOUCH_ICON);
    closeBtn->setMinimumWidth(Theme::TOUCH_ICON * 2);
    closeBtn->setToolTip(tr("Close"));
    connect(closeBtn, &QPushButton::clicked, this, &QDialog::reject);
    controls->addWidget(closeBtn);

    root->addLayout(controls);
}

// ─── Qt media backend ────────────────────────────────────────────────────────

void VideoSnapViewerDialog::startQtMediaPlayback()
{
    m_player   = new QMediaPlayer(this);
    m_audioOut = new QAudioOutput(this);
    m_player->setAudioOutput(m_audioOut);
    m_player->setVideoOutput(m_video);

    // Note: previously installed an event filter on m_video for tap-to-toggle.
    // The viewer doesn't override eventFilter, so the install was a no-op.
    // If tap-to-toggle is desired in Qt-media mode, add an eventFilter
    // override that catches MouseButtonRelease and calls onPlayPauseToggle.

    connect(m_player, &QMediaPlayer::positionChanged,
            this, &VideoSnapViewerDialog::onMediaPositionChanged);
    connect(m_player, &QMediaPlayer::durationChanged,
            this, &VideoSnapViewerDialog::onMediaDurationChanged);
    connect(m_player, &QMediaPlayer::playbackStateChanged, this,
            [this](QMediaPlayer::PlaybackState s) {
        if (m_playBtn) {
            m_playBtn->setText(s == QMediaPlayer::PlayingState
                ? tr("Pause") : tr("Play"));
        }
    });

    // Track first-frame-painted: when this fires, we know Qt's pipeline
    // is functional and any LATER error is a transient hiccup, not a
    // fail-over trigger. videoSink() returns a QVideoSink whose
    // videoFrameChanged signal fires per frame.
    if (auto *sink = m_video->videoSink()) {
        connect(sink, &QVideoSink::videoFrameChanged, this,
                [this](const QVideoFrame &) {
            m_qtFramePainted = true;
        });
    }

    connect(m_player, &QMediaPlayer::errorOccurred, this,
            [this](QMediaPlayer::Error err, const QString &msg) {
        qCWarning(lcVidView) << "QMediaPlayer error:" << err << msg;
        // Two cases:
        //   1. Error BEFORE any frame painted → Qt's pipeline is broken
        //      for THIS file. Most common: Qt6 + FFmpeg 7/8 sws_getContext
        //      negative-dstH flip incompatibility. Fail over to gst.
        //   2. Error AFTER frames have painted → transient. We've already
        //      seen working playback; log and ignore.
        if (m_qtFramePainted) {
            qCInfo(lcVidView) << "QMediaPlayer transient error after playback "
                                 "started — ignoring";
            return;
        }
        if (m_gstActive) return;  // already failing-over

        qCInfo(lcVidView) << "QMediaPlayer failed before first frame — "
                             "scheduling fail-over to gst-launch backend";
        // Persist the failure: skip Qt for subsequent snaps this session.
        qtMediaBrokenFlag() = true;
        // Defer: we're inside the connected slot. Calling teardownQtMedia()
        // here would disconnect the very signal we're handling, which Qt
        // tolerates but is brittle. singleShot(0) runs after this slot
        // returns and Qt's signal-dispatch unwinds.
        QString capturedMsg = msg;
        QTimer::singleShot(0, this, [this, capturedMsg]() {
            teardownQtMedia();
            if (!startGstPlayback()) {
                // V32.34+ VVC: if the file is H.266 and this device has no VVC
                // decoder, say so specifically — otherwise the generic "backend
                // unavailable" hides the real, actionable cause (received a VVC
                // snap from a peer whose ffmpeg has libvvenc, but ours lacks
                // libvvdec). Cheap suffix check avoids a probe on the hot path.
                const bool looksVvc =
                    m_snap.localPath.endsWith(QStringLiteral(".mkv"), Qt::CaseInsensitive)
                    && !vvcDecodeAvailable();
                if (looksVvc) {
                    showHardFailureOverlay(
                        tr("This video uses H.266/VVC, which this device can't "
                           "decode.\n\nInstall an ffmpeg build with libvvdec, or "
                           "ask the sender to use the default codec."));
                } else {
                    showHardFailureOverlay(
                        tr("%1\n\nGStreamer backend also unavailable.")
                            .arg(capturedMsg));
                }
            }
        });
    });

    m_player->setSource(QUrl::fromLocalFile(m_snap.localPath));
    m_player->play();
}

void VideoSnapViewerDialog::teardownQtMedia()
{
    // V32.34+ shutdown-crash fix (mirrors teardownGst's closingDown gate).
    // The previous bare deleteLater() path had the SAME latent shutdown hazard
    // that bit the gst QProcess teardown: deleteLater() only deletes when the
    // event loop pumps AGAIN, but when teardownQtMedia() runs from the
    // destructor during app exit the loop may never spin again — so the
    // QMediaPlayer (and its backend pipeline/threads/file handles) and the
    // QAudioOutput were left to be torn down inconsistently during static
    // destruction → the "state left behind when a window closes" segfault. The
    // gst path got the closingDown() gate; this sibling teardown was missed.
    //
    // We disconnect ALL signals first (so no media callback can fire into a
    // half-destroyed dialog), then: when closing down, delete DIRECTLY (the
    // loop won't pump, and with signals disconnected we can't be inside a media
    // callback); during normal operation keep deleteLater() (the loop pumps, and
    // a synchronous delete could otherwise re-enter a media-backend callback on
    // the current stack).
    const bool closing = QCoreApplication::closingDown();
    if (m_player) {
        m_player->disconnect();
        m_player->stop();
        m_player->setSource(QUrl());      // release file handle
        m_player->setVideoOutput(nullptr); // detach from QVideoWidget
        if (closing) {
            delete m_player;
        } else {
            m_player->deleteLater();
        }
        m_player = nullptr;
    }
    if (m_audioOut) {
        m_audioOut->disconnect();
        if (closing) {
            delete m_audioOut;
        } else {
            m_audioOut->deleteLater();
        }
        m_audioOut = nullptr;
    }
}

// ─── GStreamer fallback backend ──────────────────────────────────────────────

bool VideoSnapViewerDialog::startGstPlayback()
{
    if (!gstLaunchAvailable()) {
        qCWarning(lcVidView) << "gst-launch-1.0 not found — can't fail over";
        return false;
    }
    // Reentrancy guard: if we're already running (or starting) the gst
    // backend, return early. This can happen if QMediaPlayer's
    // errorOccurred fires twice in quick succession and both deferred
    // singleShots queue a fail-over call — without the guard, we'd
    // spawn two gst-launch processes that fight over the audio sink.
    if (m_gstActive && m_gstProc) {
        qCInfo(lcVidView) << "startGstPlayback: already running, ignoring";
        return true;
    }
    m_gstActive = true;

    // ── pipeline ────────────────────────────────────────────────────────────
    // Pipeline strategy: use `decodebin` from the TOP, not `matroskademux`
    // per-pad. decodebin handles any container (webm, mkv, mp4) and any
    // codec autoplug-style — exactly what playbin would do internally,
    // minus the playback-policy logic we don't want here.
    //
    //   filesrc location=X ! decodebin name=dec
    //     dec. ! queue ! videoconvert ! videoscale ! video/x-raw,width=640
    //          ! jpegenc quality=85 ! multifilesink location=<tmp> max-files=1
    //     dec. ! queue ! audioconvert ! audioresample ! autoaudiosink
    //
    // Why decodebin-from-top vs matroskademux+per-pad:
    //   We previously tried the more explicit matroskademux pattern because
    //   it gives DETERMINISTIC pad names (video_0, audio_0). But on Alpine
    //   gstreamer (FP5/postmarketOS), matroskademux's downstream caps
    //   negotiation rejected the chain — "not-negotiated (-4)" propagating
    //   upstream. The transcoder hit the same error with the same pattern.
    //
    //   decodebin gets around this because it autoplugs decoders INTERNALLY
    //   and routes by CAPS: when downstream expects `video/x-raw`, decodebin
    //   links the video pad there; when downstream expects `audio/x-raw`,
    //   it links the audio pad there. The `dec.` aliases in gst-launch
    //   syntax are matched by gst-launch's parser against decodebin's
    //   dynamic pads as they appear (no race because gst-launch holds the
    //   pipeline in PAUSED until all aliases resolve).
    //
    //   The mild downside: gst-launch syntax for dynamic-pad linking is
    //   slightly less robust than static pads (occasional bugs in old
    //   gstreamer versions). But on modern gstreamer (1.20+) this is the
    //   recommended pattern, and it's how `playbin` works internally.
    //
    // Caps: width-only on videoscale (no PAR), per V32.9 fix that bit
    // recorder + transcoder for integer-overflow.
    //
    // Element order videoconvert → videoscale: videoconvert handles any
    // input format (including HW DMABuf from accelerated decoders),
    // emitting CPU memory in a videoscale-friendly format. videoscale
    // is strict about its input format.
    //
    // -e: clean EOS handling. No -q: errors go to stderr for diagnosis.
    m_gstPreviewPath = QStandardPaths::writableLocation(
                          QStandardPaths::TempLocation)
                     + QStringLiteral("/snapclone-playback-")
                     + QString::number(QCoreApplication::applicationPid())
                     + QStringLiteral("-")
                     + QString::number(QDateTime::currentMSecsSinceEpoch())
                     + QStringLiteral(".jpg");
    if (QFile::exists(m_gstPreviewPath)) QFile::remove(m_gstPreviewPath);

    // Probe BEFORE building the pipeline: a file with no audio track
    // would have the `dec. ! ... ! autoaudiosink` branch dangling
    // unlinked (decodebin only emits audio-pad if audio exists, which
    // would leave one of our `dec.` aliases unmatched). Without the
    // probe gst-launch might error at parse-time, or worse the audio
    // chain might be wired to video and freeze.
    const bool hasAudio = probeHasAudio(m_snap.localPath);
    qCInfo(lcVidView) << "Playback audio probe:" << hasAudio
                      << "for" << m_snap.localPath;

    QStringList args;
    args << QStringLiteral("-e")
         << QStringLiteral("filesrc")
         << QStringLiteral("location=") + m_snap.localPath
         << QStringLiteral("!") << QStringLiteral("decodebin")
         << QStringLiteral("name=dec")
         // Video branch
         << QStringLiteral("dec.")
         << QStringLiteral("!") << QStringLiteral("queue")
         << QStringLiteral("!") << QStringLiteral("videoconvert")
         // V32.34+ orientation fix: force video-direction=identity so any
         // rotation tag carried in the stream is IGNORED on playback. Our
         // recorder bakes the chosen rotation directly into the pixels via
         // its own `videoflip` before encoding, so the stored frames are
         // already upright. If the container (matroska) or the H.264 SEI
         // ALSO carries a rotation tag — which some encoders/muxers add
         // automatically from the source orientation — a player that honors
         // that tag rotates a SECOND time, producing the sideways/upside-
         // down result on the receiving end (sender's own viewfinder looked
         // fine because it never round-trips through the tag). Pinning
         // identity here makes playback trust the pixels, not the tag.
         //   method/video-direction=identity is a no-op transform when
         //   there's no tag, so this is free for correctly-tagged files.
         << QStringLiteral("!") << QStringLiteral("videoflip")
         << QStringLiteral("video-direction=identity")
         << QStringLiteral("!") << QStringLiteral("videoscale")
         // PAR=1/1 caps force square pixels in the output.
         //
         // Why this matters: JPEG has no concept of pixel-aspect-ratio.
         // The QImage we load + paint always treats pixels as square.
         // So if the source video has SAR != 1:1 (anamorphic footage,
         // some webcam modes, transcoded broadcast video), we MUST
         // rescale to square pixels here — otherwise the JPEG renders
         // as a deformed image (stretched or squished depending on SAR).
         //
         // Why no width cap: previous attempts to combine
         // `width=W,pixel-aspect-ratio=1/1` triggered an integer-overflow
         // path in gstvideoconvertscale.c's fixate code. PAR-only caps
         // (no width constraint) sidestep that. videoscale picks output
         // dimensions to satisfy PAR=1:1 while preserving DAR — usually
         // a no-op for SAR-already-1:1 sources.
         //
         // Width-cap removed: we previously had `width=640` here as a
         // performance hint but it forced UPSCALE for sub-640 sources
         // (FP5 480×640 portrait), which videoscale refuses, producing
         // "not-negotiated" upstream. Display downscaling happens later
         // when QImage::scaled fits the JPEG into m_gstFrameLabel.
         // V32.34+ channel-divergence guard: constrain format AND
         // pixel-aspect-ratio. Previously this caps filter only fixed
         // PAR; the format slot was unconstrained, allowing any output
         // from videoscale through. On pmOS edge channels with newer
         // gstreamer/decoder plugins, decodebin may autoplug a decoder
         // emitting an exotic 10-bit / 4:4:4 / planar format that the
         // intermediate `videoconvert` doesn't always coerce cleanly
         // before reaching jpegenc. The pipeline then fails negotiation
         // ("not-negotiated -4") and the dialog hangs until the watchdog
         // kills it. Symptom: "one video plays, the next doesn't" on
         // the same phone, or "video plays on phone A but not phone B"
         // when the two are on different pmOS update channels.
         //
         // The constraint accepts the JPEG-encoder-friendly formats
         // (8-bit, common subsamplings) as a UNION. videoconvert will
         // find a path from ANY upstream format to one of these.
         //
         //   I420 / NV12  — most common decoder outputs; jpegenc supports
         //                  both natively without further conversion
         //   YUY2 / UYVY  — interleaved YUV from some webcam-capture paths
         //   RGB / BGRx   — fallback for sources that go RGB end-to-end
         //
         // We keep PAR=1/1 from the previous design (forces square
         // pixels for QImage's pixel-by-pixel paint).
         << QStringLiteral("!") << QStringLiteral(
                "video/x-raw,format={I420,NV12,YUY2,UYVY,RGB,BGRx},"
                "pixel-aspect-ratio=1/1")
         << QStringLiteral("!") << QStringLiteral("jpegenc")
         << QStringLiteral("quality=85")
         << QStringLiteral("!") << QStringLiteral("multifilesink")
         << QStringLiteral("location=") + m_gstPreviewPath
         << QStringLiteral("max-files=1")
         // V32.22: sync=true forces multifilesink to honor the pipeline
         // clock, throttling JPEG writes to the video frame's running
         // time. Without this, multifilesink races through every video
         // frame as fast as decodebin can deliver them, finishing the
         // entire video's frames in < 1 second while autoaudiosink
         // (which DOES sync to the clock) plays audio at real time —
         // observed as "fast-forward then frozen frame for the rest
         // of the duration". The audio sink provides the pipeline clock,
         // so sync=true makes multifilesink wait until its turn at each
         // frame's timestamp.
         //
         // V32.34+: BUT — sync=true requires a clock provider in the
         // pipeline. autoaudiosink provides one; multifilesink does NOT.
         // For audioless videos we have no audio branch, so
         // multifilesink would have to be its own clock — it can't, and
         // GStreamer reports "do not want to preroll" / "sink rejected
         // state change" and the whole pipeline fails to start.
         //
         // For audioless videos we use sync=false: there's no real-time
         // reference to outrun, so "racing through frames" doesn't show
         // as fast-forward — the painter timer (Qt) reads the JPEG at
         // ~30Hz regardless of how fast multifilesink rewrites it.
         << (hasAudio ? QStringLiteral("sync=true")
                      : QStringLiteral("sync=false"));
    if (hasAudio) {
        // Audio branch (only when the file has an audio track — see
        // probeHasAudio).
        //
        // V32.34+ audio: optional playback-side speech boost. Inserted
        // between audioresample and autoaudiosink so it operates on the
        // resampled stream at the sink's rate. Empty (no-op) unless the
        // viewer enabled it via Settings → Accessibility → Speech boost.
        // Per the design brief this is per-viewer at playback time, so
        // the recording stays clean and others' copies are unaffected.
        args << QStringLiteral("dec.")
             << QStringLiteral("!") << QStringLiteral("queue")
             << QStringLiteral("!") << QStringLiteral("audioconvert")
             << QStringLiteral("!") << QStringLiteral("audioresample");
        args << buildSpeechBoostSegment(m_snap.localPath);   // "" or "! equalizer-3bands ..."
        args << QStringLiteral("!") << QStringLiteral("autoaudiosink");
    }

    m_gstProc = new QProcess(this);
    m_gstProc->setProcessChannelMode(QProcess::SeparateChannels);
    m_gstStderrBuf.clear();  // V32.34+: stale errors don't bleed into a new attempt

    connect(m_gstProc, &QProcess::readyReadStandardError, this, [this]() {
        if (!m_gstProc) return;
        const QByteArray err = m_gstProc->readAllStandardError();
        if (err.isEmpty()) return;
        qCInfo(lcVidView) << "[gst]" << err.trimmed();
        // V32.34+: buffer the last few KB of stderr so the failure
        // overlay can give a specific reason instead of generic
        // "exit 1". 4096 bytes is plenty — the last error message
        // is typically <500 bytes and we only need the tail.
        m_gstStderrBuf.append(err);
        constexpr int kMaxBuf = 4096;
        if (m_gstStderrBuf.size() > kMaxBuf) {
            m_gstStderrBuf.remove(0, m_gstStderrBuf.size() - kMaxBuf);
        }
    });

    connect(m_gstProc,
            QOverload<int, QProcess::ExitStatus>::of(&QProcess::finished),
            this,
            [this](int code, QProcess::ExitStatus) {
        // EOS reached → playback finished normally. Show the last frame
        // (it's still on m_gstFrameLabel) and stop the poll. Non-zero
        // exit → pipeline failed; show hard-failure overlay.
        qCInfo(lcVidView) << "gst-launch exited code" << code;
        if (m_gstPollTimer) m_gstPollTimer->stop();
        if (code != 0) {
            // V32.34+: translate well-known stderr patterns to user-
            // readable messages. The raw stderr is in the journal
            // anyway (we log at info level above); the overlay text
            // is the user-actionable summary. Patterns intentionally
            // narrow — if we can't recognize the failure we fall
            // back to the generic message.
            QString reason = tr("Playback pipeline failed (exit %1).").arg(code);
            const QString errLower = QString::fromUtf8(m_gstStderrBuf).toLower();
            if (errLower.contains(QLatin1String("matroskademux"))
                && errLower.contains(QLatin1String("not-negotiated"))) {
                reason = tr("Can't decode this video — no decoder "
                            "available for its codec. The file is "
                            "fine; another player on a fully-equipped "
                            "system should open it.");
            } else if (errLower.contains(QLatin1String("no such element"))
                       || errLower.contains(QLatin1String("element \""))) {
                reason = tr("A required GStreamer element is missing. "
                            "Install gst-plugins-good, gst-plugins-bad "
                            "and gst-libav (or your distro's "
                            "equivalents) and try again.");
            } else if (errLower.contains(QLatin1String("no such file"))
                       || errLower.contains(QLatin1String("could not open"))) {
                reason = tr("Video file not readable — the recording "
                            "may have been deleted or moved before "
                            "playback could open it.");
            }
            showHardFailureOverlay(reason);
        } else {
            // Reset play button to "Replay" to indicate played-through.
            // Re-enable so the user can tap to REPLAY. onPlayPauseToggle
            // detects "in gst mode + process not running" and respawns
            // the pipeline.
            if (m_playBtn) {
                m_playBtn->setText(tr("Replay"));
                m_playBtn->setEnabled(true);
                m_playBtn->setToolTip(tr("Replay"));
            }
            if (m_timeLabel) m_timeLabel->setText(tr("Done"));
        }
    });

    qCInfo(lcVidView) << "Starting gst-launch playback:"
                      << QStandardPaths::findExecutable(QStringLiteral("gst-launch-1.0"))
                      << args.join(QLatin1Char(' '));
    m_gstProc->start(QStringLiteral("gst-launch-1.0"), args);
    if (!m_gstProc->waitForStarted(2000)) {
        qCWarning(lcVidView) << "gst-launch failed to start — leaving layout intact"
                                " for Qt-media fallback";
        m_gstProc->deleteLater();
        m_gstProc = nullptr;
        m_gstActive = false;
        return false;
    }

    // ── Layout swap: only do this AFTER gst-launch is confirmed running ────
    //
    // If we did the swap earlier and then `waitForStarted` returned false,
    // the caller's Qt-media fallback would find m_video gone from the
    // layout — broken dialog. Doing the swap here means: gst started OK,
    // we're committed to this backend, safe to swap widgets.
    m_video->hide();
    if (auto *root = qobject_cast<QVBoxLayout *>(layout())) {
        root->removeWidget(m_video);
    }
    if (!m_gstFrameLabel) {
        m_gstFrameLabel = new QLabel(this);
        m_gstFrameLabel->setStyleSheet(QStringLiteral(
            "background: #000; color: %1; font-size: 13px;")
            .arg(QLatin1String(Theme::TEXT_SECONDARY())));
        m_gstFrameLabel->setAlignment(Qt::AlignCenter);
        m_gstFrameLabel->setSizePolicy(QSizePolicy::Expanding,
                                        QSizePolicy::Expanding);
        m_gstFrameLabel->setMinimumHeight(400);
        // Show a placeholder until the first JPEG frame lands. Decoder
        // warmup + jpeg encode typically takes 100-200 ms — without
        // placeholder text the user sees a black widget that looks dead.
        m_gstFrameLabel->setText(tr("Loading…"));
        // Insert at the top of the root layout — the controls bar (added
        // last in buildUi) stays below.
        if (auto *root = qobject_cast<QVBoxLayout *>(layout())) {
            root->insertWidget(0, m_gstFrameLabel, 1);
        }
    } else {
        // On replay: clear the leftover "Done" frame and show loading text.
        m_gstFrameLabel->setPixmap(QPixmap());
        m_gstFrameLabel->setText(tr("Loading…"));
    }

    // Poll the JPEG file at 20 Hz. Each new mtime triggers a load + paint.
    if (!m_gstPollTimer) {
        m_gstPollTimer = new QTimer(this);
        m_gstPollTimer->setInterval(50);  // 20 Hz
        connect(m_gstPollTimer, &QTimer::timeout, this, [this]() {
            if (m_gstPreviewPath.isEmpty() || !m_gstFrameLabel) return;
            const QFileInfo fi(m_gstPreviewPath);
            if (!fi.exists()) return;
            // mtime check skips re-decoding when no new frame yet.
            const qint64 mt = fi.lastModified().toMSecsSinceEpoch();
            if (mt == m_gstPreviewMtime) return;
            m_gstPreviewMtime = mt;
            QImage img;
            if (!img.load(m_gstPreviewPath, "JPEG")) return;
            const QImage scaled = img.scaled(m_gstFrameLabel->size(),
                                              Qt::KeepAspectRatio,
                                              Qt::SmoothTransformation);
            m_gstFrameLabel->setPixmap(QPixmap::fromImage(scaled));
        });
    }
    m_gstPollTimer->start();

    // The gst backend doesn't expose duration / position cheaply (we'd
    // have to parse stderr or use a separate gst-discoverer process).
    // Disable controls that don't make sense for this backend so users
    // don't tap them expecting something.
    if (m_playBtn) {
        m_playBtn->setText(tr("Pause"));
        m_playBtn->setEnabled(false);
        m_playBtn->setToolTip(tr("Pause/seek not available in this playback mode"));
    }
    if (m_seekSlider) m_seekSlider->setEnabled(false);
    if (m_timeLabel)  m_timeLabel->setText(tr("Playing…"));
    return true;
}

void VideoSnapViewerDialog::teardownGst()
{
    if (m_gstPollTimer) {
        m_gstPollTimer->stop();
        m_gstPollTimer->deleteLater();
        m_gstPollTimer = nullptr;
    }
    if (m_gstProc) {
        // Pattern: disconnect EVERYTHING first so no finished/readyRead
        // signals fire into our (possibly half-destroyed) slots. Then kill
        // (SIGKILL) and REAP synchronously with a short bounded wait before
        // deleting the QProcess.
        //
        // V32.34+ fix: the previous "reparent to nullptr + deleteLater, no
        // waitForFinished" approach leaked at SHUTDOWN. deleteLater needs
        // the event loop to pump again to actually delete; when teardownGst
        // runs from the destructor during app exit, the loop may never spin
        // again, so the orphaned QProcess was destroyed by static teardown
        // while gst-launch was still running → "QProcess: Destroyed while
        // process is still running" and, in the rapid open→expire→close
        // cycle seen in the field log, a segfault on exit. A bounded
        // waitForFinished after SIGKILL reaps the child deterministically:
        // SIGKILL can't be caught, so the process dies in ~ms and the short
        // wait won't meaningfully re-enter slot dispatch (we've already
        // disconnected all signals from this QProcess, so even if the local
        // event loop pumps, none of our slots can fire on it).
        m_gstProc->QObject::disconnect();
        if (m_gstProc->state() != QProcess::NotRunning) {
            m_gstProc->kill();
            // Reap. 200ms is ample for a SIGKILL'd process; if the OS is
            // pathologically slow we fall through and delete anyway (the
            // kill is already delivered, so the kernel will reap the zombie
            // independently of our QProcess wrapper).
            m_gstProc->waitForFinished(200);
        }
        // V32.34+ crash fix: delete DIRECTLY, not deleteLater().
        //
        // The earlier deleteLater()+setParent(nullptr) approach had a latent
        // shutdown hazard that matches the field crash (open→expire→close→
        // exit ending in heap corruption / segfault): deleteLater() only
        // performs the delete when the event loop pumps again, but when
        // teardownGst() runs from the destructor during app exit, the loop
        // may NEVER spin again. Combined with setParent(nullptr) — which
        // detaches the QProcess from its parent's child list — NOTHING then
        // owns the object: the parent won't delete it (detached) and the
        // dead event loop won't honor the deferred delete. The orphaned
        // QProcess was left to be torn down inconsistently (static teardown
        // / leaked), corrupting the heap on the way out.
        //
        // Direct delete is safe here precisely because we've already (a)
        // disconnected ALL of this QProcess's signals above, so we cannot be
        // inside one of its signal emissions, and (b) reaped the child via
        // waitForFinished after an uncatchable SIGKILL. The original
        // deleteLater rationale ("in case we're inside a signal emission
        // from it") no longer applies once disconnect() has run.
        delete m_gstProc;
        m_gstProc = nullptr;
    }
    if (!m_gstPreviewPath.isEmpty()) {
        QFile::remove(m_gstPreviewPath);
        m_gstPreviewPath.clear();
    }
    m_gstPreviewMtime = 0;
    m_gstActive = false;
}

// ─── Hard-failure UI ─────────────────────────────────────────────────────────

void VideoSnapViewerDialog::showHardFailureOverlay(const QString &reason)
{
    qCWarning(lcVidView) << "Hard failure:" << reason;
    if (m_errorOverlay) return;
    m_errorOverlay = new QLabel(this);
    m_errorOverlay->setWordWrap(true);
    m_errorOverlay->setAlignment(Qt::AlignCenter);
    m_errorOverlay->setText(
        tr("This video can't be played here.\n"
           "%1\n\n"
           "Use the save button to export it and open in another player.")
        .arg(reason));
    m_errorOverlay->setStyleSheet(QStringLiteral(
        "background: rgba(40,40,40,0.92); color: %1;"
        "padding: 16px; border-radius: 8px; font-size: 13px;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    // Parent to whichever video display widget is currently visible.
    QWidget *host = m_gstActive ? static_cast<QWidget *>(m_gstFrameLabel)
                                : static_cast<QWidget *>(m_video);
    if (!host) host = this;
    m_errorOverlay->setParent(host);
    m_errorOverlay->setGeometry(host->rect().adjusted(20, 40, -20, -40));
    m_errorOverlay->show();
    if (m_playBtn)    m_playBtn->setEnabled(false);
    if (m_seekSlider) m_seekSlider->setEnabled(false);
}

// ─── Slots ───────────────────────────────────────────────────────────────────

void VideoSnapViewerDialog::onPlayPauseToggle()
{
    if (m_gstActive) {
        // gst mode: button is normally disabled during playback (no
        // pause without GStreamer C bindings). It's re-enabled by the
        // finished-handler when the process exits cleanly (EOS) so the
        // user can REPLAY. Tapping triggers a fresh gst-launch spawn
        // over the same file.
        const bool stillPlaying = m_gstProc
            && m_gstProc->state() != QProcess::NotRunning;
        if (stillPlaying) return;  // shouldn't reach here, button disabled

        // Replay: tear down the previous (finished) gst process and
        // start a fresh one. Layout already has m_gstFrameLabel — no
        // swap needed. Audio sink will pick up a fresh PipeWire stream.
        teardownGst();
        if (!startGstPlayback()) {
            showHardFailureOverlay(tr("Replay failed: gst-launch couldn't start."));
        }
        return;
    }
    // Qt mode: cheap pause/play toggle on the QMediaPlayer.
    if (!m_player) return;
    if (m_player->playbackState() == QMediaPlayer::PlayingState)
        m_player->pause();
    else
        m_player->play();
}

void VideoSnapViewerDialog::onSeek(int sliderPos)
{
    if (m_gstActive) return;
    if (!m_player) return;
    const qint64 dur = m_player->duration();
    if (dur > 0) m_player->setPosition(sliderPos * dur / 1000);
}

void VideoSnapViewerDialog::onMediaPositionChanged(qint64 ms)
{
    if (m_gstActive) return;
    if (!m_player || !m_seekSlider || !m_timeLabel) return;
    const qint64 dur = m_player->duration();
    if (dur > 0 && !m_seekSlider->isSliderDown()) {
        m_seekSlider->setValue(static_cast<int>(ms * 1000 / dur));
    }
    m_timeLabel->setText(QStringLiteral("%1 / %2")
        .arg(formatTime(ms)).arg(formatTime(dur)));
}

void VideoSnapViewerDialog::onMediaDurationChanged(qint64 ms)
{
    if (m_gstActive) return;
    if (!m_timeLabel || !m_player) return;
    m_timeLabel->setText(QStringLiteral("%1 / %2")
        .arg(formatTime(m_player->position())).arg(formatTime(ms)));
}

void VideoSnapViewerDialog::onSaveClicked()
{
    if (m_snap.localPath.isEmpty() || !QFile::exists(m_snap.localPath)) {
        QMessageBox::warning(this, tr("Save"), tr("Source file no longer exists."));
        return;
    }

    const QString ext = QFileInfo(m_snap.localPath).suffix();
    const QString defaultName = QStringLiteral("snap-from-%1-%2.%3")
        .arg(m_snap.senderId.value.left(12),
             m_snap.sentAt.toUTC().toString(QStringLiteral("yyyyMMdd-HHmmss")),
             ext.isEmpty() ? QStringLiteral("webm") : ext);

    const QString videosDir = QStandardPaths::writableLocation(QStandardPaths::MoviesLocation);
    const QString suggested = (videosDir.isEmpty() ? QDir::homePath() : videosDir)
                            + QStringLiteral("/") + defaultName;

    const QString dest = QFileDialog::getSaveFileName(this,
        tr("Save Video Snap"), suggested,
        tr("WebM Video (*.webm);;Matroska Video (*.mkv);;All Files (*)"));
    if (dest.isEmpty()) return;

    if (QFile::exists(dest)) QFile::remove(dest);
    if (!QFile::copy(m_snap.localPath, dest)) {
        QMessageBox::warning(this, tr("Save Failed"),
            tr("Could not write to %1").arg(dest));
        return;
    }
    qCInfo(lcVidView) << "Exported video to" << dest;
    emit videoExported(dest);
    emit snapKept(m_snap.id);   // V32.42+: host marks it kept (exempt from expiry)
}

} // namespace SnapClone::UI::Video
