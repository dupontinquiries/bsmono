#pragma once
// ─── core/VoiceDenoiser.hpp ───────────────────────────────────────────────────
//
// SCAFFOLD for RNN noise suppression in the ANC voice profile. A real model
// (coming from another project) will plug in behind this interface; until then
// the stage is INERT — it passes audio through untouched ("muted", i.e. it does
// no processing rather than emitting silence, so calls still sound normal).
//
// Contract: process() runs in place on a mono int16 frame at the capture sample
// rate, just before encode, when the active profile enables denoising. It must
// be cheap and allocation-free on the hot path. When `m_model` is null (no model
// loaded) it returns immediately, leaving the frame unchanged.
//
// To wire the real model: give loadModel() the weights, set m_model, and replace
// the body of the `if (m_model)` branch in process() with the inference call.
// Nothing else in the pipeline needs to change.
// ─────────────────────────────────────────────────────────────────────────────

#include <cstdint>
#include <cstddef>

namespace SnapClone::Core {

class VoiceDenoiser {
public:
    VoiceDenoiser() = default;

    /// True once a model is loaded and process() will actually denoise.
    [[nodiscard]] bool active() const { return m_model != nullptr; }

    /// Enable/disable the stage. Has no effect until a model is loaded — with no
    /// model the stage is inert regardless, so enabling it is a no-op for now.
    void setEnabled(bool on) { m_enabled = on; }
    [[nodiscard]] bool enabled() const { return m_enabled; }

    /// Load model weights. STUB: the real loader (from the incoming noise model)
    /// will parse `data` and set m_model. Returns false here — no model yet.
    bool loadModel(const void * /*data*/, std::size_t /*len*/) { return false; }

    /// In-place denoise of one mono int16 frame. No-op until a model is loaded.
    void process(int16_t *pcm, std::size_t samples)
    {
        if (!m_enabled || m_model == nullptr || pcm == nullptr || samples == 0)
            return; // inert ("muted") — leave the audio exactly as captured
        // ── real RNN inference goes here once m_model is wired ──
        // model_infer(m_model, pcm, samples);
    }

private:
    void *m_model = nullptr;   // opaque model handle; null = no model loaded
    bool  m_enabled = false;   // profile toggles this (ANC on); inert w/o a model
};

} // namespace SnapClone::Core
