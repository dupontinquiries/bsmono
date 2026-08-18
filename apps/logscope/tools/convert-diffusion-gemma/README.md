# convert-diffusion-gemma

HF `DiffusionGemma` (google/diffusiongemma-26B-A4B) safetensors → a GGUF the
engine recognises. Tensor names follow llama.cpp's `gemma4` conventions
(`blk.N.pre_ffw_norm_2`, `blk.N.ffn_gate_up_exps`, `blk.N.layer_output_scale`,
`blk.N.ffn_gate_inp.scale`, `blk.N.ffn_down_exps.scale`, …) plus two pinned
extensions for the diffusion-only weights, matched by `gemmadiff::read_shape`
and `--gate-gemmadiff` on the engine side:

- `blk.N.layer_output_scale_enc.weight` — the encoder-pass per-layer scalar
  (`model.encoder.language_model.layers.N.layer_scalar` in the HF drop).
- `self_cond.{pre_norm,ffn_gate,ffn_up,ffn_down}.weight` — the decoder-top
  self-conditioning FFN.

Metadata goes under `diffusion_gemma.*` (mirroring gemma4's keys:
`sliding_window_pattern`, `key_length` / `key_length_swa`, per-layer-type
`head_count_kv`, `rope.freq_base` / `freq_base_swa`, expert counts, softcap)
plus the shared `diffusion.*` block (`shift_logits` written **explicitly** as
the string `"false"` — in-place LLaDA lineage; the engine reads it with
`get_str` and a missing kv falls to a lineage default, which is exactly the
llada fork-GGUF failure this avoids). `tokenizer.ggml.mask_token_id` comes
from `tokenizer.json` (`<mask>` = 4 in the shipped drop).

## Usage

```sh
# real conversion (config.json + scheduler/ + generation_config.json + shards)
python3 convert.py /path/to/diffusiongemma-26B-A4B-it \
    --out diffusiongemma-26b-a4b-f16.gguf --dtype f16 \
    --gguf-py /path/to/llama.cpp/gguf-py \
    --tokenizer-gguf /path/to/any-gemma3.gguf   # donor for tokenizer.* kvs

# end-to-end selfcheck: tiny synthetic 2-layer model, no shards needed
python3 convert.py --selfcheck --gguf-py /path/to/llama.cpp/gguf-py
```

Needs `numpy` + `safetensors`; `gguf` either via `--gguf-py <llama.cpp>/gguf-py`
or `pip install gguf`. Shards are streamed one tensor at a time — peak memory
is one tensor, not 51 GB.

What it refuses rather than guesses: a canvas that does not tile into the
scheduler's block length; a `v_proj` present/absent pattern that disagrees with
the layer-type table (V-from-K is *per full-attention layer*, and the census is
checked, not assumed); an expert stack whose 3-D orientation is ambiguous
(gate_up and down stacks are detected by shape and permuted to the GGUF layout;
a shape that could be read two ways is an error, not a coin flip). Vision-tower
tensors are skipped — text pathway only.

## Open items

1. **Proportional rope factors.** Full-attention layers declare
   `rope_type: "proportional"` (partial rotary 0.25, θ=1e6). The drop ships no
   modeling code and no `rope_freqs` tensor, so the factor formula is unknown;
   the converter does **not** fabricate `blk.N.rope_freqs.weight`, and the
   engine names `PerLayerRopeFreqs`-adjacent behaviour in its gap report. When
   the reference lands (transformers 5.8 release or llama.cpp support), wire it
   here first.
2. **Tokenizer: SOLVED natively.** Gemma's SentencePiece model is BPE-type —
   merges carry the segmentation, no unigram scores exist to lose — so the
   converter now emits the full ggml tokenizer straight from `tokenizer.json`
   (262144 pieces, ~515k merges): `tokenizer.ggml.model = "llama"` + tokens +
   merges + token types, which the engine routes to its SP-BPE mode (`▁`
   spaces, `<0xNN>` byte fallback). Verify any emission against the reference
   implementation: `convert.py <hf_dir> --emit-tokenizer --out tok.gguf`
   embeds probe strings encoded by the HF `tokenizers` pipeline at emit time,
   and `kernel-validate --gate-tokenizer tok.gguf` re-encodes them with the
   engine and demands id equality. The `--tokenizer-gguf` donor path remains
   for weights-only redistributions — with a hazard now on record: a Gemma-3
   donor passes the 262144-width check while shipping DIFFERENT special
   strings at the same ids (105/106 are `<start_of_turn>`/`<end_of_turn>`
   there, `<|turn>`/`<turn|>` here). Width cannot see that; native emission
   makes it moot.
3. **IQ1_S production.** This tool emits f32/f16 GGUFs only. 1.5625-bpw files
   come from `llama-quantize` once the arch exists upstream. Note the expert
   `down` rows are 704 wide — not a multiple of the 256-weight IQ1_S block — so
   a real "IQ1_S" file is necessarily a per-tensor mix; the engine's loader and
   preflight already price mixed files per tensor.
4. **Engine forward.** A CPU reference forward now exists (`gemmadiffcpu.rs`):
   `--gate-gemmadiff <file>` loads an f32/f16 GGUF through it, checks causality
   on the file's own weights and denoises a block end to end (encode → denoise
   → commit → encode). The ENGINE forward — kernels, per-layer-head-dim KV
   layout, the app path — is still the eight-gap report, and `archcfg` keeps
   `supported: false` until a real checkpoint passes a gate on hardware. The
   `--selfcheck` fixture also carries `__fixture.iq1s_probe`, a pre-packed
   IQ1_S tensor the gate uses to pin the native-pricing and row-slice pathway
   against real file bytes; real conversions never emit it.
