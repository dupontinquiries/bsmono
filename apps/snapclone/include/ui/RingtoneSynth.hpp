#pragma once
// ─── ui/RingtoneSynth.hpp ─────────────────────────────────────────────────────
//
// Parameterized synthesis for the incoming-call ringtone. QtCore-only — no
// QtMultimedia, no widgets — so the SAME code backs both IncomingCallDialog and
// the standalone `ringtone_workshop` tool (tools/), guaranteeing the tool
// previews exactly what the app plays.
//
// Produces ONE loop cycle of PCM (the ring repeats it). All timing is in seconds
// so it's sample-rate independent.
//
// ── Proximity noise easter-egg ────────────────────────────────────────────────
// The `noise*` params are a sonic easter egg: blend a seeded noise texture into
// the ring whose amount tracks how near you are to a shared landmark.
//   noiseIntensity : 0 = far (clean motif) … 1 = at the landmark (full texture)
//   noiseSeed      : per-landmark character (hash a landmark id into this)
//   noiseKernel    : which texture (airy / grainy / shimmering)
// The synth only consumes the scalar+seed+kernel; computing proximity from the
// LandmarkHash/Wayfinder system and feeding it here is the app's job (a follow-up
// hook in IncomingCallDialog::startRingtone — see comment there).
// ─────────────────────────────────────────────────────────────────────────────

#include <QByteArray>
#include <vector>
#include <cstdint>

namespace SnapClone::Audio {

enum class SampleFmt { Int16, Float32 };

/// Noise-texture kernels for the proximity easter-egg.
enum class NoiseKernel { None, Airy, Grain, Shimmer };

struct RingtoneNote {
    double freqHz   = 440.0;
    double startSec = 0.0;
};

struct RingtoneParams {
    std::vector<RingtoneNote> notes;       ///< the motif (order = play order)
    double noteSec    = 0.30;              ///< per-note ring length
    double attackSec  = 0.006;             ///< soft attack (anti-click)
    double decayRate  = 6.0;               ///< exp(-t*decayRate) bell decay
    double harm2      = 0.25;              ///< 2nd-harmonic mix (warmth)
    double harm3      = 0.08;              ///< 3rd-harmonic mix

    double echoDelaySec = 0.28;            ///< feedback-comb delay
    double echoGain     = 0.40;            ///< feedback-comb gain (decaying repeats)

    double cycleSec   = 2.60;              ///< total loop length
    double fadeInSec   = 0.005;            ///< anti-click in
    double fadeOutSec  = 0.60;             ///< fade so the loop point is silent
    double master      = 0.60;             ///< output level (clamped after)

    NoiseKernel noiseKernel    = NoiseKernel::None;
    double      noiseIntensity = 0.0;      ///< 0..1 (proximity)
    std::uint64_t noiseSeed    = 0;        ///< per-landmark character

    /// The app's default ring: E5 → C#5 → A4 (A-major arpeggio settling on root).
    static RingtoneParams defaults();
};

/// One loop cycle as a normalized, clamped float mono buffer.
std::vector<float> renderMono(const RingtoneParams &p, int sampleRate);

/// One loop cycle as host-endian PCM in `fmt`; mono is replicated across channels.
QByteArray renderPcm(const RingtoneParams &p, int sampleRate, int channels, SampleFmt fmt);

/// CLI/config helpers.
NoiseKernel  noiseKernelFromString(const QByteArray &s);   ///< unknown → None
const char  *noiseKernelName(NoiseKernel k);

} // namespace SnapClone::Audio
