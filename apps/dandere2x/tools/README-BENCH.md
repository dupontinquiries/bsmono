# dandere2x benchmark kit

Self-contained: source (GPU feature only, no GUI deps), the models
(RealESRGAN_x2 67 MB, realesr-animevideov3 2.5 MB, waifu2x upconv_7 10 MB),
and `run_benchmarks.sh`.

    unzip d2x-bench-kit.zip && cd d2x-bench-kit
    ./run_benchmarks.sh                         # ~15–40 min depending on GPU
    CLIP=/path/to/real.mkv ./run_benchmarks.sh  # + analysis and one run on real footage
    QUICK=1 ./run_benchmarks.sh                 # skip the end-to-end upscales

Needs cargo (stable), ffmpeg/ffprobe, and a Vulkan ICD. On Asahi:
`vulkaninfo --summary` should list Honeykrisp; if `probe` reports llvmpipe,
stop — the numbers would be for a CPU rasterizer.

What it measures, in order:
1. `selftest`, both kernels, both models: GPU vs CPU reference on this device.
   This is the first time the kernels run on Honeykrisp; PASS means bit-exact.
2. The test suites (CPU exactness vs numpy references; GPU with 3 sets in flight).
3. CPU hot paths, per call.
4. `bench`: ms/tile, GMAC/s, TFLOPS, per kernel × VRAM budget × in-flight sets,
   plus fixed tiles 400/640/1024. TFLOPS against the device peak is the
   kernel's efficiency — the number the next kernel work is judged by.
5. `analyze` on synthetic 1080p clips at quality 97/93/90: reuse fraction and
   main-network share (and on your CLIP if given).
6. End-to-end 1080p→4K upscale throughput: rt4 vs rt2, cascade on/off, full
   motion, `-j 2`, and the compact model.

Send back `report/d2x-report-<stamp>.tar.gz` (a few hundred KB; clips and
outputs are deleted).
