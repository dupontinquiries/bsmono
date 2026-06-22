# Helix startup profiling (v0.1.650)

## What I could do in-sandbox (and the honest limits)
The uploaded binary is aarch64; the sandbox is x86_64. I installed
qemu-user-static + the aarch64 glibc/libgcc cross-libs and RAN the real
helix 25.07.1 binary under emulation. So I can exercise the actual startup
CODE PATH and count operations — but EMULATED WALL-TIME IS MEANINGLESS as
latency (qemu overhead dominates and doesn't model the M2's caches/IO).
Real millisecond numbers MUST come from your machine. Counts/structure
below are real; times are not.

## What startup actually does (measured via --health + -strace)
- helix --health enumerates EVERY supported language (hundreds), checking
  per-language: grammar present?, highlight query?, textobject query?,
  indent query?, language server?, formatter? That scan is real startup
  work.
- Runtime scan surface in the shipped 25.07.1 bundle:
    245 compiled grammar .so files
    792 query files (highlights/textobjects/indents/injections)
    1228 runtime files total
- `hx --version` short-circuits (only ~7 file opens) — so use a real file
  open, not --version, to measure true startup.

## The decomposition that matters (what "slow startup" usually is)
Opening a file makes helix:
  1. parse config.toml + languages.toml,
  2. detect the file's language,
  3. dlopen the matching grammar .so (245 exist; only the needed one loads),
  4. load + compile that language's tree-sitter QUERIES (highlights etc.),
  5. maybe auto-spawn the LSP for that language (often the REAL cost, and
     misattributed to "helix startup").
Cold (first launch, files paged off disk — notably on Asahi/aarch64) vs
warm differs a lot. LSP spawn is frequently the dominant term.

## Local harness — run on the M2 for REAL numbers (see scripts/bench-helix.sh)
It isolates the components:
  - cold vs warm (drop caches not possible without root; approximated by
    first-vs-rest + a unique temp file each cold trial),
  - empty buffer (`hx /tmp/empty` then quit) = editor+config+theme only,
  - real file of your language = adds grammar + queries (+ LSP),
  - LSP-disabled control (a languages.toml override) to subtract the LSP
    term,
  - uses hyperfine if present (best), else a bash time loop.
Paste the output back and I can act on the part that's ours (the grammar-
flip / editor-detect timing in web2qml layers on top of helix's own).

## The part that's OURS (measurable here, native) — and it's NOT compute
web2qml's "editor detected → flip voice grammar" path:
  - editor_voice::detect_editor() is a trivial string match (hx/helix/vim
    → Editor) — microseconds, NOT a latency source.
  - So our-side lag is the POLLING CADENCE: how often we check the
    foreground process to notice helix appeared. That's a poll-interval
    tuning question, not a compute one. If "helix feels slow to become
    voice-aware" is part of the complaint, the lever is the detect poll
    interval (and whether we react to the PTY foreground-change signal
    rather than poll). Find the cadence in the foreground-cmd poll path
    and consider event-driven detection.
Net: helix's OWN startup (grammar/query/LSP) must be measured on the M2
(scripts/bench-helix.sh); our detection add-on is a poll-cadence lever, not
a compute cost.

## v0.1.651 — REAL numbers (x86_64 helix ran natively) + a harness bug I found
The user uploaded the x86_64 build — it runs natively here. Real numbers
(sandbox x86 CPU, NOT the M2, but the PROPORTIONS transfer):
  hx --version       ~4.7 ms   (binary load + core init)
  hx --health rust   ~15.9 ms  (+ one language's grammar + queries)
  hx --health (all)  ~31.8 ms  (+ scanning all ~250 languages)
So helix's own non-LSP startup is TENS of ms, not seconds.

### Harness bug caught (would have corrupted the user's runs too)
My first bench-helix.sh timed buffer-open via `script(1)`. Measured: the
empty-buffer "startup" came out at ~2042 ms — but `script -qec true` ALONE
takes ~2015 ms. The 2s was `script` PTY teardown, NOT helix. Validating the
measurement method (timing `script true`) caught it. Rewrote the harness to
use clean-exit modes (--version/--health) + hyperfine; dropped the `script`
path entirely.

### Where the felt latency really is (hypothesis, confirm on M2)
Since helix core init is tens of ms, "slow startup" is almost certainly
(a) cold disk paging of the binary+grammars on first launch (Asahi NVMe),
and/or (b) LSP spawn on opening a real file. The harness isolates both. The
all-language scan (~32ms) is small but non-zero — not the main term.
