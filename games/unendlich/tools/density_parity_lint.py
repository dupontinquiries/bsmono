#!/usr/bin/env python3
"""CPU <-> GPU worldgen parity lint (density + value-noise).

The worldgen surface is computed twice: on the CPU in
`world_model.rs::density` (over `noise.rs`) and on the GPU in
`chunk_generation.comp` (with the noise inlined). These MUST agree, or
GPU-generated chunks stripe/terrace while CPU ones don't.

This lint fingerprints the parity-critical magic numbers in BOTH paths, in two
checks:
  * density — VERTICAL_BIAS_SCALE, continental/hills scales, detail
    frequency/octaves/amplitude, clamp bounds.
  * noise   — the hash avalanche multiplier, hash_2d/3d axis multipliers, the
    hash->float divisor, and the fbm lacunarity/gain. A drift here corrupts
    EVERYTHING built on the noise (terrain, caves, biomes, trees).

The shader-staleness guard in build.rs catches a stale .spv (compiled binary !=
source). It does NOT catch a *source-level* drift: someone edits the detail
amplitude (or a frequency/scale) in one file but not the other. This lint
fingerprints the parity-critical magic numbers in both files and fails if they
diverge.

Heuristic, not a proof of equivalence — but it catches the exact regression
class that produced the original striping. Run in CI alongside the build.

Exit 0 = parity, 1 = mismatch / extraction failure.
"""
import re
import sys
from pathlib import Path

ENGINE = Path(__file__).resolve().parents[1] / "crates" / "unendlich-engine"
CPU = ENGINE / "src" / "world" / "world_model.rs"
GPU = ENGINE / "src" / "shaders" / "chunk_generation.comp"
NOISE = ENGINE / "src" / "world" / "noise.rs"  # CPU noise; GPU noise is inlined in the comp

# Fields that must match across CPU and GPU. (label, description)
FIELDS = [
    ("bias_scale", "VERTICAL_BIAS_SCALE"),
    ("cont_scale", "continental height multiplier"),
    ("hills_scale", "hills height multiplier"),
    ("detail_freq", "3D detail-noise frequency"),
    ("detail_octaves", "3D detail-noise octaves"),
    ("detail_amp", "3D detail amplitude (the striping knob)"),
    ("clamp_lo", "vertical-bias clamp lower bound"),
    ("clamp_hi", "vertical-bias clamp upper bound"),
]

# The value-noise primitives underneath density/caves/biomes/trees. The GPU comp
# claims byte-for-byte match with noise.rs; a drift here corrupts EVERYTHING.
NOISE_FIELDS = [
    ("hash_mix", "hash_u32 avalanche multiplier (0x45d9f3b)"),
    ("hash2d_x", "hash_2d x multiplier"),
    ("hash2d_y", "hash_2d y multiplier"),
    ("hash3d_z", "hash_3d z multiplier"),
    ("h2f_div", "hash_to_float divisor (u32 max)"),
    ("lacunarity", "fbm frequency step (freq *= L)"),
    ("gain", "fbm amplitude step (amp *= G)"),
    ("fade_a", "smoothstep fade t^5 coeff (quintic=6)"),
    ("fade_b", "smoothstep fade t^4 coeff (quintic=15)"),
    ("fade_c", "smoothstep fade t^3 coeff (quintic=10)"),
]


def f(s):
    """Normalize a numeric literal to a float for tolerant comparison."""
    return round(float(s), 6)


def fn_body(text, sig_regex):
    """Return the brace-matched body of the first function whose signature
    matches `sig_regex`. Scopes regexes so we don't match `.clamp(` etc. from
    unrelated functions elsewhere in the file."""
    m = re.search(sig_regex, text)
    if not m:
        return ""
    i = text.find("{", m.end())
    if i < 0:
        return ""
    depth = 0
    for j in range(i, len(text)):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[i : j + 1]
    return text[i:]


def extract(whole, region, is_glsl):
    """Pull the parity fingerprint. `whole` is the full file (for the constant);
    `region` is just the density function body (for the math)."""
    d = {}
    # VERTICAL_BIAS_SCALE = 40.0 — declared as a constant, unambiguous file-wide.
    m = re.search(r"VERTICAL_BIAS_SCALE\s*[:=].*?=\s*([0-9.]+)", whole) \
        or re.search(r"VERTICAL_BIAS_SCALE\s*=\s*([0-9.]+)", whole)
    if m:
        d["bias_scale"] = f(m.group(1))
    # continental(...) * 30.0   /   model_continental(...) * 30.0
    m = re.search(r"(?:model_)?continental\([^)]*\)\s*\*\s*([0-9.]+)", region)
    if m:
        d["cont_scale"] = f(m.group(1))
    # hills(...) * 15.0   /   model_hills(...) * 15.0
    m = re.search(r"(?:model_)?hills\([^)]*\)\s*\*\s*([0-9.]+)", region)
    if m:
        d["hills_scale"] = f(m.group(1))
    # detail noise: fbm_3d(x * 0.025, ...)
    m = re.search(r"fbm_3d\(\s*x\s*\*\s*([0-9.]+)", region)
    if m:
        d["detail_freq"] = f(m.group(1))
    # octaves: first integer arg after the seed in the detail fbm_3d call.
    m = re.search(
        r"fbm_3d\(\s*x\s*\*\s*[0-9.]+,\s*y\s*\*\s*[0-9.]+,\s*z\s*\*\s*[0-9.]+,"
        r"\s*[^,]+,\s*([0-9]+)\s*,",
        region,
    )
    if m:
        d["detail_octaves"] = int(m.group(1))
    # detail amplitude: `detail * 0.15` (the final return term).
    m = re.search(r"detail\s*\*\s*([0-9.]+)", region)
    if m:
        d["detail_amp"] = f(m.group(1))
    # vertical-bias clamp bounds, scoped to the density body.
    if is_glsl:
        m = re.search(
            r"clamp\(\([^)]*\)\s*/\s*VERTICAL_BIAS_SCALE,\s*(-?[0-9.]+),\s*(-?[0-9.]+)\)",
            region,
        )
    else:
        m = re.search(r"\.clamp\(\s*(-?[0-9.]+),\s*(-?[0-9.]+)\s*\)", region)
    if m:
        d["clamp_lo"] = f(m.group(1))
        d["clamp_hi"] = f(m.group(2))
    return d


def extract_noise(text):
    """Pull the value-noise fingerprint. Works on both noise.rs (Rust) and the
    comp (GLSL); patterns are scoped to the exact var names so tree_detail_hash
    (uint(wx)*0x5ce343e9...) is not mis-captured."""
    d = {}
    # hash_u32 avalanche: ((x >> 16) ^ x) {.wrapping_mul(|* }0x45d9f3b
    m = re.search(r">>\s*16\)\s*\^\s*x\)\s*(?:\.wrapping_mul\(|\*\s*)0x([0-9a-fA-F]+)", text)
    if m:
        d["hash_mix"] = int(m.group(1), 16)
    # hash_2d/3d axis multipliers: (x as u32).wrapping_mul(N) | uint(x) * Nu
    for key, var in (("hash2d_x", "x"), ("hash2d_y", "y"), ("hash3d_z", "z")):
        m = re.search(
            rf"(?:uint\({var}\)|\({var} as u32\))\s*(?:\*\s*|\.wrapping_mul\()\s*([0-9]+)",
            text,
        )
        if m:
            d[key] = int(m.group(1))
    # hash_to_float divisor (u32 max), tolerant of Rust's `_` digit separators.
    m = re.search(r"/\s*(4[0-9_]+)\.0", text)
    if m:
        d["h2f_div"] = int(m.group(1).replace("_", ""))
    # fbm lacunarity / gain.
    m = re.search(r"freq\s*\*=\s*([0-9.]+)", text)
    if m:
        d["lacunarity"] = f(m.group(1))
    m = re.search(r"amp\s*\*=\s*([0-9.]+)", text)
    if m:
        d["gain"] = f(m.group(1))
    # smoothstep / fade polynomial: quintic t*t*t*(t*(t*6 - 15) + 10). The inner
    # `t*(t*A - B) + C` is the discriminating part; cubic t*t*(3-2t) won't match.
    m = re.search(r"t\s*\*\s*\(\s*t\s*\*\s*([0-9.]+)\s*-\s*([0-9.]+)\s*\)\s*\+\s*([0-9.]+)", text)
    if m:
        d["fade_a"] = f(m.group(1))
        d["fade_b"] = f(m.group(2))
        d["fade_c"] = f(m.group(3))
    return d


def report(name, fields, cpu, gpu, mismatch_msg):
    """Print one comparison table; return True iff parity (all fields match)."""
    print(f"\n{name}")
    print("-" * 70)
    print(f"{'field':12} {'CPU':>12} {'GPU':>12}  status   description")
    ok = True
    missing = False
    for key, desc in fields:
        cv = cpu.get(key)
        gv = gpu.get(key)
        if cv is None or gv is None:
            status, missing = "MISSING", True
        elif cv == gv:
            status = "ok"
        else:
            status, ok = "MISMATCH", False
        print(f"{key:12} {str(cv):>12} {str(gv):>12}  {status:8} {desc}")
    if missing:
        print("  -> a field failed to extract; regexes may need a nudge after a refactor.")
        return False
    if not ok:
        print(f"  -> {mismatch_msg}")
        return False
    return True


def main():
    if not CPU.exists() or not GPU.exists():
        print(f"density-parity: source missing\n  cpu={CPU}\n  gpu={GPU}")
        return 1
    cpu_text = CPU.read_text()
    gpu_text = GPU.read_text()
    # ---- density check ----
    # CPU: the density math was hoisted into surface_target() (2D per-column
    # terms: continental/hills/sea) + density_from_surface() (per-voxel:
    # vertical bias clamp + detail fbm). `fn density` itself now just composes
    # them. Concatenate all three bodies so the regexes still find every term,
    # mirroring the GPU side's two-body concatenation below.
    cpu_region = (
        fn_body(cpu_text, r"fn density\(")
        + "\n"
        + fn_body(cpu_text, r"fn surface_target\(")
        + "\n"
        + fn_body(cpu_text, r"fn density_from_surface\(")
    )
    # GPU: clamp/amp/freq/octaves in model_density_with_target; cont/hills in
    # model_density (the entrypoint). Concatenate both bodies as the region.
    gpu_region = (
        fn_body(gpu_text, r"model_density_with_target\(")
        + "\n"
        + fn_body(gpu_text, r"\bfloat\s+model_density\(")
    )
    density_ok = report(
        "density  (world_model.rs::density  vs  chunk_generation.comp)",
        FIELDS,
        extract(cpu_text, cpu_region, is_glsl=False),
        extract(gpu_text, gpu_region, is_glsl=True),
        "CPU/GPU density diverged -> GPU chunks stripe vs CPU. Keep detail_amp <= 0.15.",
    )

    # ---- noise check ----  (the primitives under density/caves/biomes/trees)
    noise_ok = True
    if NOISE.exists():
        noise_ok = report(
            "noise    (noise.rs  vs  chunk_generation.comp inlined noise)",
            NOISE_FIELDS,
            extract_noise(NOISE.read_text()),
            extract_noise(gpu_text),
            "CPU/GPU value-noise diverged -> ALL worldgen (terrain/caves/biomes/"
            "trees) differs between paths.",
        )
    else:
        print(f"\nnoise: source missing ({NOISE}) — skipped")

    print("\n" + "=" * 70)
    if density_ok and noise_ok:
        print("RESULT: PARITY — CPU and GPU worldgen fingerprints match.")
        return 0
    print("RESULT: MISMATCH — see the failing check(s) above.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
