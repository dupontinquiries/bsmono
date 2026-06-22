#include "ui/RingtoneSynth.hpp"

#include <algorithm>
#include <cmath>
#include <random>

namespace SnapClone::Audio {

namespace {
constexpr double kTwoPi = 6.28318530717958647692;
constexpr double kPi    = 3.14159265358979323846;

// Mix the proximity noise easter-egg into the (pre-master) motif buffer. Kept
// deliberately subtle (a low ceiling) so it colors the ring rather than masking
// it; a slow swell makes it breathe under the motif rather than sit flat.
void mixNoise(std::vector<float> &buf, int sr, const RingtoneParams &p)
{
    const int N = static_cast<int>(buf.size());
    if (N <= 0) return;

    std::mt19937_64 rng(p.noiseSeed ? p.noiseSeed : 0x9E3779B97F4A7C15ULL);
    std::uniform_real_distribution<double> uni(-1.0, 1.0);
    const double amt = std::clamp(p.noiseIntensity, 0.0, 1.0) * 0.18; // subtle ceiling
    if (amt <= 0.0) return;

    auto swell = [N](int i) { return std::sin(kPi * double(i) / double(N)); }; // 0→1→0

    switch (p.noiseKernel) {
    case NoiseKernel::Airy: {
        // High-passed (differentiated) white noise → an airy/breath bed.
        double prev = 0.0;
        for (int i = 0; i < N; ++i) {
            const double wn = uni(rng);
            const double hp = wn - prev;
            prev = wn;
            buf[size_t(i)] += float(hp * amt * swell(i));
        }
        break;
    }
    case NoiseKernel::Grain: {
        // Scattered Hann-windowed noise grains → a granular texture.
        const int grain = std::max(1, sr / 40);  // ~25 ms grains
        const int step  = std::max(1, sr / 12);   // a grain ~every 80 ms
        for (int s0 = 0; s0 < N; s0 += step) {
            for (int k = 0; k < grain && s0 + k < N; ++k) {
                const double win = 0.5 - 0.5 * std::cos(kTwoPi * k / grain);
                buf[size_t(s0 + k)] += float(uni(rng) * win * amt * 1.4 * swell(s0));
            }
        }
        break;
    }
    case NoiseKernel::Shimmer: {
        // Low-passed noise ring-modulated by a slowly sweeping carrier → shimmer.
        double lp = 0.0;
        for (int i = 0; i < N; ++i) {
            const double wn = uni(rng);
            lp += 0.05 * (wn - lp);                      // one-pole low-pass (tone)
            const double t = double(i) / sr;
            const double carrierHz = 1200.0 + 600.0 * std::sin(kTwoPi * 0.5 * t);
            const double carrier = std::sin(kTwoPi * carrierHz * t);
            buf[size_t(i)] += float(lp * carrier * amt * 2.0 * swell(i));
        }
        break;
    }
    case NoiseKernel::None:
    default:
        break;
    }
}
} // namespace

RingtoneParams RingtoneParams::defaults()
{
    RingtoneParams p;
    p.notes = { {659.25, 0.00}, {554.37, 0.24}, {440.00, 0.48} }; // E5, C#5, A4
    return p;
}

std::vector<float> renderMono(const RingtoneParams &p, int sr)
{
    if (sr <= 0) return {};
    const int N = static_cast<int>(double(sr) * p.cycleSec);
    if (N <= 0) return {};

    std::vector<float> buf(size_t(N), 0.0f);

    // ── motif: bell-like notes ──
    for (const auto &nt : p.notes) {
        const int s0 = static_cast<int>(nt.startSec * sr);
        const int ns = static_cast<int>(p.noteSec * sr);
        for (int k = 0; k < ns; ++k) {
            const int idx = s0 + k;
            if (idx < 0 || idx >= N) continue;
            const double t = double(k) / sr;
            double env = std::exp(-t * p.decayRate);
            if (p.attackSec > 0.0 && t < p.attackSec) env *= t / p.attackSec;
            const double f = nt.freqHz;
            const double w = std::sin(kTwoPi * f * t)
                           + p.harm2 * std::sin(kTwoPi * 2.0 * f * t)
                           + p.harm3 * std::sin(kTwoPi * 3.0 * f * t);
            buf[size_t(idx)] += float(w * env);
        }
    }

    // ── echo: feedback comb → decaying repeats ──
    const int D = static_cast<int>(p.echoDelaySec * sr);
    if (D > 0 && p.echoGain != 0.0)
        for (int i = D; i < N; ++i)
            buf[size_t(i)] += float(p.echoGain) * buf[size_t(i - D)];

    // ── proximity noise easter-egg ──
    if (p.noiseKernel != NoiseKernel::None && p.noiseIntensity > 0.0)
        mixNoise(buf, sr, p);

    // ── master level + fades (loop point lands on silence) ──
    const int fadeIn  = std::max(1, static_cast<int>(p.fadeInSec * sr));
    const int fadeOut = std::max(1, static_cast<int>(p.fadeOutSec * sr));
    for (int i = 0; i < N; ++i) {
        double a = p.master;
        if (i < fadeIn)      a *= double(i) / double(fadeIn);
        if (i > N - fadeOut) a *= double(N - i) / double(fadeOut);
        double s = double(buf[size_t(i)]) * a;
        s = std::clamp(s, -1.0, 1.0);
        buf[size_t(i)] = float(s);
    }
    return buf;
}

QByteArray renderPcm(const RingtoneParams &p, int sr, int channels, SampleFmt fmt)
{
    if (channels < 1) channels = 1;
    const std::vector<float> mono = renderMono(p, sr);
    const int N = static_cast<int>(mono.size());
    if (N <= 0) return {};

    QByteArray pcm;
    if (fmt == SampleFmt::Float32) {
        pcm.resize(qsizetype(N) * channels * qsizetype(sizeof(float)));
        auto *out = reinterpret_cast<float *>(pcm.data());
        for (int i = 0; i < N; ++i)
            for (int c = 0; c < channels; ++c) *out++ = mono[size_t(i)];
    } else { // Int16
        pcm.resize(qsizetype(N) * channels * qsizetype(sizeof(qint16)));
        auto *out = reinterpret_cast<qint16 *>(pcm.data());
        for (int i = 0; i < N; ++i) {
            const qint16 v = static_cast<qint16>(mono[size_t(i)] * 32767.0f);
            for (int c = 0; c < channels; ++c) *out++ = v;
        }
    }
    return pcm;
}

NoiseKernel noiseKernelFromString(const QByteArray &s)
{
    const QByteArray k = s.trimmed().toLower();
    if (k == "airy")    return NoiseKernel::Airy;
    if (k == "grain")   return NoiseKernel::Grain;
    if (k == "shimmer") return NoiseKernel::Shimmer;
    return NoiseKernel::None;
}

const char *noiseKernelName(NoiseKernel k)
{
    switch (k) {
    case NoiseKernel::Airy:    return "airy";
    case NoiseKernel::Grain:   return "grain";
    case NoiseKernel::Shimmer: return "shimmer";
    case NoiseKernel::None:
    default:                   return "none";
    }
}

} // namespace SnapClone::Audio
