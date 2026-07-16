# kBM3 latent explorer

A web-based replacement for the tkinter explorer. The old one gave you a blank
canvas, reachable on only 3 of 128 latent dims, and a mislabeled "waveform"
(really just mel-band averages). This one puts the **live mel spectrogram** at
the centre — you see the sound reshape as you move — and adds real controls.

## Run it

**Standalone (demo synth):** open `index.html`. No server needed. A procedural
spectrogram *and* a WebAudio voice are both driven by the same 128-d latent, so
steering the field visibly and audibly changes the output. Good for feeling out
the interaction; the audio is a stand-in, not the model.

**With your model (real audio):** serve a trained checkpoint and connect:

```bash
pip install fastapi uvicorn torch librosa soundfile numpy
python server.py --weights checkpoints/gen_8000.pt --port 8000
# open http://localhost:8000
```

Or open `index.html` anywhere and click **connect model…**, then enter the
server URL. The server's `Generator` mirrors `gan-zai-v5.2.py` exactly, so a
`gen_*.pt` state_dict loads directly. `/generate` returns the mel (fast) and
Griffin-Lim audio (slower — lower `GL_ITERS` in `server.py` for snappier
response, or run on GPU).

## What you can do

- **Navigator** — drag the field to steer z₀/z₁; scroll for z₂. A motion trail
  shows where you've been; the cursor glows at the current point.
- **Latent dimensions** — sliders for z₀–z₇, plus randomize / reset / seed.
- **Morph** — capture point **A** and point **B**, then scrub the slider to
  crossfade, or **walk A→B→A** to auto-cycle. Endpoints are marked on the field.
- **Presets** — save the current point (with a spectrogram thumbnail), click to
  recall, export/import the bank as JSON.
- **Transport** — play/pause, loop, volume. Keyboard: Space play, R randomize,
  arrows nudge z₀/z₁, A/B set points.

## Design

Built as an audio instrument, not a webpage: studio-dark ground, a true magma
spectrogram as the luminous hero, a magenta/cyan signal duotone for controls,
Space Grotesk for labels + JetBrains Mono for every numeric readout. Responsive
to mobile, keyboard-navigable, honors reduced-motion.
