//! llamacpp_session.rs — a KV-cache-reusing session for Logscope's `LlamaCpp` backend.
//!
//! STATUS: sketch, NOT compiled (no macOS / no llama.cpp build in the sandbox). Written against the
//! llama-cpp-2 0.1.150 API as read from the crate source: `LlamaContextParams::with_flash_attention_policy`,
//! `with_type_k/with_type_v(KvCacheType)`, `with_n_ubatch`, `with_n_threads_batch`,
//! `LlamaContext::clear_kv_cache_seq(seq, p0, p1)` (llama_memory_seq_rm underneath).
//!
//! Why this exists: the current `generate_stream` creates a NEW LlamaContext per call, so every turn
//! re-prefills the whole conversation. Ollama/llama-server keep the context and reuse the prompt prefix,
//! which is most of the "slower than Ollama" in agent loops (TTFT per turn, not tok/s).
//!
//! Lifetime gotcha (the one that costs time): `LlamaContext<'a>` borrows `&'a LlamaModel`. You cannot put
//! the model and its context in one plain struct without a self-referential cell (`self_cell`/`ouroboros`).
//! The cheap way is to own both on the worker thread's stack — the thread that already owns the (!Send)
//! backend in `logscope-egui/src/backend.rs` — and keep the session alive across requests there.
//!
//! Cargo features to add in crates/logscope/Cargo.toml (Metal is what Ollama uses on Macs; no Vulkan SDK):
//!   llamacpp-metal  = ["llama-ffi", "llama-cpp-2/metal"]
//!   llamacpp-vulkan = ["llama-ffi", "llama-cpp-2/vulkan"]          # keep for Asahi; = today's `llamacpp`
//!   llamacpp-dyn    = ["llama-ffi", "llama-cpp-2/dynamic-backends"] # one binary, ggml backend .dylib/.so
//!                                                                    # chosen at runtime (needs dynamic-link;
//!                                                                    # more moving parts, test before relying)

use std::num::NonZeroU32;

use llama_cpp_2::context::params::{KvCacheType, LlamaContextParams};
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;

pub struct SessionParams {
    pub n_ctx: u32,
    pub n_batch: u32,        // prompt chunk size; 512–2048 is plenty, does NOT need to equal n_ctx
    pub n_ubatch: u32,       // 512 default; bigger helps prefill on Ultra-class GPUs, costs memory
    pub kv_q8: bool,         // q8_0 K/V halves KV memory; small speed win at long ctx, needs flash attn
    pub n_gpu_layers: i32,   // -1 = all
    pub n_threads: i32,      // CPU threads for anything not offloaded; perf-core count is right
}

impl Default for SessionParams {
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(8);
        Self { n_ctx: 32768, n_batch: 2048, n_ubatch: 512, kv_q8: false, n_gpu_layers: -1, n_threads: (cores * 2 / 3).max(4) }
    }
}

pub fn load_model(backend: &LlamaBackend, path: &str, p: &SessionParams) -> Result<LlamaModel, String> {
    // -1 cast `as u32` is u32::MAX; llama.cpp reads it back as i32 -1 which currently means "all", but
    // don't rely on the wraparound: name it.
    let ngl: u32 = if p.n_gpu_layers < 0 { 999 } else { p.n_gpu_layers as u32 };
    let mp = LlamaModelParams::default().with_n_gpu_layers(ngl).with_use_mmap(true);
    LlamaModel::load_from_file(backend, path, &mp).map_err(|e| format!("load '{path}': {e}"))
}

/// One live context + the token history it holds. Lives on the worker thread, next to the model.
pub struct Session<'m> {
    ctx: LlamaContext<'m>,
    model: &'m LlamaModel,
    history: Vec<LlamaToken>, // tokens currently materialised in the KV cache, positions 0..len
    n_batch: usize,
}

impl<'m> Session<'m> {
    pub fn new(backend: &LlamaBackend, model: &'m LlamaModel, p: &SessionParams) -> Result<Self, String> {
        let mut cp = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(p.n_ctx))
            .with_n_batch(p.n_batch)
            .with_n_ubatch(p.n_ubatch)
            .with_n_threads(p.n_threads)
            .with_n_threads_batch(p.n_threads)
            // explicit rather than AUTO so a driver that reports "unsupported" shows up as an error, not a slowdown
            .with_flash_attention_policy(llama_cpp_sys_2::llama_flash_attn_type_LLAMA_FLASH_ATTN_TYPE_ENABLED);
        if p.kv_q8 {
            cp = cp.with_type_k(KvCacheType::Q8_0).with_type_v(KvCacheType::Q8_0);
        }
        let ctx = model.new_context(backend, cp).map_err(|e| format!("context: {e}"))?;
        Ok(Self { ctx, model, history: Vec::new(), n_batch: p.n_batch as usize })
    }

    /// Prefill only what changed: keep the longest common prefix with the previous turn in the KV cache,
    /// drop the tail, decode the new suffix in n_batch chunks. Same contract llama-server's prompt cache has.
    fn prefill(&mut self, tokens: &[LlamaToken]) -> Result<(), String> {
        let common = self.history.iter().zip(tokens).take_while(|(a, b)| a == b).count();
        // must leave at least one token to decode so there are logits to sample from
        let common = common.min(tokens.len().saturating_sub(1));
        if common < self.history.len() {
            self.ctx.clear_kv_cache_seq(Some(0), Some(common as u32), None).map_err(|e| format!("kv rm: {e:?}"))?;
        }
        self.history.truncate(common);
        let mut batch = LlamaBatch::new(self.n_batch, 1);
        let mut pos = common;
        while pos < tokens.len() {
            let end = (pos + self.n_batch).min(tokens.len());
            batch.clear();
            for (i, t) in tokens[pos..end].iter().enumerate() {
                let p = pos + i;
                batch.add(*t, p as i32, &[0], p == tokens.len() - 1).map_err(|e| format!("batch add: {e}"))?;
            }
            self.ctx.decode(&mut batch).map_err(|e| format!("prefill decode: {e}"))?;
            pos = end;
        }
        self.history.clear();
        self.history.extend_from_slice(tokens);
        Ok(())
    }

    pub fn generate_stream(&mut self, prompt: &str, max_tokens: usize, temperature: f32, mut on_token: impl FnMut(&str)) -> Result<usize, String> {
        let tokens = self.model.str_to_token(prompt, AddBos::Always).map_err(|e| format!("tokenize: {e}"))?;
        if tokens.is_empty() { return Err("empty prompt".into()); }
        let n_ctx = self.ctx.n_ctx() as usize;
        if tokens.len() + max_tokens > n_ctx {
            return Err(format!("prompt {} + reply {} > n_ctx {}: raise n_ctx or trim on the caller side", tokens.len(), max_tokens, n_ctx));
        }
        self.prefill(&tokens)?;

        let mut sampler = if temperature <= 0.01 {
            LlamaSampler::greedy()
        } else {
            LlamaSampler::chain_simple([LlamaSampler::temp(temperature), LlamaSampler::dist(1234)])
        };
        let mut batch = LlamaBatch::new(1, 1);
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut n = 0usize;
        // logits for the last prompt token are at index (last batch).n_tokens()-1; after each decode of a
        // single-token batch that index is 0
        let mut logits_idx = -1i32; // -1 = "last token of the previous batch" in this binding's sample()
        while n < max_tokens {
            let tok = sampler.sample(&self.ctx, logits_idx);
            sampler.accept(tok);
            if self.model.is_eog_token(tok) { break; }
            let piece = self.model.token_to_piece(tok, &mut decoder, false, None).unwrap_or_default();
            on_token(&piece);
            batch.clear();
            let pos = self.history.len();
            batch.add(tok, pos as i32, &[0], true).map_err(|e| format!("batch add (gen): {e}"))?;
            self.ctx.decode(&mut batch).map_err(|e| format!("gen decode: {e}"))?;
            self.history.push(tok);   // generated tokens are in the cache too: next turn's prefix includes them
            logits_idx = 0;
            n += 1;
        }
        Ok(n)
    }
}

// Worker-thread shape (backend.rs): the thread owns backend + model + session and loops on requests.
//
//   let backend = LlamaBackend::init()?;
//   let model = load_model(&backend, &path, &params)?;
//   let mut session = Session::new(&backend, &model, &params)?;
//   for req in rx { session.generate_stream(&req.prompt, req.max_tokens, req.temperature, |p| tx.send(p)); }
//
// One session = one conversation. For N concurrent chats, N sessions on N sequences (with_n_seq_max) or
// N threads with their own context; the model is shared by reference, weights load once.
