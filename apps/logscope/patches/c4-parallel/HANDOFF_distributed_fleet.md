# Handoff — distributed fleet + headless backend node

**Goal for this chat:** take the six delivered patches (a complete, unit-tested *protocol +
scheduler* layer for multi-machine fan-out) and **combine them into a running system**, then add a
**headless backend mode** so a machine with no GUI can join a fleet and serve work. Everything below
the protocol layer is unwritten on purpose — it's the real-IO/GPU glue that only a box can exercise.

The end state: one coordinator machine broadcasts a project; up to ten worker machines (each running
an ollama server *or* the logscope engine, **headless**) discover it, enlist, receive slices, run
them locally, and return results that reassemble into the finished artifact.

---

## 0. TL;DR — what to implement

1. **Headless node binary** `crates/logscope-egui/src/bin/logscope-node.rs` (built without the `gui`
   feature) with two modes:
   - **worker**: load a `Backend`, wrap in `localexec::BackendExecutor` (or `ollama::OllamaExecutor`),
     join via `enlist::Worker` over `UdpTransport`, then serve `dispatch::Worker` work.
   - **coordinator**: `enlist::Coordinator` → `bridge::tasks_*` → `dispatch::Dispatcher` (driven with a
     real `Instant` clock + `sweep`) → `bridge::assemble_results` → stdout.
2. **Real clock**: feed `Instant`-derived millis into `Dispatcher::assign_at` / `sweep`.
3. **Runner glue** (GUI, secondary): a `:enlist` / `:fanout` command in `runner.rs`.

All three are *integration* code. The protocol/scheduler beneath them is done and tested; do **not**
rewrite it — wire it.

---

## 1. Orientation (read first)

- **Repo:** `logscope-suite`. Two crates matter here: `crates/logscope-egui` (orchestration + app;
  this is where all six patches land) and `crates/gpu-gemm` (the wgpu inference engine, `gpu_kernels`).
- **Rules:** `RULESET.md` (K1–K10) is binding. Most relevant here: changes must be additive + tested;
  backend logic stays frontend-free; anything touching the engine is gated. The distributed code added
  no kernels, so K1–K3 (bit-exact decode, AGX-safe dequant, `wg1()` dispatch) don't bite — but if you
  touch `gpu-gemm`, they do. Read `FIELD_NOTES.md` for merge history and `TODO.md` for the live plan.
- **AI-git:** history lives in `ai-git/` (inspect with `git --git-dir=ai-git log`), not `.git/`.
- **Compilation is now possible (and was used).** The whole lean lib was compiled and tested with the
  bundled **Rust 1.96** offline toolchain. To reproduce in a sandbox: install it from `rustup.zip`
  (`unzip` → `rust-*/INSTALL.sh` → installs `rustc`/`cargo` to `/usr/local/bin`), then the repo's
  `.cargo/config.toml` forces `-fuse-ld=mold` (not present) so **override it**:
  `export RUSTFLAGS="-C prefer-dynamic"` (this replaces `build.rustflags`, letting `cc` use `/usr/bin/ld`).
  Then `cargo test -p logscope-egui --lib --no-default-features` runs the full lean suite — **117 tests,
  all green** as shipped. cargo fetches deps from crates.io (allowed). The GUI build (`--features gui`)
  pulls eframe + system libs and may still be blocked; the GPU path needs hardware.
- **Also use `rustcheck`** (in the bundle) for fast structure checks without a full build:
  `rustcheck --fast FILE.rs` is Tier-0 Rust-aware static checking (delimiter balance ignoring
  strings/comments, unterminated literals, `mod foo;` missing-backing-file). `rustcheck --changed` wraps
  real `cargo check`. Use rustcheck for a quick pass, `cargo test` for the truth.
- **Discipline that produced these patches:** every layer is split into a *pure, unit-tested core* and a
  *thin IO/GPU seam*. Keep that. The seams (sockets, HTTP, GPU decode, clock) are integration-tested on
  hardware; everything else has socket-free unit tests that **pass under `cargo test` today**.

---

## 2. What's already delivered (apply in this order)

All patches are `git apply`-able at the repo root and target `crates/logscope-egui/src`. Each adds an
always-on module (no gui dep) registered in `lib.rs`, with unit tests run by
`cargo test -p logscope-egui --lib <module>`.

| # | patch | module | what it is | tests |
|---|-------|--------|-----------|-------|
| 1 | `devloop-orchestrator.patch` | `devloop.rs` (+lib) | long-horizon decompose→fan→verify→assemble loop; `run_mounted` IO shell (mount→isolate→run→bundle); `WorkspaceVerifier` | 6 |
| 2 | `enlist-protocol.patch` | `enlist.rs` (+lib) | the join protocol: `Coordinator`/`Worker` state machines, `select_invitees` (cap `MAX_FLEET=10`), wire `Msg` enum + `encode`/`decode` codec, `ct_eq`/`frame`/`unframe` token auth, `Transport` trait, `UdpTransport`, driving glue | 9 |
| 3 | `dispatch.patch` | `dispatch.rs` (+lib) | the fan-out scheduler: `Task`, `Executor` trait, `Dispatcher` (capacity-aware assign, result/fail/drop, lease `sweep`), member-side `Worker` | 6 |
| 4 | `ollama-executor.patch` | `ollama.rs` (+lib) | `OllamaExecutor: Executor` — POST `/api/generate`; pure `build_request`/`json_escape`/`parse_response`, socket seam `http_roundtrip` | 5 |
| 5 | `bridge.patch` | `bridge.rs` (+lib) | project ↔ `Task` transforms: `tasks_from_skeleton`/`assemble_results`, `tasks_fanout`/`collect_fanout`; end-to-end loopback test | 5 |
| 6 | `localexec.patch` | `localexec.rs` (+lib) | `BackendExecutor<B: Backend>: Executor` — runs a slice on a local engine `Backend` (wgpu or llama.cpp) | 3 |
| 7 | (in the consolidated patch) | `speculate.rs` (+lib) | **aggressive-coordinator anticipation**: ruleset → background investigation `Lane`s (clarify/decompose/precheck/anticipate) as small-model prompts → `LeadCache` with relevance `match_leads` | 5 |

| 8 | (in the consolidated patch) | `voicefan.rs` (+lib) | **speaker-stack → large tree fanout**: speaker-attributed `Utterance`s → per-speaker investigation branches via `speculate`, round-robin-fair under a `FanBudget`, tagged for `fold` into a per-speaker tree | 5 |

| 9 | (in the consolidated patch) | `voice_ingest.rs` (+lib) | **whisper passes → utterances**: `Transcriber` coalesces full re-transcription passes, finalizes on speaker-change/endpoint, conservative claim-strip (`<name> here…`), `fan_budget` sizing | 6 |

| 10 | (in the consolidated patch) | `reaction.rs` (+lib) | **steer/stop a generation from live speech**: `classify` (interrupt/steer/add-context/affirm/ignore) + debounced `Reactor` raising a `CancelFlag` the `Backend::generate` cancel hook reads | 7 |

| 11 | (in the consolidated patch) | `explain.rs` (+lib) | **code-grounded explanation planner** — the keystone: question + `CodeRef`s → facet explanation lanes (overview/walkthrough/edge-cases/deps/follow-ups) → fleet → `assemble` into a conversational `Explanation` tree | 5 |
| 12 | (in the consolidated patch) | `ctxplan.rs` (+lib) | **chat-backend context assembly** — over a `/tmp` project space, decide per file: loop in **whole**, **peek** only the relevant sections, or **skip**, under a token budget; emits the exact byte-ranges the IO layer should read | 6 |

> **All of the above ships as one consolidated patch** (`HANDOFF_distributed_fleet.patch`) generated via
> `git diff`, so apply it with **`git apply`** (not `patch` — the new-file hunks need git's `new file
> mode` headers). It also carries a correctness fix the toolchain caught: `devloop::pick_winner` used
> `max_by_key`, which returns the *last* max on ties, so two equal-rank candidates picked the later one
> instead of the earliest — fixed with an index tie-break. The full lean suite is **117 green**.

>**One-apply option:** `HANDOFF_consolidated.patch` is a single `git apply` covering **both** crates —
all 13 `logscope-egui` modules *and* all 12 `logscope` engine modules (25 files). It reproduces the tested
tree byte-for-byte (**146** logscope-egui + **67** logscope tests green). The two per-track patches
(`HANDOFF_distributed_fleet.patch`, `HANDOFF_engine_perf.patch`) remain if you'd rather apply one track at a
time.

## How it adds up to a conversational codebase explainer

The point of the stack is one experience: **ask about a codebase and get a conversational, branching
explanation that already anticipated your next question.** The pieces compose left-to-right:

1. **Ground** — `retrieval` + `ast` (gui) turn a question into `explain::CodeRef`s (file/symbol/snippet).
2. **Plan** — `explain::explanation_lanes` makes code-anchored facet prompts (what it does → how it works →
   edge cases → what it touches → what you'll ask next), breadth-first so a tight budget still explains the
   whole picture before drilling in.
3. **Fan out** — the lanes become `dispatch::Task`s run across the `enlist`/`dispatch` fleet on cheap
   models (`ollama`/`localexec`), so a large explanation is computed in parallel.
4. **Anticipate** — `speculate` + the `Followups` facet pre-warm the *likely next questions* on spare
   capacity, so the answer to "and how are errors handled?" is often already in `LeadCache` when asked.
5. **Talk to it** — `voice_ingest` + `voicefan` let several people *ask by voice*, fanning each speaker's
   question into its own explanation branch; `reaction` lets you **steer or stop** an explanation mid-stream
   ("no, walk through the error path instead").
6. **Assemble** — `explain::assemble` folds the fleet's answers into an `Explanation` (lead + ordered
   sections + follow-ups) that the GUI renders as `convograph` nodes — a *tree* of explanation you can
   expand, exactly what the fanout produced.

So the distributed/voice/speculation work isn't incidental: it's what lets the explanation be **large,
parallel, spoken, steerable, and pre-thought** instead of a single blocking prompt. `explain.rs` is the
seam that makes all of it about *code*.

## Engine track — tokens/sec + multimodal (separate patch: `HANDOFF_engine_perf.patch`)

These two are **engine** concerns, not orchestration, so they live in the lower `crates/logscope` crate
(GPU-free, both the app and the wgpu engine depend on it; `gpu-gemm` itself is workspace-excluded and needs
hardware). Apply `HANDOFF_engine_perf.patch` with `git apply`; both modules are pure and `cargo test
-p logscope --lib` runs them (note one *pre-existing, unrelated* failure, `shorten::test_drm_example`, that
is not part of this work — filter with `-- specdecode multimodal`).

| module | what it adds | seam | tests |
|---|---|---|---|
| `logscope::specdecode` | **tokens/sec via speculative decoding** — `verify_greedy` (greedy, lossless) **and `verify_sampled`** (the Leviathan/Chen accept-with-prob + residual-resample rule, lossless at temperature>0), `SpecStats` (acceptance rate, tokens/forward-pass), `DraftPolicy`/`SpecDecoder` (adaptive `k`) | the draft + batched-target **forward passes** (the wgpu engine runs them) | 10 |
| `logscope::multimodal` | **vision-language input** — tiling geometry (`tile_grid`/`image_tokens`), `assemble` (image markers → vision `Slot`s interleaved with text), **and `segment_prompt`/`pieces_from_prompt`** (parse a real chat string's `<image>`/`<image:N>` markers into the `Piece` stream; tokenizer injected) | the **vision-encoder** forward + pixel resampling; the text **tokenizer** | 10 |
| `logscope::batching` | **tokens/sec via continuous batching** — `Scheduler` admits requests into a fixed KV-slot pool and frees+refills each slot the moment a sequence completes, keeping the decoded batch full (utilization→1) | the **batched decode** forward over `active()` each step | 4 |
| `logscope::kvcache` | **paged KV cache** — `Paged` hands out fixed-size blocks via per-sequence block tables (grow on boundary cross), with ref-counted `fork` prefix-sharing + copy-on-write; no `max_len` reservation, no external fragmentation ⇒ bigger batches | the **K/V tensor store** + paged-attention kernel (this says which blocks / when to copy) | 7 |
| `logscope::arch` | **more model support (Qwen / DeepSeek / …)** — `detect` reads GGUF hparams → `ArchInfo` (GQA / MoE / DeepSeek MLA / Qwen3 QK-norm / RoPE scaling); `support` gives an honest `Full`/`Partial`/`Unsupported` verdict vs. `EngineCaps` | the GGUF **metadata read** (fills `Hparams`) + the per-feature **kernels** | 7 |
| `logscope::gemmplan` | **GPU dispatch speedups** — output-tile grids, the **>65535-workgroup fold** into 2-D/3-D (RULESET K3, where a wrong grid silently drops work), and split-K for skinny matmuls | the GEMM/attention **kernels** themselves | 8 |
| `logscope::dedup` | **cross-model compression in `~/models`** — content-defined chunking (Gear rolling hash) + a content-addressed `Store`; quants/finetunes that share weight regions store the shared chunks once | model-file **I/O** (read bytes, write the chunk store) | 5 |
| `logscope::moe` | **mixture-of-experts routing** (unlocks Mixtral/DeepSeek/Qwen-MoE) — `route_token` top-k + softmax/sigmoid gate weights, `plan` groups tokens per expert for the grouped GEMM, `combine_scalar` does the gate-weighted merge | the per-expert **GEMM** kernels | 5 |
| `logscope::mla` | **DeepSeek latent attention** — `MlaConfig` gives the cached-latent footprint (`cache_compression` ~70× vs MHA, ties to `kvcache`) and the projection GEMM shapes (`projections`, feed to `gemmplan`) | the attention / up-projection **kernels** | 4 |
| `logscope::rope` | **metadata RoPE scaling** — synthesizes the per-dim `inv_freq` factors + YaRN `mscale` from `rope_scaling` metadata (linear/YaRN), so models that declare long context in metadata (Qwen2-YaRN, DeepSeek, Yi) work with the engine's existing baked-factor apply path | the rotary **kernel** | 4 |
| `logscope::quantfmt` | **GGUF quant sizing** — block layout / byte footprint / bits-per-weight for every ggml type (incl. ones not yet decodable), for memory planning + mmap offsets + load validation | n/a (pure facts; pairs with the engine's existing **decoder**) | 4 |

How they wire into the existing `Backend`:
- **specdecode** sits inside a wgpu-engine `generate`: ask `SpecDecoder::draft_len()`, run the draft model
  for that many tokens, run ONE batched target forward over the prefix+draft, call `commit(draft,
  target_argmax)` to get the committed tokens and an adapted `k`. The existing batched-decode override on
  `Backend::generate_batched` is the same GPU capability this leans on. `verify_greedy` handles greedy
  decoding; `verify_sampled` handles temperature>0 (it needs the per-position target/draft *distributions*,
  which the engine already has pre-argmax).
- **batching** drives the multi-request loop: `submit` queued requests, call `admit()` each step to fill
  free KV slots (prefill the returned ids), decode `active()` in one batched forward, and `complete(id)` on
  EOS/cap so a waiting request takes the slot next step. It composes with `specdecode` (each active slot can
  itself be speculatively decoded) and with the fleet (`dispatch`) above it.
- **kvcache** sits under `batching`: gate admission on `Paged::can_admit`, `admit` the prompt, `append` per
  decode step, `free` on completion. `fork` shares the prompt prefix across the many `explain`/`voicefan`
  fanout lanes (one prompt, many branches) so they cost one copy until they diverge — copy-on-write makes
  that safe. The block ids it hands out drive the paged-attention kernel.

**On the three follow-ups asked here:**
- *More model types (Qwen, DeepSeek).* `arch` classifies them and reports exactly what each needs: Qwen2/3
  are GQA + QKV-bias (runnable; Qwen3's QK-norm is a `Partial` until that small kernel lands), while
  DeepSeek-V2/V3 are MLA + large MoE — `Unsupported` until those two kernels exist, and the verdict says so
  rather than failing opaquely at load. (Before adding engine cores I checked what already exists: the **dequantizer**, the **sampler**, and **baked llama-3 RoPE scaling** are already in gpu-gemm and validated under K1/K2 — so I did not rebuild them. `rope` fills the genuine gap next to them: computing YaRN/linear factors from *metadata* when no `rope_freqs.weight` tensor is baked in; `quantfmt` is the *sizing* table that complements, not duplicates, the byte-level decoder.) Flip an `EngineCaps` flag as each kernel ships and support upgrades. The `moe` and `mla` modules are the **pure planning layer beneath those two kernels**: `moe` does the routing/dispatch/combine bookkeeping a MoE FFN needs, and `mla` gives the latent-cache dims (→ `kvcache`) and projection GEMM shapes (→ `gemmplan`) — so once the grouped-GEMM and latent-attention kernels land, the `EngineCaps.moe`/`.mla` flags flip and DeepSeek/Mixtral go from `Unsupported` to runnable.
- *GPU speedups for llama.* The honest big wins are kernels (the seam); the pure lever here is `gemmplan` —
  correct, fully-covering dispatch grids (the K3 fold is the one that silently drops work if wrong) and
  split-K to fill the GPU on skinny matmuls. The `fold_is_a_bijection_up_to_count` test proves no workgroup
  id is dropped or duplicated.
- *Cross-model compression.* `dedup` is content-defined so a one-byte change re-syncs locally instead of
  invalidating the whole file, which is what lets quants/finetunes of one base share storage. Pair it with a
  cryptographic chunk digest (BLAKE3) for a real on-disk store.
- **multimodal** runs at prompt-assembly time: tokenize text to `Piece::Text`, mark images as
  `Piece::Image(i)`, compute each image's `image_tokens(...)`, then `assemble(...)` to get the slot layout;
  the engine fills text slots from the embedding table and image slots from the vision encoder's output.
  `logscope` already carries `base64` for the multimodal Ollama path; this is the local-engine counterpart.

> Note: `enlist-protocol.patch` was revised across sessions (transport + auth folded in); use the latest
> single file. The `lib.rs` hunks are layered so the six apply cleanly in the table's order.

After applying all six: `rustcheck --fast crates/logscope-egui/src/*.rs` should be clean, and on a box
`cargo test -p logscope-egui --lib` should pass the new modules' tests (≈62 across the 12 modules) within the 140-test lean lib suite — verified green with the bundled toolchain.

---

## 3. Module API reference (what you're combining)

Exact signatures, so you can wire without rediscovering. All in `logscope_egui::`.

### `enlist`
```rust
pub const MAX_FLEET: usize = 10;
pub type PeerId = String;                          // "ip:port" of the node's UDP endpoint
pub struct Capability { pub kind: String, pub models: Vec<String>, pub slots: u32 }
pub enum Msg { Opening{..}, Available{..}, Invite{..}, Accept{..}, Decline{..},
               Assign{coordinator,peer,task:u64,payload:String}, Result{peer,task:u64,output:String}, Failed{peer,task:u64,reason} }

pub trait Transport { fn broadcast(&mut self,&Msg); fn send(&mut self,&PeerId,&Msg); fn recv(&mut self)->Vec<(PeerId,Msg)>; }
pub struct UdpTransport;                            // ::bind(addr,bcast) -> io::Result<Self>; .with_token(&str)
pub fn encode(&Msg)->String;  pub fn decode(&str)->Option<Msg>;
pub fn ct_eq(&[u8],&[u8])->bool; pub fn frame(token,&str)->String; pub fn unframe(token,&str)->Option<String>;

pub struct Coordinator;  // ::new(project,id,want,nonce)
  //  .opening()->Msg ; .open(&mut T) ; .on_available(&PeerId,&Capability,nonce) ; .pump(&mut T)
  //  .invite()->Vec<Msg> ; .close_and_invite(&mut T) ; .on_accept(&PeerId) ; .on_decline(&PeerId,&str)->Option<Msg>
  //  .phase()->Phase{Announcing,Gathering,Inviting,Ready} ; .members()->&[PeerId] ; .is_full()->bool
pub struct Worker;       // ::new(id,Capability)
  //  .on_opening(project,nonce)->Option<Msg> ; .on_invite(project,nonce)->Msg ; .serve(&mut T) ; .is_joined()->bool
```

### `dispatch`
```rust
pub struct Task { pub id: u64, pub payload: String }
pub trait Executor { fn run(&mut self, task:u64, payload:&str) -> Result<String,String>; }

pub struct Dispatcher;   // ::new(coordinator, members:&[(PeerId,u32 slots)], tasks:Vec<Task>, max_retries:u32)
  //  .assign_at(now:u64)->Vec<Msg> / .assign() ; .dispatch_at(&mut T,now) / .dispatch(&mut T)
  //  .pump(&mut T)  (drains Result/Failed) ; .on_result/.on_failed/.drop_member
  //  .sweep(now:u64, lease:u64)->usize  (reclaim unanswered assigns) ; .is_complete()->bool
  //  .results()->&HashMap<u64,String> ; .abandoned()->&[u64]
pub struct Worker;       // ::new(id) ; .work<T,E>(&mut T,&mut E)  (drains Assign→Executor→Result/Failed)
```

### `bridge`
```rust
pub fn tasks_from_skeleton(skeleton:&str)->(Vec<Task>, Vec<decompose::Hole>);
pub fn assemble_results(skeleton:&str, holes:&[Hole], results:&HashMap<u64,String>)->Option<String>;
pub fn tasks_fanout(prompt:&str, k:usize)->Vec<Task>;
pub fn collect_fanout(k:usize, results:&HashMap<u64,String>)->Vec<String>;
```

### executors
```rust
ollama::OllamaExecutor::new(host:&str /*"ip:port"*/, model:&str, num_predict:u32)  // .with_timeout(Duration)
localexec::BackendExecutor::new(backend:B /*impl backend::Backend*/, params:GenParams)
```

### the engine `Backend` (what `BackendExecutor` wraps) — `logscope_egui::backend`
```rust
pub trait Backend {  // NOT Send — must be owned by one thread
    fn name(&self)->&'static str; fn loaded(&self)->bool;
    fn load(&mut self, path:&str, &GenParams)->Result<(),String>;
    fn generate(&mut self, prompt:&str, &GenParams, cancel:&dyn Fn()->bool, on_token:&mut dyn FnMut(&str)->bool)->Result<usize,String>;
    fn generate_batched(&mut self, &[String], &GenParams, &dyn Fn()->bool)->Vec<String>;  // wgpu overrides: batched decode
    // continue_last, template have defaults
}
pub enum BackendKind { LlamaCpp, Wgpu }   // ::preferred()
pub fn make(kind: BackendKind) -> Box<dyn Backend>;   // factory; Wgpu yields the real engine under --features wgpu-engine
// GenParams: logscope::llm_backend::GenParams (impl Default) — max_tokens, n_ctx, temperature, …
```

---

## 4. The integration to implement

### 4.1 Headless backend node (PRIMARY DELIVERABLE)

Add `crates/logscope-egui/src/bin/logscope-node.rs`, modeled on the existing
`src/bin/logscope-cli.rs` (same crate; that bin shows the `MockBackend`, `--model GGUF`, and
stdout/stderr output discipline — reuse the `MockBackend` for off-GPU testing of the loop).

**Build:** headless, so **no `gui` feature**. The default features are `["gui","gpu-scene"]`, so a node
build is `cargo build -p logscope-egui --bin logscope-node --no-default-features --features wgpu-engine`
(or `--features llamacpp` if that's how the llama.cpp backend is gated — check `Cargo.toml`). Add a
`[[bin]]` entry for `logscope-node` with `required-features` matching, mirroring `logscope-cli`.

**Why a new bin and not a flag on logscope-cli:** the cli drives a *script* through the runner; the node
runs the *network loops*. Different lifetimes. Keep them separate; share `MockBackend` via a small module
if convenient.

#### Worker mode
One thread owns the (`!Send`) `Backend` and the `UdpTransport`. A single-slot worker is the simplest
correct design (advertise `slots: 1`); `generate` blocks the loop while it runs, which is fine when the
node serves one slice at a time. Multi-slot needs concurrency — see Gotchas.

```rust
// pseudo-Rust; adapt names to the real signatures in §3
fn run_worker(model_path: &str, kind: BackendKind, bind: &str, bcast: &str, token: &str) -> Result<(),String> {
    let params = GenParams::default();               // tune: reply_ceiling, temperature, n_ctx…
    let mut be = backend::make(kind);
    be.load(model_path, &params)?;
    let id = local_endpoint(bind);                   // the PeerId other nodes reach us at, "ip:port"
    let cap = enlist::Capability { kind: kind_str(kind).into(),
                                   models: vec![model_basename(model_path)], slots: 1 };
    let mut w_join = enlist::Worker::new(&id, cap);
    let mut w_work = dispatch::Worker::new(&id);
    let mut exec   = localexec::BackendExecutor::new(*be, params);   // or OllamaExecutor for an ollama box
    let mut t = enlist::UdpTransport::bind(bind, bcast)?.with_token(token);

    loop {
        w_join.serve(&mut t);     // hears Opening → broadcasts Available; hears Invite → Accept/Decline
        w_work.work(&mut t, &mut exec);  // hears Assign → runs Executor (blocking) → Result/Failed
        std::thread::sleep(std::time::Duration::from_millis(20));   // poll interval
    }
}
```
Notes:
- `enlist::Worker` and `dispatch::Worker` are *separate* state machines sharing one transport. Both drain
  `t.recv()` each tick; each ignores messages not meant for it. That's by design — fine on one bus.
- `BackendExecutor::run` doesn't stream to the network; it collects the full reply and the `dispatch`
  `Worker` sends one `Result`. (Streaming partial tokens over the wire is a later refinement.)
- For an **ollama** node, swap `exec` for `ollama::OllamaExecutor::new("127.0.0.1:11434", model, n)`. No
  `Backend`/GPU needed — that's the point of having both executors behind one trait.

#### Coordinator mode
```rust
fn run_coordinator(skeleton_path: &str, want: usize, bind: &str, bcast: &str, token: &str) -> Result<(),String> {
    let skeleton = std::fs::read_to_string(skeleton_path).map_err(|e| e.to_string())?;
    let (tasks, holes) = bridge::tasks_from_skeleton(&skeleton);   // or bridge::tasks_fanout(prompt, k)
    let id = local_endpoint(bind);
    let mut t = enlist::UdpTransport::bind(bind, bcast)?.with_token(token);

    // 1) enlist up to `want` machines
    let mut coord = enlist::Coordinator::new("proj", &id, want, fresh_nonce());
    coord.open(&mut t);
    let start = std::time::Instant::now();
    while coord.phase() != enlist::Phase::Ready {
        coord.pump(&mut t);
        if coord.phase() == enlist::Phase::Gathering && start.elapsed() > GATHER_WINDOW {
            coord.close_and_invite(&mut t);          // stop gathering, invite the ranked top `want`
        }
        if start.elapsed() > ENLIST_DEADLINE { break; }   // proceed with whoever joined
        std::thread::sleep(Duration::from_millis(20));
    }
    let members: Vec<(enlist::PeerId,u32)> =
        coord.members().iter().map(|m| (m.clone(), 1)).collect();   // slots: track from Capability if you keep it

    // 2) dispatch the tasks across members, with a REAL clock driving the lease sweep
    let mut disp = dispatch::Dispatcher::new(&id, &members, tasks, /*max_retries*/ 2);
    let t0 = std::time::Instant::now();
    let now = || t0.elapsed().as_millis() as u64;
    disp.dispatch_at(&mut t, now());
    while !disp.is_complete() {
        disp.pump(&mut t);                 // collect Result/Failed
        disp.sweep(now(), LEASE_MS);       // reclaim silently-lost assigns → requeue
        disp.dispatch_at(&mut t, now());   // backfill freed slots
        std::thread::sleep(Duration::from_millis(20));
        if now() > RUN_DEADLINE_MS { break; }
    }

    // 3) reassemble and emit (stdout = artifact, stderr = progress — match logscope-cli discipline)
    match bridge::assemble_results(&skeleton, &holes, disp.results()) {
        Some(artifact) => { print!("{artifact}"); Ok(()) }
        None => Err(format!("incomplete: {} task(s) abandoned", disp.abandoned().len())),
    }
}
```

**CLI surface** (suggested): `logscope-node worker --model GGUF [--ollama HOST] --bind 0.0.0.0:48710
--broadcast 255.255.255.255:48710 --token KEY` and `logscope-node coordinate --skeleton FILE --want N
--bind … --broadcast … --token KEY`. Add `--mock` (worker) to use `MockBackend` so the *loop* can be
tested off-GPU. Generate the token with `gpu_kernels::netserve::generate_key` (or paste a shared one).

### 4.2 Real clock
The scheduler is time-injected precisely so it stays testable: pass monotonic millis
(`Instant::now().elapsed()`) into `assign_at`/`sweep`. Pick `LEASE_MS` from observed slice latency
(a long generation must not be swept mid-run — set `LEASE_MS` > worst-case `generate` time, or have the
worker send periodic keepalives — see Gotchas). The `assign()/dispatch()` zero-time wrappers exist only
for tests; the node uses the `*_at` forms.

### 4.3 Runner `:enlist` / `:fanout` glue (GUI, secondary)
In `runner.rs`, add `Command::Enlist`/`Command::Fanout` variants (parse + dispatch arm, like the existing
`Fill`/`FuzzyAll`). The handler runs the §4.1 coordinator flow against the app's `UdpTransport`, then
folds the assembled artifact (or `collect_fanout` candidates) back into the conversation graph as new
nodes — the same way `:fill`/`:fa` insert results today. Gui-gated; sockets + graph writes are
integration-tested.

### 4.4 Wire `speculate` into the aggressive coordinator
`speculate.rs` is built and tested; what remains is connecting it. In the coordinator (or the app's idle
loop), when `SpecConfig::aggressive` is on: call `investigation_lanes(context, &cfg)` on the in-progress
context, `tasks_from_lanes(&lanes)` → feed to a **separate** `dispatch::Dispatcher` seeded with
**cheap-model** members (e.g. ollama members loaded with a small model), run it in the background on spare
capacity, then `LeadCache::absorb(&lanes, disp.results())`. When the user's real request lands, call
`LeadCache::match_leads(request, 1)` and surface the top leads as context/notes for the main run. Keep the
speculative dispatcher distinct from the main work dispatcher so background lanes never starve the real
request (give them lower-priority/idle members, or cap their slot usage).

### 4.5 whisper.cpp + speaker stack → live tree fanout
The **fanout core is built and tested** (`voicefan.rs`): speaker-attributed `Utterance`s → per-speaker
investigation branches via `speculate`, interleaved round-robin under a `FanBudget` (fair share, no single
talker hogs it — the same balanced-fan-in idea as voiceterm's `mic_scheduler`), each branch a
`dispatch::Task` tagged with its speaker, and `fold` regroups the fleet's outputs into per-speaker tree
nodes. The end-to-end test runs a two-speaker batch through a real `Dispatcher` and folds the results.
What remains are the seams around it:
- **The whisper + speaker-stack adapter.** The *pure* half is now built (`voice_ingest.rs`): a
  `Transcriber` that coalesces whisper's full-re-transcription `Pass`es into finalized
  `voicefan::Utterance`s (flush on speaker-change / endpoint / silence), conservative claim-stripping
  (`"<name> here …"`, `"my name is <name> …"`), and `fan_budget(spare_slots, n_speakers, per_speaker_cap)`
  to size the `FanBudget`. **What remains is the FFI seam:** drive `Transcriber::push` from real
  whisper-streaming passes tagged by `speakers::identify()` voiceprint attribution, sourced from mic
  capture — adapt voiceterm's `web2qml/src/{voice, speakers, audio_profile, speaker_claim, mic_scheduler}.rs`
  (a *separate crate* — port the minimal `identify → (name, conf)` path, don't depend on it wholesale).
  For names, prefer `speaker_claim::match_claim_to_registry` over the minimal `parse_claim` here.
- **The reaction classifier is built** (`reaction.rs`): `classify` buckets fragments into
  `Interrupt | Steer | AddContext | Affirm | Ignore`; a debounced `Reactor` (immediate on a stable
  endpoint, N agreeing partials otherwise) raises a `CancelFlag` that the `cancel: &dyn Fn()->bool` on
  `Backend::generate` reads, stopping generation mid-token. Remaining seam: feed it the *driving* speaker's
  live fragments (gate on `speakers::is_allowed`) and, on a committed `Steer`/`AddContext`, enqueue the
  redirected/added turn into the run loop.
- **Tree insertion (gui seam).** `fold`'s `SpeakerNode`s become conversation-graph branches under each
  speaker — same insertion path `:fa`/`:fill` use.

---

## 5. Testing & acceptance

**In a sandbox (no toolchain):** `rustcheck --fast` on every file you add/edit; expect `PASS 0 static`.
`rustcheck --fast crates/logscope-egui/src/*.rs` also re-verifies that every `mod`/`[[bin]]` has a
backing file.

**On the box (toolchain present):**
- `cargo test -p logscope-egui --lib` — the 34 protocol/scheduler tests must stay green.
- `cargo build -p logscope-egui --bin logscope-node --no-default-features --features wgpu-engine` — node
  compiles headless.
- **Single-box loopback:** run a coordinator and 1–2 `--mock` workers on `127.0.0.1` (different ports,
  broadcast `127.255.255.255:PORT` or a loopback-friendly addr); confirm enlist reaches `Ready` and a
  trivial 1–2-hole skeleton reassembles. No GPU needed — proves the wiring.
- **Real worker:** one `worker --model GGUF` on the engine box; confirm a slice round-trips with real
  tokens. Then a second machine (ollama) to prove heterogeneous members.

**Acceptance:** a coordinator on machine A and workers on B/C (one engine, one ollama) complete a
multi-hole skeleton end to end, the artifact is byte-identical to running the same decomposition
locally, and killing a worker mid-run causes its slice to be reassigned (the `sweep`/`drop` paths) rather
than stranding the run.

---

## 6. Gotchas & invariants (don't relearn these the hard way)

- **`Backend` is `!Send`.** Own it on the node's worker thread; never move it across threads. The
  existing `backend.rs` `spawn()`/`worker()` channel pattern (≈line 706) is how the GUI keeps it on one
  thread — reuse that pattern if you add concurrency.
- **Single-slot first.** A blocking `generate` can't pump the network. For `slots > 1`, run N backend
  instances on N threads (each its own `dispatch::Worker` sharing the bus via a `Send` transport handle)
  or use `generate_batched`. Until then advertise `slots: 1`.
- **Lease vs long generations.** `sweep` will reclaim an assign whose `Result` hasn't arrived within
  `LEASE_MS`. A 30-token decode can take many seconds. Either set `LEASE_MS` generously or add a
  lightweight keepalive (a periodic message that re-stamps the lease) — don't let a healthy slow worker
  get its task yanked.
- **UDP is best-effort.** A dropped `Result` is tolerated (re-dispatch after `sweep`); a dropped
  `Assign` is now tolerated too (sweep frees the slot). A dropped `Accept` during enlist just means that
  member won't appear in `members()` — proceed with whoever joined. An ack/retry around `Invite`/`Assign`
  is a worthwhile robustness upgrade but not required for a first cut.
- **Auth.** Always pass `--token`; `UdpTransport::with_token` frames+verifies every datagram (empty token
  = open, only for a fully trusted LAN). Same posture as `netserve.rs`: authenticates senders, does not
  encrypt — run on a trusted LAN or an SSH tunnel.
- **`MAX_FLEET = 10`** is enforced twice (constructor clamp + `select_invitees`); don't add an invite
  path that bypasses both.
- **Payload is opaque.** `dispatch`/`bridge` never parse `Task.payload`; the executor does. If you want
  per-task model selection, encode `{model, prompt}` in the payload and parse it in the executor (extend
  `OllamaExecutor`/`BackendExecutor`) — don't teach the scheduler about it.
- **Two `Verdict` enums** exist in the repo (`bestregion::Verdict` vs `sandbox::Verdict`) — unrelated to
  this work but a known footgun if you wander into `devloop`.
- **Gating:** the six modules are always-on (no gui dep) and must stay that way so `logscope-cli`/
  `logscope-node` build lean. Only the *real engine* (`wgpu-engine`) and *GUI* (`gui`) are feature-gated.
  `BackendExecutor` is generic over `Backend` precisely so it compiles without the engine (mock in tests).
- **rustcheck Tier-0 only catches structure.** It will not catch a type/borrow/trait error — those need
  `cargo check` on the box. Don't over-trust a green Tier-0; it means "not structurally broken," not
  "compiles."

---

## 7. Pointers
- Existing headless bin to model: `crates/logscope-egui/src/bin/logscope-cli.rs`.
- Backend trait + factory + worker pattern: `crates/logscope-egui/src/backend.rs`.
- LAN HTTP server + key generation (reference for tokens / an HTTP control plane): `crates/gpu-gemm/src/netserve.rs` (`serve`, `handle`, `generate_key`, `Keyring`, `ct_eq`).
- Single-box fleet (what runs *inside* one worker): `FLEET.md`, `gpu_kernels::run_fleet_trie`.
- The six modules to combine: `enlist.rs`, `dispatch.rs`, `bridge.rs`, `ollama.rs`, `localexec.rs` (+ `devloop.rs` for the local decompose loop the distributed path mirrors).
