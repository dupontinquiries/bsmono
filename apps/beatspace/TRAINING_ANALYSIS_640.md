# 640-frame run: what the checkpoints say

Read from `checkpoints-epoch337-640frames`, generator + discriminator
weights and the two JSON sidecars. No training was re-run; this is what
the files themselves report.

## The geometry, and the GUI error it causes

`config.json` and `resume.json` agree: `time_frames 640`, `n_mels 128`,
`latent 128`. The generator's first projection is therefore
`g.fc.w [81920, 128]`, since 81920 = 640 x 128.

That is exactly the `errors.log` failure:

    shape mismatch in set, lhs: [8192, 128], rhs: [81920, 128]

8192 = 64 x 128 — the `time_frames: 64` config DEFAULT. The GUI builds a
generator from the running config and only then reads the checkpoint, so
a checkpoint trained at 640 cannot load into a default-configured app.
The weights are fine; the geometry does not match. `load_generator` now
says that in those words and names the value to set, instead of printing
a raw tensor error once per frame.

## Two things worth knowing before spending more GPU time

**1. The run is at epoch 200, not 337.** `resume.json` records
`"epoch": 200`, and `gen_latest` is BYTE-IDENTICAL to `gen_000200`
(md5 `9c96a73a…`; the discriminators match too, `4896a261…`). With
`ckpt_every: 200`, that means the last write of both files was the same
event. Either the run stopped at 200 and the archive name is optimistic,
or it continued and nothing after epoch 200 was ever persisted — and
those have very different consequences. If it is the second, ~137 epochs
of compute exist only in a process that has since exited. Worth checking
the trainer's own log before assuming the model is further along than
its files are.

**2. The weights are healthy, which is the good news.** No NaN, no Inf,
anywhere in either network. 11.26M generator parameters, global mean
+0.00007 and std 0.032 — centred and tightly scaled, no sign of the
divergence a GAN at this size is prone to. Per-layer deconv stds sit in
a narrow 0.034-0.040 band across all four transposed convolutions, which
is what a stable, evenly-training stack looks like; a collapsed layer
shows up as a std an order of magnitude off its neighbours and none is.

The one figure to watch is `g.bn1.run_var`, max 8.81 against 4.06 and
6.59 for its bn0/bn2 siblings. That is a running statistic rather than a
learned weight, so it is not itself a fault — but a batchnorm layer
whose running variance is drifting up faster than the layers either side
of it is the first place instability shows, and with `batch_size: 4`
those statistics are noisy by construction. If a later checkpoint has it
climbing further, that is the layer to look at.

## What cannot be concluded from these files

Nothing about audio quality, mode collapse or whether the generator has
learned anything useful. Weight health says training did not blow up; it
says nothing about whether the output is music. That needs generated
samples and a listen, or at minimum a spectrogram compared against the
training distribution — neither of which is in this archive.
