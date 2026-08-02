# Can we approximate the model instead of reading it?

Two ideas hide in that question and they behave oppositely.

## Approximating the WEIGHTS: no, it loses to quantization

The natural form is low-rank: replace a `4096 x 4096` matrix with `U·V` at rank `r`. The arithmetic
settles it before any quality argument:

```
full                     16.78M params   x 4.85 bpw = 10.2 MB at Q4_K
rank-256  approximation   2.10M params   at f16     =  4.2 MB
rank-512  approximation   4.19M params   at f16     =  8.4 MB
rank-1024 approximation   8.39M params   at f16     = 16.8 MB
```

**A rank-512 approximation at f16 is barely smaller than the full matrix at Q4_K, and rank-1024 is
larger.** And a low-rank factor cannot itself be aggressively quantized without compounding two lossy
approximations.

The deeper reason it loses: quantization keeps every weight and reduces precision; low rank discards
structure. Empirically LLM weight matrices tolerate the first far better than the second — their
singular-value spectra decay slowly, so truncating to a useful rank removes something the model was
using.

There is also a hard information argument. A matvec output element is a dot product over ALL `k` inputs,
so its value depends on every weight in the row. No summary statistic gives the right answer without
touching them; anything that claims to is answering a different question.

## Predicting WHICH weights matter: yes, and you are already running it

The version that works does not approximate the weights at all. It predicts, per token, which weights
will contribute ~nothing, and skips **reading** them. That is a bandwidth saving rather than an
approximation of one, and it is exactly the shape that helps a bandwidth-bound decoder.

**MoE is this idea made explicit.** Scout routes each token to 1 of 16 experts and reads one sixteenth
of the FFN weights — the router IS the predictor, trained jointly rather than bolted on. The reason
Scout's active set is ~10.5 GiB against a 34 GiB model is precisely this.

For DENSE models the same trick is available under the name contextual sparsity: a small predictor says
which FFN neurons will be near-zero for this token, and their rows are never read. The published results
claim large fractions skippable with small quality cost, and the mechanism is sound — ReLU-family
activations really do produce mostly-zero intermediates.

### What it would cost here

* The predictor must be **cheaper than the rows it saves**. At 113 GiB/s CPU and 258 GiB/s GPU, a
  predictor that costs more than a few percent of the layer erases its own benefit.
* It must be **conservative in the right direction**. Skipping a row that mattered is a wrong token;
  reading a row that did not is a wasted read. The asymmetry means the threshold should err toward
  reading, which reduces the saving.
* It needs the same measurement discipline as everything else: a gate comparing output against the
  unskipped path, because "close enough" here means a model that is subtly worse in ways no crash
  reveals.

## What this means for us

Nothing to build for dense models yet, and something to stop overlooking for MoE: the streaming path
already reads only the routed experts, so the win is banked. The open item is that **Scout's active set
of ~10.5 GiB per token fits the 48 GiB budget comfortably** — which is an argument for keeping it
resident rather than streaming, and that is a routing decision `hybridsplit` can already express.

The honest summary: you cannot compute with a summary of the weights, but you can decline to read the
ones that will not matter — and the second is what MoE has been doing all along.
