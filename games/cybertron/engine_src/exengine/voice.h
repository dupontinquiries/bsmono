/* voice.h -- procedural robot "language". Maps text deterministically to a
 * warbling gargle: each letter has fixed phoneme characteristics, so the same
 * word always sounds the same -- a consistent constructed language rather than
 * random noise. Different speakers shift pitch/formant so characters differ. */
#ifndef EX_VOICE_H
#define EX_VOICE_H

#include <stddef.h>
#include "sound/sound.h"

#define EX_VOICE_RATE 16000

/* Synthesize `text` into a freshly-malloc'd 16-bit mono PCM buffer at
 * EX_VOICE_RATE. `pitch` scales all formants (1.0 = default; >1 higher voice).
 * Returns the sample count and sets *out (caller frees). Deterministic. */
int ex_voice_synth(const char *text, float pitch, short **out);

/* Synthesize and play `text` as a speaker's voice. Manages a small voice-source
 * pool internally (frees the oldest). `speaker` 0..3 picks a voice character. */
void ex_voice_say(const char *text, int speaker);

/* Free any voice sources still held (call at shutdown). */
void ex_voice_shutdown(void);

#endif
