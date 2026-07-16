# kBM3 neural vocoder

Replaces Griffin-Lim in the mel→audio step. Griffin-Lim guesses phase over dozens
of iterations and still sounds metallic; a neural vocoder is one forward pass —
faster *and* far better — which is also what makes real-time audio in the explorer
possible.

## The key idea

Off-the-shelf vocoders (HiFi-GAN releases, etc.) expect a specific mel front-end
that does **not** match yours (128 mels, `power_to_db(ref=1.0)`, clipped to
[-80, 40] dB, then mapped to [-1, 1]). So instead of fighting that, this vocoder
learns to invert *your exact representation*: `wav_to_unit_mel()` here is
byte-for-byte the pipeline's cache-builder mel. The result — the GAN's generated
unit-mels feed straight into `vocode()` with no conversion.

## Architecture (verified)

HiFi-GAN-style generator: `mel [B,128,T]` → conv → four ConvTranspose1d upsamples
(rates 8·8·2·2 = 256 = hop) each followed by multi-receptive-field residual
blocks → `tanh` → `wav [B,1,T·256]`. Trained against a multi-period discriminator
with LS-GAN loss + feature matching, plus a multi-resolution STFT loss and an L1
log-mel reconstruction loss. Compact at **3.64M params**.

Self-test (`python vocoder.py test`, synthetic data, no audio needed) confirms:
`mel[2,128,32] → wav[2,1,8192]`, all losses backprop, and `vocode()` returns the
right-length waveform.

## Train it

The vocoder needs waveform targets, so it reads audio directly (recomputing the
matching mel per segment for perfect alignment):

```bash
pip install torch librosa soundfile numpy
python vocoder.py train --data ./music --iters 200000 --batch 16
# -> vocoder_ckpt/voc_*.pt
```

On CPU this is slow (like the GAN); it's a natural fit for your new Vulkan path.

## Use it

In the explorer server, pass the checkpoint — audio switches from Griffin-Lim to
the vocoder automatically:

```bash
python server.py --weights checkpoints/gen_8000.pt --vocoder vocoder_ckpt/voc_100000.pt
```

Anywhere else (e.g. `gan-zai-v5.2.py`'s sampling), swap Griffin-Lim for:

```python
from vocoder import load_vocoder, vocode
netG_voc, vcfg = load_vocoder("vocoder_ckpt/voc_100000.pt")
wav = vocode(unit_mel, vcfg, netG_voc)      # unit_mel: [n_mels, T] in [-1,1]
```

## Notes

For production quality you'd typically add weight-norm to the convs and a
multi-scale discriminator alongside the multi-period one; the multi-resolution
STFT loss here already covers much of what MSD contributes, and the code is kept
lean so it trains stably. The vocoder is fully convolutional, so it handles any
mel length at inference (the GAN's 128×256 → ~3s of audio).
