# perfbisect

Bisect "it runs worse on machine A than machine B" without doing it by
hand. std-only; no engine dependency — it runs unendlich's own probes and
the game under a knob matrix, fingerprints each machine, and divides.

```sh
# once, per machine: build the engine's probes and the game
cd games/unendlich
cargo build --release --features "render window" --bin unendlich-world --bin perf-probe \
  --bin world-bench --bin mesh-bench --bin octree-bench
# optional, same features: --bin gpu-mesh-bench

# then, from the monorepo root, on each machine
cargo run --release --manifest-path games/shared/perfbisect/Cargo.toml -- \
  run --engine games/unendlich --out desktop.tsv          # ~18 x 12 s of game runs
#   --quick      five variants instead of eighteen
#   --seconds N  game run length per variant (default 12; 3 s warmup inside it)
#   --only calib,probe,game

# copy the phone's tsv over, then
cargo run --release --manifest-path games/shared/perfbisect/Cargo.toml -- \
  compare desktop.tsv phone.tsv
```

`compare` prints every probe both machines have, A/B, and the ANOMALY —
A/B divided by the CPU calibration ratio, so the phone's slower CPU is
divided out and what remains is unexplained. Then rules name suspects
with the numbers that fired them: implicit Vulkan layers that differ,
layers-forced-off faster than base (a layer loads by default), half
render scale halving the frame (fragment-bound: driver/resolution/
layer), vsync variants all equal (present mode is not it), a GPU-worldgen
penalty only one machine shows (the FP64/FP32 compute path).

The fingerprint records `vulkaninfo --summary`, the implicit layer
directories, and every `VK_*`, `MESA_*`, `UNENDLICH_*` variable, because
"no flags set" on the desktop usually means flags set by something else.

Output is TSV so two reports diff with anything. Written blind to the
actual disparity; the first real pair of reports will say which rules
were worth having.
