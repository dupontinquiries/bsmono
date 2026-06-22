// ─── tools/ringtone_workshop.cpp ─────────────────────────────────────────────
//
// A standalone bin for workshopping the incoming-call ringtone WITHOUT building
// or running the full app (and without setting up a call). It drives the very
// same SnapClone::Audio::RingtoneSynth the app uses, so what you hear here is
// what the app plays.
//
// Renders one loop cycle to a .wav you can audition / A-B / drop into the repo.
// Every synth knob is a flag, including the proximity noise easter-egg
// (--noise / --noise-intensity / --seed) so you can dial in the textures by ear
// before wiring them to live landmark proximity.
//
// Build (from the project root):
//   cmake -B build -DSNAPCLONE_BUILD_TOOLS=ON
//   cmake --build build --target ringtone_workshop
// Run:
//   ./build/tools/ringtone_workshop --out ring.wav
//   ./build/tools/ringtone_workshop --notes 659.25,554.37,440 --echo-gain 0.5 --out a.wav
//   ./build/tools/ringtone_workshop --noise shimmer --noise-intensity 0.7 --seed 42 --out near.wav
//   ./build/tools/ringtone_workshop --out ring.wav --play     # if built with QtMultimedia
//
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/RingtoneSynth.hpp"

#include <QCoreApplication>
#include <QCommandLineParser>
#include <QByteArray>
#include <QFile>
#include <QDataStream>
#include <QStringList>
#include <QtGlobal>

#include <cstdio>

#ifdef RINGTONE_WORKSHOP_PLAY
#  include <QAudioFormat>
#  include <QAudioSink>
#  include <QAudioDevice>
#  include <QMediaDevices>
#  include <QBuffer>
#  include <QEventLoop>
#  include <QTimer>
#  include <QAudio>
#endif

namespace A = SnapClone::Audio;

// Write a canonical 44-byte-header PCM/float WAV. PCM bytes are host-endian and
// the header is little-endian; both our targets (x86, FP5/ARM) are LE so the
// samples line up directly.
static bool writeWav(const QString &path, const QByteArray &pcm,
                     int sampleRate, int channels, bool isFloat)
{
    const int bits = isFloat ? 32 : 16;
    const quint16 audioFormat = isFloat ? 3 : 1;            // 3 = IEEE float, 1 = PCM
    const quint32 byteRate    = quint32(sampleRate) * channels * (bits / 8);
    const quint16 blockAlign  = quint16(channels * (bits / 8));
    const quint32 dataSize    = quint32(pcm.size());

    QFile f(path);
    if (!f.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        std::fprintf(stderr, "error: cannot open %s for writing\n", qPrintable(path));
        return false;
    }
    QDataStream out(&f);
    out.setByteOrder(QDataStream::LittleEndian);

    out.writeRawData("RIFF", 4);
    out << quint32(36 + dataSize);
    out.writeRawData("WAVE", 4);

    out.writeRawData("fmt ", 4);
    out << quint32(16);              // fmt chunk size
    out << audioFormat;
    out << quint16(channels);
    out << quint32(sampleRate);
    out << byteRate;
    out << blockAlign;
    out << quint16(bits);

    out.writeRawData("data", 4);
    out << dataSize;
    out.writeRawData(pcm.constData(), int(pcm.size()));

    f.close();
    return true;
}

#ifdef RINGTONE_WORKSHOP_PLAY
// Play the rendered PCM once (no loop) so you can hear it from the CLI.
static void playOnce(const QByteArray &pcm, int sampleRate, int channels, bool isFloat)
{
    QAudioFormat fmt;
    fmt.setSampleRate(sampleRate);
    fmt.setChannelCount(channels);
    fmt.setSampleFormat(isFloat ? QAudioFormat::Float : QAudioFormat::Int16);

    const QAudioDevice dev = QMediaDevices::defaultAudioOutput();
    if (dev.isNull()) { std::fprintf(stderr, "no audio output device; skipping --play\n"); return; }
    if (!dev.isFormatSupported(fmt)) {
        std::fprintf(stderr, "device rejected format; skipping --play\n");
        return;
    }

    QBuffer buf;
    buf.setData(pcm);
    buf.open(QIODevice::ReadOnly);

    QAudioSink sink(dev, fmt);
    QEventLoop loop;
    QObject::connect(&sink, &QAudioSink::stateChanged, [&](QAudio::State s) {
        if (s == QAudio::IdleState || s == QAudio::StoppedState) loop.quit();
    });
    sink.start(&buf);
    // Safety timeout slightly longer than the cycle.
    QTimer::singleShot(8000, &loop, &QEventLoop::quit);
    loop.exec();
    sink.stop();
}
#endif

int main(int argc, char **argv)
{
    QCoreApplication app(argc, argv);
    QCoreApplication::setApplicationName("ringtone_workshop");
    QCoreApplication::setApplicationVersion("1.0");

    QCommandLineParser parser;
    parser.setApplicationDescription(
        "Render/audition the SnapClone incoming-call ringtone. Same synth as the app.");
    parser.addHelpOption();
    parser.addVersionOption();

    auto opt = [&](const QString &name, const QString &desc, const QString &val = QString()) {
        // Always a value-taking option (valueName = name); `val` is the default,
        // which may be empty (option still requires a value when present).
        parser.addOption(QCommandLineOption(name, desc, name, val));
    };
    opt("out",             "Output WAV path.", "ringtone.wav");
    opt("sr",              "Sample rate (Hz).", "44100");
    opt("channels",        "Channel count.", "1");
    parser.addOption(QCommandLineOption("float", "Render 32-bit float instead of 16-bit PCM."));
    opt("notes",           "Comma-separated note freqs in Hz (overrides default motif).");
    opt("gap",             "Seconds between note onsets when --notes is given.", "0.24");
    opt("note-sec",        "Per-note ring length (s).", "0.30");
    opt("decay",           "Note decay rate (higher = shorter).", "6.0");
    opt("echo-gain",       "Feedback-comb echo gain.", "0.40");
    opt("echo-delay",      "Feedback-comb echo delay (s).", "0.28");
    opt("master",          "Master output level.", "0.60");
    opt("cycle",           "Total loop length (s).", "2.60");
    opt("fade-out",        "Fade-out length (s) so the loop point is silent.", "0.60");
    opt("noise",           "Proximity noise kernel: none|airy|grain|shimmer.", "none");
    opt("noise-intensity", "Noise amount 0..1 (proximity: 0 far, 1 at landmark).", "0.0");
    opt("seed",            "Noise seed (per-landmark character).", "0");
#ifdef RINGTONE_WORKSHOP_PLAY
    parser.addOption(QCommandLineOption("play", "Also play the result once via the default device."));
#endif
    parser.process(app);

    A::RingtoneParams p = A::RingtoneParams::defaults();

    const int  sr       = parser.value("sr").toInt();
    const int  channels = parser.value("channels").toInt();
    const bool isFloat  = parser.isSet("float");

    if (parser.isSet("notes")) {
        const QStringList toks = parser.value("notes").split(',', Qt::SkipEmptyParts);
        const double gap = parser.value("gap").toDouble();
        p.notes.clear();
        int i = 0;
        for (const QString &t : toks) {
            bool ok = false;
            const double hz = t.trimmed().toDouble(&ok);
            if (ok && hz > 0.0) p.notes.push_back({ hz, gap * i++ });
        }
        if (p.notes.empty()) {
            std::fprintf(stderr, "error: --notes had no valid frequencies\n");
            return 2;
        }
    }
    if (parser.isSet("note-sec"))   p.noteSec     = parser.value("note-sec").toDouble();
    if (parser.isSet("decay"))      p.decayRate   = parser.value("decay").toDouble();
    if (parser.isSet("echo-gain"))  p.echoGain    = parser.value("echo-gain").toDouble();
    if (parser.isSet("echo-delay")) p.echoDelaySec= parser.value("echo-delay").toDouble();
    if (parser.isSet("master"))     p.master      = parser.value("master").toDouble();
    if (parser.isSet("cycle"))      p.cycleSec    = parser.value("cycle").toDouble();
    if (parser.isSet("fade-out"))   p.fadeOutSec  = parser.value("fade-out").toDouble();

    p.noiseKernel    = A::noiseKernelFromString(parser.value("noise").toUtf8());
    p.noiseIntensity = parser.value("noise-intensity").toDouble();
    p.noiseSeed      = parser.value("seed").toULongLong();

    const A::SampleFmt fmt = isFloat ? A::SampleFmt::Float32 : A::SampleFmt::Int16;
    const QByteArray pcm = A::renderPcm(p, sr, channels, fmt);
    if (pcm.isEmpty()) {
        std::fprintf(stderr, "error: render produced no samples (check --sr/--cycle)\n");
        return 3;
    }

    const QString outPath = parser.value("out");
    if (!writeWav(outPath, pcm, sr, channels, isFloat)) return 4;

    const double seconds = double(p.cycleSec);
    std::printf("wrote %s\n", qPrintable(outPath));
    std::printf("  %d Hz, %d ch, %s, %.2fs, %lld bytes\n",
                sr, channels, isFloat ? "float32" : "int16", seconds,
                static_cast<long long>(pcm.size()));
    std::printf("  notes:");
    for (const auto &n : p.notes) std::printf(" %.2f@%.2fs", n.freqHz, n.startSec);
    std::printf("\n  echo: gain=%.2f delay=%.2fs | master=%.2f | noise: %s intensity=%.2f seed=%llu\n",
                p.echoGain, p.echoDelaySec, p.master,
                A::noiseKernelName(p.noiseKernel), p.noiseIntensity,
                static_cast<unsigned long long>(p.noiseSeed));

#ifdef RINGTONE_WORKSHOP_PLAY
    if (parser.isSet("play")) playOnce(pcm, sr, channels, isFloat);
#endif
    return 0;
}
