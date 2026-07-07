#include "voice.h"
#include <math.h>
#include <string.h>
#include <stdlib.h>
#include <ctype.h>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

/* Fixed phoneme table: vowels are voiced two-formant tones (formant pair gives
 * the vowel identity); consonants are shorter, some voiced (a hum), some
 * unvoiced (a noise burst). All deterministic from the letter. */
typedef struct { float f1, f2; int voiced; float dur; } ph_t;

static ph_t phoneme(char c)
{
  switch (c) {
    case 'a': return (ph_t){ 720.f, 1150.f, 1, 0.085f };
    case 'e': return (ph_t){ 500.f, 1900.f, 1, 0.080f };
    case 'i': return (ph_t){ 320.f, 2300.f, 1, 0.075f };
    case 'o': return (ph_t){ 520.f,  900.f, 1, 0.090f };
    case 'u': return (ph_t){ 350.f,  780.f, 1, 0.085f };
    case 'y': return (ph_t){ 420.f, 1700.f, 1, 0.075f };
    default: break;
  }
  int voiced = (strchr("bdgjlmnrvwz", c) != NULL);
  float base = 170.f + (float)(c - 'a') * 24.f;   /* deterministic per letter */
  return (ph_t){ base, base * 1.6f, voiced, voiced ? 0.065f : 0.050f };
}

int ex_voice_synth(const char *text, float pitch, short **out)
{
  int len = (int)strlen(text);
  /* measure total length first */
  int total = 0;
  for (int i = 0; i < len; i++) {
    char c = (char)tolower((unsigned char)text[i]);
    if (c >= 'a' && c <= 'z') total += (int)(phoneme(c).dur * EX_VOICE_RATE);
    else if (c == ' ' || c == ',' || c == '.') total += (int)(0.045f * EX_VOICE_RATE);
  }
  if (total <= 0) total = (int)(0.06f * EX_VOICE_RATE);
  short *buf = (short *)calloc((size_t)total, sizeof(short));
  if (!buf) { *out = NULL; return 0; }

  int pos = 0;
  unsigned rng = 2166136261u;
  float phase1 = 0, phase2 = 0;
  for (int i = 0; i < len && pos < total; i++) {
    char c = (char)tolower((unsigned char)text[i]);
    if (c == ' ' || c == ',' || c == '.') { pos += (int)(0.045f * EX_VOICE_RATE); continue; }
    if (c < 'a' || c > 'z') continue;
    ph_t p = phoneme(c);
    int seg = (int)(p.dur * EX_VOICE_RATE);
    float f1 = p.f1 * pitch, f2 = p.f2 * pitch;
    for (int s = 0; s < seg && pos < total; s++, pos++) {
      float t  = (float)s / (float)EX_VOICE_RATE;
      float u  = (float)s / (float)seg;
      float env = sinf((float)M_PI * u);              /* bell attack/decay */
      float sig;
      if (p.voiced) {
        /* two formant tones, ring-modulated by a low warble -> wet gargle */
        phase1 += 2.f*(float)M_PI*f1/EX_VOICE_RATE;
        phase2 += 2.f*(float)M_PI*f2/EX_VOICE_RATE;
        float gargle = 0.55f + 0.45f * sinf(2.f*(float)M_PI*36.f*t)
                                     * (0.7f + 0.3f*sinf(2.f*(float)M_PI*6.f*t));
        sig = (sinf(phase1)*0.62f + sinf(phase2)*0.38f) * gargle;
      } else {
        rng = rng*1664525u + 1013904223u;
        float n = ((float)((rng >> 9) & 0x7FFF) / 16384.f) - 1.f;
        phase1 += 2.f*(float)M_PI*f1/EX_VOICE_RATE;
        sig = n*0.55f + sinf(phase1)*0.18f;           /* hiss + a little tone */
      }
      int v = (int)(env * sig * 8600.f);
      if (v > 32767) v = 32767; if (v < -32767) v = -32767;
      buf[pos] = (short)v;
    }
  }
  *out = buf;
  return total;
}

/* small ring of live voice sources so lines can overlap slightly */
#define VOICE_POOL 4
static ex_source_t *g_voice[VOICE_POOL];
static int g_voice_next = 0;

void ex_voice_say(const char *text, int speaker)
{
  static const float PITCH[4] = { 1.0f, 0.78f, 1.22f, 0.62f };  /* per-speaker voice */
  float pitch = PITCH[speaker & 3];
  short *pcm = NULL;
  int n = ex_voice_synth(text, pitch, &pcm);
  if (!pcm || n <= 0) { free(pcm); return; }
  if (g_voice[g_voice_next]) { ex_sound_free(g_voice[g_voice_next]); g_voice[g_voice_next] = NULL; }
  g_voice[g_voice_next] = ex_sound_pcm(pcm, n, EX_VOICE_RATE);
  free(pcm);
  if (g_voice[g_voice_next]) { ex_sound_gain(g_voice[g_voice_next], 0.8f); ex_sound_play(g_voice[g_voice_next]); }
  g_voice_next = (g_voice_next + 1) % VOICE_POOL;
}

void ex_voice_shutdown(void)
{
  for (int i = 0; i < VOICE_POOL; i++) if (g_voice[i]) { ex_sound_free(g_voice[i]); g_voice[i] = NULL; }
}
