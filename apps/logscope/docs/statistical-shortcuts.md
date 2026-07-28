# Statistical shortcuts for a bandwidth-bound decoder — what works and what cannot

Decode reads the active weights once per token. At 800 GB/s a 30B model at Q4 caps out near 50 tok/s no
matter how clever the arithmetic is, so the only shortcuts that matter are ones that **read fewer
bytes**. That is the filter every idea below is put through.

## What cannot work: interpolating the token stream

**The vocabulary has no metric.** Token 5000 and 5001 are unrelated symbols; there is no sense in which a
logit vector is smooth over its index, so there is nothing to interpolate *across*. A "field" over
logits would be a field over an arbitrary permutation of itself.

**Curve-fitting the sequence is a worse language model.** Predicting token N+1 from a fit over tokens
1..N is exactly what the model does, using vastly more information. Any fit that reproduced it would
have to encode the same knowledge; any fit that is cheap is cheap because it does not.

This is worth stating plainly because the intuition behind it is a good one — there *is* structure to
exploit — it is just in a different axis.

## What can work: the layer dimension

Hidden states form a trajectory across layers, and for easy tokens the intermediate layers already
commit to the answer the final layer will give (the "logit lens" observation). Where that holds, the
remaining layers' weights **never needed to be read**.

That is a genuine bandwidth saving rather than an approximation of one, and it is measurable before it
is implemented:

* project layer L's hidden state through the output head, take the argmax, compare with the final
  token;
* the fraction of tokens where layer L already agrees is the fraction whose remaining layers were dead
  weight;
* the cost is one extra output projection per probe layer — real, but tiny beside 28 skipped layers.

The honest risk: agreement at layer L on *this* token does not guarantee it on the next, so a
confidence threshold and a fallback are needed, and a wrong early exit is a wrong token rather than a
slow one. That makes it a quality/bandwidth trade like speculative decoding — except speculative
decoding is exact and this is not, which is a real argument for doing speculation first.

## What can work: several small models instead of one large

Also a bandwidth argument, and the arithmetic decides it:

```
4 x 3B at Q4  ~=  6.7 GiB per token
1 x 30B at Q4 ~= 17.0 GiB per token
```

The panel is cheaper **per round**, so it gets roughly two and a half rounds before it costs what the
single model costs. That sets the bar precisely: the panel does not need to be *better* than the large
model, only *as good* within its round budget. `deliberate::round_budget` computes this rather than
asserting it, and a panel of ten 3B models gets exactly one round — at which point deliberation buys
nothing and the panel must be genuinely smaller to have anything to spend.

### The failure mode that makes naive voting worthless

Ensembles converge on the **majority's** error. Small models trained on similar data share their
mistakes, so three of them agreeing is far weaker evidence than three independent observers agreeing —
and a majority vote reports that weak agreement with the confidence of a quorum.

`Panel` therefore only credits agreement once the models have **demonstrated they can disagree** in the
same session. A panel that has never disagreed about anything is not a panel; it is one model queried
repeatedly, and `UnearnedConsensus` escalates rather than settling. This is the same discipline as
`reflex`: two cheap judges disagreeing is information, and two cheap judges agreeing is only information
if they were capable of disagreeing.

## On reinforcing the small models

Worth doing only after the panel demonstrably works, and the ordering is not arbitrary. Training small
models on deliberation traces amplifies whatever the panel already does — including its shared blind
spots. Fine-tuning on traces produced by a panel that has never disagreed would actively *entrench* the
correlation that makes its consensus meaningless.

So the sequence is: measure that the panel disagrees, measure that its consensus tracks the large
model's answers, and only then use its traces for anything.
