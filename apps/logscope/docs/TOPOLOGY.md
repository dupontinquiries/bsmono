# Topology: Ultra (Asahi) · mini (macOS) · phone (hatch) · Claude

No second monitor, no KVM: the mini is headless and only ever receives directives; the phone only ever
receives one status line; Claude is driven as a binary. Everything the person touches is logscope-egui
on the Ultra.

```
                     ┌──────────────── Fairphone (postmarketOS / cosmic) ────────────────┐
                     │  tideshell · hatch: `mini · loaded Ornith-1.5-35B (mtp) 71 tok/s`  │
                     └───────────────────────────▲──────────────────────────────────────┘
                                                 │ DHT (snapclone presence node), channel = shared secret
                                                 │ hatch-core::status::Feed resolves every 20 s
        ┌──────────── M2 Ultra · Asahi Linux ─────────────┐        ┌────────── M5 Pro mini · macOS ──────────┐
        │ logscope-egui                                     │        │ logscope-shim  (:8790, bearer token)     │
        │  ├ addons::claude_code  → `claude -p` (headless)  │        │  ├ library  /Volumes/NVMe/models  (all)  │
        │  ├ addons::mini         → directives ─────────────┼──HTTP──┼─►├ cache    ~/models-cache  (budget, LRU)│
        │  ├ addons::status       → ledger / http / DHT ────┼────────┼─►│ POST /status  (relay to DHT)          │
        │  └ Remote backend       → LOGSCOPE_REMOTE=matrix:mini      │  ├ llama-server :8080 (-np, MTP profile) │
        │ hive: matrix · plan · dispatch · ledger · artifacts│◄──────┼──┤   registered as box `mini-llama`      │
        │ wgpu engine (Vulkan) · perf-loop · the workspace   │  10GbE │  └ Run{plan} → dispatch on its matrix   │
        └────────────────────────────┬──────────────────────┘        └──────────────────────────────────────────┘
                                     │ `claude -p --output-format stream-json` (the only permitted route for a Max plan)
                                     ▼
                            Claude Code binary → api.anthropic.com  (tier frontier, locality external)
```

## What flows where

| flow | from → to | carrier | shape |
|---|---|---|---|
| directive | Ultra UI → mini | HTTP `POST /directive` | `hive::directive::Directive` (status, models, load, unload, run, summary, say) |
| model | NVMe library → cache → llama-server | shim `library::ensure` | copy under budget, LRU eviction, `.part` rename |
| box | mini → Ultra's matrix | `Add to matrix` / `hive matrix add` | `mini` box, tier large, local; `LOGSCOPE_REMOTE=matrix:mini` uses it |
| plan | Ultra → mini → its boxes | `Directive::Run` | `hive::plan::Plan`; outcomes + summaries back in the reply, full text in the mini's ledger |
| review | Ultra → Claude → Ultra | `claude -p` | reply as a ledger artifact (`review`), optionally one status line |
| status | any box → ledger, shim, phone | `hive::status::StatusLine` | `{at, source, level, text}`; level is the liaison's fact / inference / unknown |
| integrate | Ultra bubble → the box with the real checkout | `Directive::Integrate` (bundle as base64) | `git bundle verify` → `refs/logscope/<name>` → throwaway worktree → verifier → the merge command, printed, never run |

## Setup

mini (once):
```
cargo build --release -p logscope-shim --features dht     # in apps/logscope
./target/release/logscope-shim                             # writes ~/.config/logscope/shim.json; edit library_dir, token, profiles
LOGSCOPE_STATUS_CHANNEL=<secret> ./target/release/logscope-shim
```
Ultra:
```
LOGSCOPE_MINI_URL=http://mini:8790 LOGSCOPE_MINI_TOKEN=<token> LOGSCOPE_STATUS_CHANNEL=<secret> logscope-egui
```
phone:
```
HATCH_STATUS_CHANNEL=<secret> hatch-egui        # built with --features status-dht
```

## The bubble's exit

`mount::bundle` (or any `git bundle create … HEAD <branch>`) is the only thing that leaves the scratch. The Hive tab's
mini panel sends it with `Integrate`; the shim on the box that holds the real checkout verifies the bundle, fetches the
branch into `refs/logscope/<name>`, checks it out in a throwaway worktree, runs the verifier there (`cargo check` by
default), removes the worktree, and replies GREEN/RED with `git merge --ff-only refs/logscope/<name>`. HEAD is never
moved by a machine. The same shim can run on the Ultra itself for local integration.

## Rules the code enforces

* Claude is reached only by driving the `claude` binary; the addon never reads a token.
* A directive is a JSON value both crates share (`hive::directive`); adding an op is one enum variant.
* The mini's cache is explainable by `ls`: budget minus LRU, nothing hidden.
* Status lines carry the liaison's epistemic tag; hatch shows the tag glyph (· fact, ~ inference, ? unknown) and dims a line older than three ask intervals.
* The DHT channel is a secret string; sha1(secret) is the key, so nobody without the string can find the line.
