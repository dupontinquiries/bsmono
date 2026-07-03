#ifndef SNAPCLONE_AUDIO_FFI_H
#define SNAPCLONE_AUDIO_FFI_H

// ─── snapclone_audio_ffi.h ───────────────────────────────────────────────────
//
// C ABI for the Rust snapclone-audio crate. Available when CMake is built
// with -DSNAPCLONE_RUST_AUDIO=ON (default ON).
//
// Classical (non-ML) voice-activity detection + speech-likelihood scoring,
// used by the accessibility "speech boost" feature. The playback-side speech
// boost (VideoSnapViewerDialog) can use this to apply the mid-band EQ lift
// ONLY when speech is actually present, instead of statically colouring all
// audio. No model files, no FFT library; pure f32 arithmetic — a few hundred
// microseconds per 20 ms frame on FP5-class ARM.
//
// ─────────────────────────────────────────────────────────────────────────────

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// ── Stateless one-shot scoring ───────────────────────────────────────────────

/// Score one frame of mono 16-bit PCM for speech likelihood, stateless.
/// `samples` points to `n` int16 values; `sample_rate` is the PCM rate in Hz
/// (e.g. 48000). Returns a likelihood in [0, 1] (0.0 on invalid input).
///
/// Blends short-time energy, zero-crossing rate, and speech-band (≈300Hz–
/// 3.4kHz) energy ratio. Stateless = fixed noise floor, fresh filter per
/// call. Good for batch "is this clip speech-heavy" decisions. For real-time
/// streaming prefer the stateful detector below (adaptive floor, continuous
/// filter, smoothed output for click-free boost ramping).
///
/// `samples` must be valid for reads of `n` int16 values, or NULL (→ 0.0).
float sc_audio_speech_likelihood(const int16_t *samples, size_t n,
                                 uint32_t sample_rate);

// ── Stateful streaming detector (opaque handle) ──────────────────────────────

/// Opaque streaming VAD detector. Create with sc_audio_vad_new, feed frames
/// with sc_audio_vad_process, free with sc_audio_vad_free. NOT thread-safe;
/// use one detector per audio stream from a single thread.
typedef struct VadDetector VadDetector;

/// Allocate a streaming detector for `sample_rate` Hz. Returns NULL on a
/// zero sample rate. Free exactly once with sc_audio_vad_free.
VadDetector *sc_audio_vad_new(uint32_t sample_rate);

/// Process one frame of mono int16 PCM. Returns the SMOOTHED speech
/// likelihood in [0, 1] (attack faster than release, so boost ramps in
/// quickly on speech onset and lingers briefly after). 0.0 on invalid input.
float sc_audio_vad_process(VadDetector *handle, const int16_t *samples,
                           size_t n);

/// Free a streaming detector. NULL is a safe no-op.
void sc_audio_vad_free(VadDetector *handle);

/// Sample rate the detector was created with (Hz); 0 if handle is NULL.
/// Debugging / sanity aid.
uint32_t sc_audio_vad_sample_rate(const VadDetector *handle);

/// ABI version probe; returns 1. Lets the C side confirm the crate linked
/// and matches the expected ABI before calling the other entry points.
int sc_audio_abi_version(void);

/* ─── Radio talk-mode transmit gate (Buzz push-to-talk) ─────────────────────
 *
 * Decides, per audio frame, whether the local mic should transmit. Modes:
 *   0 Off            — monitor only, never transmit
 *   1 PushToTalk     — transmit while the talk control is held
 *   2 VoiceActivated — VOX: open on speech with squelch hysteresis + hang
 *   3 Continuous     — always open (hands-free)
 * VOX consumes the VAD likelihood (sc_audio_vad_process / speech_likelihood).
 * Opaque handle; create, drive per frame, free. Pair with the group
 * FloorControl (snapclone_group_ffi.h) for group push-to-talk. */
#define SC_RADIO_MODE_OFF             0
#define SC_RADIO_MODE_PUSH_TO_TALK    1
#define SC_RADIO_MODE_VOICE_ACTIVATED 2
#define SC_RADIO_MODE_CONTINUOUS      3

typedef struct TransmitGate TransmitGate;

/// Create a gate with explicit squelch tuning (VOX open/close thresholds in
/// [0,1] with open > close; hang_ms keeps it open through pauses; min_open_ms
/// is the anti-stutter floor). Free with sc_radio_gate_free.
TransmitGate *sc_radio_gate_new(float open_threshold, float close_threshold,
                                uint32_t hang_ms, uint32_t min_open_ms);
/// Create a gate with the default squelch tuning.
TransmitGate *sc_radio_gate_new_default(void);
/// Advance one frame; returns 1 to transmit, 0 to stay silent (0 on null).
/// `mode` is one of SC_RADIO_MODE_*; `ptt_held` and the return are 0/1;
/// `vad_likelihood` in [0,1] (ignored unless VoiceActivated); `now_ms` is a
/// monotonic clock.
int sc_radio_gate_update(TransmitGate *handle, int mode, int ptt_held,
                         float vad_likelihood, uint64_t now_ms);
/// Force the gate closed (channel torn down / floor revoked). Null-safe.
void sc_radio_gate_reset(TransmitGate *handle);
/// Free a gate from sc_radio_gate_new[_default].
void sc_radio_gate_free(TransmitGate *handle);

/* ─── Capture framer: re-chunk mic PCM into fixed voice frames ──────────────
 *
 * Qt's audio source delivers arbitrary-length mono i16 PCM; the VAD and the
 * Opus encoder want exact fixed-size frames (e.g. 20 ms = 320 samples @ 16 kHz).
 * The C++ mic callback pushes whatever buffer it receives, then drains frames in
 * a loop — each popped frame goes to sc_audio_vad_process + the transmit gate
 * and, while transmitting, to the Opus encoder + group seal. Opaque handle;
 * mono only. Free with sc_capture_framer_free. */
typedef struct CaptureFramer CaptureFramer;

/// Create a framer (never null). Free with sc_capture_framer_free.
CaptureFramer *sc_capture_framer_new(uint32_t sample_rate, uint32_t frame_ms);
void sc_capture_framer_free(CaptureFramer *handle);
/// Samples per emitted frame (size your pop buffer to this). 0 on null.
int  sc_capture_framer_frame_samples(const CaptureFramer *handle);
/// Push n captured samples (null-safe no-op).
void sc_capture_framer_push(CaptureFramer *handle, const int16_t *samples, size_t n);
/// Complete frames available to pop right now (0 on null).
int  sc_capture_framer_available(const CaptureFramer *handle);
/// Pop one frame into out (capacity out_cap samples): 1 if written, else 0.
int  sc_capture_framer_pop(CaptureFramer *handle, int16_t *out, size_t out_cap);
/// Drop buffered samples (transmit stopped). Null-safe.
void sc_capture_framer_reset(CaptureFramer *handle);
/// Bound the backlog to max_frames; returns samples dropped (0 on null).
int  sc_capture_framer_cap_backlog(CaptureFramer *handle, size_t max_frames);

/// RMS level of n samples in [0,1] for a mic meter (stateless; 0 on null/empty).
float sc_audio_frame_rms(const int16_t *samples, size_t n);
/// Peak absolute level of n samples in [0,1] for clip indication (stateless).
float sc_audio_frame_peak(const int16_t *samples, size_t n);

/* ─── Mic conditioner: boost + gentle compression ──────────────────────────
 * Lifts the very quiet Fairphone-5 mic and tames the boosted peaks. Apply
 * per popped frame, in place, right after the framer (before VAD/encode).
 * Stateful (envelope follower) — one instance per capture stream. */
typedef struct MicConditioner MicConditioner;

/// Create with explicit params. Never null. Free with sc_mic_conditioner_free.
MicConditioner *sc_mic_conditioner_new(uint32_t sample_rate, float pre_gain_db,
                                       float threshold_db, float ratio,
                                       float attack_ms, float release_ms);
/// Create with Fairphone-5 defaults (+9 dB, -12 dBFS, 1.5:1, 0 ms, 100 ms).
MicConditioner *sc_mic_conditioner_new_default(uint32_t sample_rate);
/// Process n i16 samples in place (boost, compress, clamp). Null-safe no-op.
void sc_mic_conditioner_process(MicConditioner *handle, int16_t *samples, size_t n);
/// Reset the envelope (call when transmit stops).
void sc_mic_conditioner_reset(MicConditioner *handle);
void sc_mic_conditioner_free(MicConditioner *handle);

/* ─── HarshGuard: perceptual painful-sound limiter ─────────────────────────
 * Ducks the play-out when the signal turns painful (too loud / sharp / harsh)
 * to protect the listener. Apply per frame, in place, on the final play-out
 * PCM (after make-up). Stateful — one instance per play-out stream. */
typedef struct HarshGuard HarshGuard;

/// Create with explicit params. Never null. Free with sc_harsh_guard_free.
HarshGuard *sc_harsh_guard_new(uint32_t sample_rate, float peak_threshold_db,
                               float crest_threshold, float hf_ratio_threshold,
                               float max_attenuation_db, float attack_ms,
                               float release_ms);
/// Create with defaults (-3 dBFS peak, crest 6, HF 0.55, -12 dB, 1/220 ms).
HarshGuard *sc_harsh_guard_new_default(uint32_t sample_rate);
/// Process n i16 samples in place (duck painful sounds). Null-safe no-op.
void sc_harsh_guard_process(HarshGuard *handle, int16_t *samples, size_t n);
/// Reset the envelopes/gain.
void sc_harsh_guard_reset(HarshGuard *handle);
void sc_harsh_guard_free(HarshGuard *handle);

/* ─── OutputAgc: play-out loudness normalizer ──────────────────────────────
 * Drives the play-out toward a target loudness: boosts quiet far-end speech by
 * up to a ceiling and eases loud speech down to the same target, so every call
 * sits at a consistent volume. A tanh soft-limiter means the boost can never
 * hard-clip. Apply per frame, in place, on the play-out PCM. Stateful — one
 * instance per play-out stream. Apply BEFORE HarshGuard is NOT required; the
 * receiver runs HarshGuard first (tame painful content at natural level), then
 * this (normalize), so the two don't fight. */
typedef struct OutputAgc OutputAgc;

/// Create with explicit params. Never null. Free with sc_output_agc_free.
OutputAgc *sc_output_agc_new(uint32_t sample_rate, float target_rms,
                             float max_gain_db, float min_gain_db,
                             float noise_floor_db, float attack_ms,
                             float release_ms, float limit_ceiling);
/// Create with defaults (target -12 dBFS, +27/-6 dB, gate -55 dB, 10/300 ms, ceiling 0.97).
OutputAgc *sc_output_agc_new_default(uint32_t sample_rate);
/// Normalize n i16 samples in place toward the target loudness. Null-safe no-op.
void sc_output_agc_process(OutputAgc *handle, int16_t *samples, size_t n);
/// Reset the AGC envelope/gain.
void sc_output_agc_reset(OutputAgc *handle);
void sc_output_agc_free(OutputAgc *handle);

/* ─── Voice wire format (versioned MessagePack) ─────────────────────────────
 *
 * One voice packet = an encoded frame + header, MessagePack-serialized here so
 * both ends share one definition. The send path builds a packet from the Opus
 * bytes + metadata, seals it, sends it; the receive path opens + decodes it for
 * the seq (→ jitter buffer) and payload (→ decoder). Protocol versioning is
 * baked in: every frame carries a version and the decoder rejects unknown ones. */
#define SC_VOICE_CODEC_OPUS   1
#define SC_VOICE_CODEC_PCM16  2
#define SC_VOICE_FLAG_NONE            0u
#define SC_VOICE_FLAG_TALKSPURT_START (1u << 0) /* first frame after silence */
#define SC_VOICE_FLAG_DTX             (1u << 1) /* Opus comfort-noise frame */

/* Current wire protocol version (frames we emit carry this). */
uint8_t sc_voice_protocol_version(void);

/* Encode a frame to MessagePack. ver is stamped internally. Returns 0; or
 * -1 (null out_len), -2 (out too small — out_len = required size), -3 (unknown
 * codec). Size `out` to ~ payload_len + 16. */
int sc_voice_frame_encode(uint32_t seq, uint32_t ts_ms, uint8_t codec, uint8_t flags,
                          const uint8_t *payload, size_t payload_len,
                          uint8_t *out, size_t out_cap, size_t *out_len);

/* Decode a frame. Any out field pointer may be NULL to skip it (out_seq and
 * out_payload_len must not be). Returns 0; or -1 (null), -2 (payload buffer too
 * small — out_payload_len = required), -4 (malformed), -5 (unsupported version —
 * out_ver = the offending version). Size out_payload to the input length and -2
 * cannot happen. */
int sc_voice_frame_decode(const uint8_t *bytes, size_t len,
                          uint8_t *out_ver, uint32_t *out_seq, uint32_t *out_ts_ms,
                          uint8_t *out_codec, uint8_t *out_flags,
                          uint8_t *out_payload, size_t out_payload_cap, size_t *out_payload_len);

/* ─── Jitter buffer (receive side) ─────────────────────────────────────────
 *
 * Absorbs network reordering/jitter/loss between arrival and steady play-out.
 * Push decoded wire frames (seq + payload) as they arrive; pop once per frame
 * interval on the play-out timer. Opaque handle; codec-agnostic. */
typedef struct JitterBuffer JitterBuffer;

JitterBuffer *sc_jitter_new(uint32_t target_frames, uint32_t max_frames);
void sc_jitter_free(JitterBuffer *handle);
/* Insert an arriving frame (null-safe). */
void sc_jitter_push(JitterBuffer *handle, uint32_t seq, const uint8_t *payload, size_t payload_len);
/* Pop one play-out result: 0 = frame written to out (out_len set) → decode+play;
 * 1 = conceal (run Opus PLC); 2 = silence; -1 = null; -2 = frame ready but out
 * too small (out_len = required, frame consumed — size out_cap ~4096). */
int sc_jitter_pop(JitterBuffer *handle, uint8_t *out, size_t out_cap, size_t *out_len);
/* Frames currently buffered (0 on null). */
int sc_jitter_buffered(const JitterBuffer *handle);
/* Clear + un-prime (new talk-spurt / teardown). Null-safe. */
void sc_jitter_reset(JitterBuffer *handle);

/* ─── Opus codec ────────────────────────────────────────────────────────────
 *
 * Encode/decode mono i16 PCM frames. AVAILABLE ONLY in a build of the audio
 * crate with the `opus` Cargo feature (links libopus); the symbols are absent
 * otherwise. One whole frame (2.5/5/10/20/40/60 ms) of PCM per call. */
typedef struct OpusEncoder OpusEncoder;
typedef struct OpusDecoder OpusDecoder;

/* VOIP-mode encoder. bitrate bits/s (<=0 → Opus default). NULL on bad args. */
OpusEncoder *sc_opus_encoder_new(uint32_t sample_rate, uint32_t channels, int32_t bitrate);
/// Apply quality/efficiency settings to an existing encoder (best-effort).
/// complexity 0..10 (lower = faster/less CPU); max_bandwidth 0=NB 1=MB 2=WB
/// 3=SWB 4=FB; dtx / inband_fec are 0|1; packet_loss_pct 0..100. Out-of-range
/// values leave that setting unchanged.
void sc_opus_encoder_apply_quality(OpusEncoder *handle, int32_t bitrate_bps,
                                   int32_t complexity, int32_t max_bandwidth,
                                   int32_t dtx, int32_t inband_fec,
                                   int32_t packet_loss_pct);
void sc_opus_encoder_free(OpusEncoder *handle);
/* Encode one frame of mono i16 PCM → out; returns bytes written, or -1. */
int sc_opus_encode(OpusEncoder *handle, const int16_t *pcm, size_t pcm_len,
                   uint8_t *out, size_t out_cap);

OpusDecoder *sc_opus_decoder_new(uint32_t sample_rate, uint32_t channels);
void sc_opus_decoder_free(OpusDecoder *handle);
/* Decode one frame → out PCM; returns samples written, or -1. data NULL/len 0
 * runs packet-loss concealment. fec requests in-band FEC recovery. */
int sc_opus_decode(OpusDecoder *handle, const uint8_t *data, size_t data_len,
                   int16_t *out, size_t out_cap, int fec);

/* ─── Voice security tiers ──────────────────────────────────────────────────
 * Public (default): lightweight ChaCha20-Poly1305 per frame (sc_voice_seal/open),
 * lowest latency. Private (opt-in, mutual): post-quantum hybrid key exchange
 * (the crypto layer, via ml-kem on rustc >= 1.81) + playback obfuscation. */

/* Lightweight per-frame AEAD (public tier). key = 32 B, nonce = 12 B. The nonce
 * MUST be unique per key: the channel maintains a session counter — the voice
 * seq restarts each talk-spurt, so do NOT use it directly as the nonce. */
#define SC_VOICE_AEAD_TAG_LEN   16
#define SC_VOICE_AEAD_KEY_LEN   32
#define SC_VOICE_AEAD_NONCE_LEN 12
/* Seal → out = ciphertext+tag (pt_len+16). 0; -1 null; -2 too small (out_len=req); -3 internal. */
int sc_voice_seal(const uint8_t *key32, const uint8_t *nonce12,
                  const uint8_t *aad, size_t aad_len,
                  const uint8_t *plaintext, size_t pt_len,
                  uint8_t *out, size_t out_cap, size_t *out_len);
/* Open → out = plaintext. 0; -1 null; -2 too small; -4 authentication failed. */
int sc_voice_open(const uint8_t *key32, const uint8_t *nonce12,
                  const uint8_t *aad, size_t aad_len,
                  const uint8_t *ciphertext, size_t ct_len,
                  uint8_t *out, size_t out_cap, size_t *out_len);

/* Security-mode negotiation. Private engages only if BOTH peers request it AND
 * the PQ handshake completes; until then voice flows over the (encrypted) public
 * tier — never in the clear. Opaque handle. */
typedef struct SecurityNegotiation SecurityNegotiation;
#define SC_VOICE_MODE_PUBLIC  0
#define SC_VOICE_MODE_PRIVATE 1
SecurityNegotiation *sc_voice_security_new(int local_request);
void sc_voice_security_free(SecurityNegotiation *h);
void sc_voice_security_set_remote(SecurityNegotiation *h, int wants_private);
void sc_voice_security_set_local(SecurityNegotiation *h, int wants_private);
void sc_voice_security_set_pq_ready(SecurityNegotiation *h, int ready);
int  sc_voice_security_mode(const SecurityNegotiation *h);        /* SC_VOICE_MODE_* */
int  sc_voice_security_awaiting_pq(const SecurityNegotiation *h); /* 1 while upgrading */

/* Playback obfuscation (private tier). Perturbs decoded audio to defeat machine
 * fingerprinting (Shazam-style peak matching, speaker ID, bot ASR) while keeping
 * speech intelligible. Seed per channel (e.g. from the session key); intensity
 * 0..1 (0 = passthrough). Process each playback frame in order. */
typedef struct Obfuscator Obfuscator;
Obfuscator *sc_obfuscator_new(uint64_t seed, uint32_t sample_rate, float intensity);
Obfuscator *sc_obfuscator_new_full(uint64_t seed, uint32_t sample_rate, float intensity,
                                   int whisper, int saturate, int decoy_tones,
                                   int gaps, int stutter);
void sc_obfuscator_free(Obfuscator *h);
void sc_obfuscator_process(Obfuscator *h, int16_t *frame, size_t n);

/* ─── Post-quantum hybrid handshake (private tier) ──────────────────────────
 * Agree a 32-byte voice session key via X25519 + ML-KEM-768 — safe if EITHER
 * holds. Initiator is stateful (new -> offer -> finish); responder is one-shot.
 * Feed the derived key to sc_voice_seal/open and the obfuscator seed, then call
 * sc_voice_security_set_pq_ready(1). (Always in the audio crate; ml-kem needs
 * rustc >= 1.81 to build.) */
#define SC_PQ_SESSION_KEY_LEN 32
typedef struct PqInitiator PqInitiator;

PqInitiator *sc_pq_initiator_new(void);
void sc_pq_initiator_free(PqInitiator *h);
/* Write the offer (x25519_pub || mlkem_ek) to send. 0; -1 null; -2 too small
 * (out_len = required). */
int sc_pq_initiator_offer(const PqInitiator *h, uint8_t *out, size_t out_cap, size_t *out_len);
/* Finish from the responder's answer; writes the 32-byte key. 0; -1 null; -3
 * malformed answer / decapsulation failed. */
int sc_pq_initiator_finish(const PqInitiator *h, const uint8_t *answer, size_t answer_len,
                           uint8_t *out_key32);
/* Responder one-shot: offer in -> answer bytes + 32-byte key out. 0; -1 null;
 * -2 answer buffer too small (out_answer_len = required); -3 malformed offer. */
int sc_pq_responder_answer(const uint8_t *offer, size_t offer_len,
                           uint8_t *out_answer, size_t out_answer_cap, size_t *out_answer_len,
                           uint8_t *out_key32);

#ifdef __cplusplus
} // extern "C"
#endif

#endif // SNAPCLONE_AUDIO_FFI_H
