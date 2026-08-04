# Scout's speed on this device is bounded by expert streaming, not by kernels

## The arithmetic

Llama-4 Scout: `d=5120`, `ff=16384`, 48 layers, 16 experts, **top-1 routing**.

```
one expert-layer at q4      0.132 GiB
x 48 layers                 6.33 GiB fetched PER TOKEN
at 21 GiB/s (measured)      301 ms/token  =  3.3 tok/s ceiling
observed                    0.92 tok/s
```

Top-1 sounds cheap and is not. One expert per *layer* means 48 expert-layers per token, which is the
same bytes as one whole expert across the model — **6.33 GiB per token**, every token.

## Pinning cannot fix it

All 16 experts at q4 are **101 GiB**. The device holds ~31 GiB of buffers concurrently. Even at the
maximum usable budget only 3 of 16 pin, so ~80% of expert-layer fetches miss regardless.

```
16 GiB budget -> pin 1/16
24 GiB        -> pin 2/16
28-31 GiB     -> pin 3/16
```

Raising the budget from 16 to 28 is worth having — it roughly triples the hit rate from a very low
base — but it does not change the shape of the problem.

## What this means for the speed work

**The decode-kernel work does not apply here.** The 182 ms/token and 10 ms roofline measured on
DeepSeek-8B are about a *resident* model where the weights are already on the device and the cost is
memory latency. Scout on this hardware never gets there: it is upload-bound at 21 GiB/s, and a faster
matvec cannot help a kernel that is waiting for bytes to arrive.

**There is still 3.6× of headroom** between 0.92 and the 3.3 tok/s ceiling, and that gap is worth
chasing — it is prefetch, overlap and ring-slot scheduling, not kernel throughput. Whether the ring
double-buffers the next layer's expert while the current one computes is the question, and
`StreamPlan`'s doc claims it does.

**And the ceiling is what it is.** 3.3 tok/s is the honest best case for Scout on an M2 Ultra under
Asahi with a 21 GiB/s upload path. A model whose experts fit resident is a different regime entirely —
Qwen3-30B at 17 GiB is fully resident and bound by kernels rather than transfers, which is where the
subgroup and vec4 work pays.

## Against the references

llama.cpp's `--n-gpu-layers` partial offload has the same structure and the same ceiling; its advantage
on this class of model is that it reads native Q4_K blocks with no requantization, so its transfer is
the file itself rather than a converted copy.

Colibri does not stream at all — it is single-model, fully resident by design, and simply refuses what
does not fit. That is a cleaner contract and a narrower one.
