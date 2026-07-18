#!/usr/bin/env python3
"""Convert a kBM3 (gan-zai-v5.3.py) generator checkpoint into the format the
`beatspace` egui app loads: `gen_latest.safetensors` + `config.json`.

Why a converter is needed
-------------------------
The kBM3 Python trainer saves PyTorch `.pt` checkpoints; the Rust `beatspace`
GUI uses candle and loads `.safetensors` with its own key names. The two models
are the *same architecture* (identical layer shapes, including the f/2 FC), but
the weight KEY NAMES differ:

    PyTorch (gan-zai-v5.3.py Generator)   ->  beatspace (candle) keys
    ------------------------------------      ------------------------
    fc.weight                             ->  g.fc.w
    fc.bias                               ->  g.fc.b
    main.0.weight   (ConvTranspose2d 0)   ->  g.deconv0.w
    main.1.weight   (BatchNorm2d 0 gamma) ->  g.bn0.gamma
    main.1.bias     (BatchNorm2d 0 beta)  ->  g.bn0.beta
    main.1.running_mean                   ->  g.bn0.run_mean
    main.1.running_var                    ->  g.bn0.run_var
    main.3.weight   (ConvTranspose2d 1)   ->  g.deconv1.w
    main.4.*        (BatchNorm2d 1)       ->  g.bn1.*
    main.6.weight   (ConvTranspose2d 2)   ->  g.deconv2.w
    main.7.*        (BatchNorm2d 2)       ->  g.bn2.*
    main.9.weight   (ConvTranspose2d 3)   ->  g.deconv3.w   (no BN after last)

No tensor transposes are required: PyTorch `nn.Linear.weight` is [out,in] and
beatspace's Linear stores [out,in] with `x @ w.t()`; PyTorch
`nn.ConvTranspose2d.weight` is [in,out,kH,kW] and beatspace stores the same.
Only the names (and BN weight/bias -> gamma/beta) change.

Usage
-----
    python to_beatspace.py --ckpt checkpoints/gen_20000.pt --out ./bs_ckpt
    # writes ./bs_ckpt/gen_latest.safetensors and ./bs_ckpt/config.json
    # then:  beatspace gui --ckpt-dir ./bs_ckpt

The checkpoint may be either a raw generator state_dict, or a dict with the
generator under one of: 'G', 'gen', 'generator', 'model', 'ema', 'g_ema'
(pass --key to force one; --ema prefers an EMA generator if present).
"""
import argparse
import json
import os
import sys


def find_state_dict(obj, forced_key=None, prefer_ema=False):
    """Pull the generator state_dict out of whatever .pt structure we got."""
    import torch  # noqa

    # already a flat state_dict of tensors?
    def looks_like_sd(d):
        return isinstance(d, dict) and d and all(
            hasattr(v, "shape") for v in d.values()
        )

    if forced_key is not None:
        if forced_key not in obj:
            sys.exit(f"--key '{forced_key}' not found; top-level keys: {list(obj)[:12]}")
        return obj[forced_key]

    if looks_like_sd(obj):
        return obj

    if isinstance(obj, dict):
        ema_keys = ["g_ema", "ema", "gen_ema", "generator_ema"]
        gen_keys = ["G", "gen", "generator", "model", "netG", "g"]
        order = (ema_keys + gen_keys) if prefer_ema else (gen_keys + ema_keys)
        for k in order:
            if k in obj and looks_like_sd(obj[k]):
                print(f"[convert] using generator under key '{k}'")
                return obj[k]
        # nested one level (e.g. {'state_dict': {...}})
        for k in ("state_dict", "model_state_dict"):
            if k in obj and isinstance(obj[k], dict):
                return find_state_dict(obj[k], None, prefer_ema)

    sys.exit(
        "could not locate a generator state_dict in the checkpoint. "
        f"top-level type={type(obj).__name__}, keys={list(obj)[:12] if isinstance(obj, dict) else 'n/a'}. "
        "Pass --key to point at it."
    )


def strip_prefixes(sd):
    """Drop common wrapper prefixes like 'module.' (DataParallel)."""
    out = {}
    for k, v in sd.items():
        nk = k
        for pref in ("module.", "_orig_mod."):
            if nk.startswith(pref):
                nk = nk[len(pref):]
        out[nk] = v
    return out


def convert(sd):
    """Map kBM3 PyTorch generator keys -> beatspace candle keys.
    Returns (new_state_dict, n_deconv, n_bn) or exits on a structural mismatch."""
    sd = strip_prefixes(sd)
    out = {}

    # --- FC ---
    if "fc.weight" not in sd or "fc.bias" not in sd:
        sys.exit(f"expected fc.weight/fc.bias in checkpoint; got keys like {list(sd)[:8]}")
    out["g.fc.w"] = sd["fc.weight"]
    out["g.fc.b"] = sd["fc.bias"]

    # --- main.N sequence: deconv at 0,3,6,9 ; BN at 1,4,7 ---
    deconv_indices = [0, 3, 6, 9]
    bn_indices = [1, 4, 7]
    di = 0
    for mi in deconv_indices:
        wk = f"main.{mi}.weight"
        if wk not in sd:
            sys.exit(f"missing {wk} -- checkpoint architecture doesn't match gan-zai-v5.3.py")
        out[f"g.deconv{di}.w"] = sd[wk]
        di += 1
    bi = 0
    for mi in bn_indices:
        g = f"main.{mi}.weight"
        b = f"main.{mi}.bias"
        rm = f"main.{mi}.running_mean"
        rv = f"main.{mi}.running_var"
        for need in (g, b, rm, rv):
            if need not in sd:
                sys.exit(f"missing BatchNorm tensor {need}")
        out[f"g.bn{bi}.gamma"] = sd[g]
        out[f"g.bn{bi}.beta"] = sd[b]
        out[f"g.bn{bi}.run_mean"] = sd[rm]
        out[f"g.bn{bi}.run_var"] = sd[rv]
        bi += 1

    return out, di, bi


def infer_config(new_sd):
    """Recover the model shape from tensor shapes so config.json matches."""
    fc_w = new_sd["g.fc.w"]            # [fc_ch*h0*w0, latent]
    latent = int(fc_w.shape[1])
    # deconv0 weight is [fc_ch, f/2, 4, 4]; fc_ch == f/2
    d0 = new_sd["g.deconv0.w"]
    fc_ch = int(d0.shape[0])
    g_filters = fc_ch * 2             # since fc_ch = f//2
    # last deconv output channel is 1; recover n_mels/time_frames needs h0,w0.
    # fc_ch*h0*w0 = fc_w.shape[0]  -> h0*w0 = shape0 / fc_ch
    h0w0 = int(fc_w.shape[0]) // fc_ch
    # h0 = n_mels/16, w0 = time_frames/16, and the model upsamples 16x.
    # We can't uniquely split h0w0 without one of them, but n_mels is usually
    # square-ish; default kBM3 is n_mels=256 -> h0=16. Try to find integer h0
    # with n_mels a multiple of 16 that divides h0w0.
    # Most reliable: expose both and let the user confirm; default to the
    # standard kBM3 256x560 if it fits.
    return latent, g_filters, h0w0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ckpt", required=True, help="path to kBM3 generator .pt")
    ap.add_argument("--out", default="./bs_ckpt", help="output dir for beatspace")
    ap.add_argument("--key", default=None, help="force the top-level dict key holding the generator")
    ap.add_argument("--ema", action="store_true", help="prefer an EMA generator if present")
    ap.add_argument("--n-mels", type=int, default=None, help="override n_mels for config.json")
    ap.add_argument("--time-frames", type=int, default=None, help="override time_frames for config.json")
    ap.add_argument("--sr", type=int, default=48000)
    ap.add_argument("--n-fft", type=int, default=4096)
    ap.add_argument("--hop", type=int, default=512)
    args = ap.parse_args()

    try:
        import torch
    except ImportError:
        sys.exit("this converter needs PyTorch (to read the .pt). pip install torch")
    try:
        from safetensors.torch import save_file
    except ImportError:
        sys.exit("this converter needs safetensors. pip install safetensors")

    obj = torch.load(args.ckpt, map_location="cpu", weights_only=False)
    sd = find_state_dict(obj, args.key, args.ema)
    new_sd, n_deconv, n_bn = convert(sd)
    # ensure contiguous float32 CPU tensors
    new_sd = {k: v.detach().to(torch.float32).contiguous() for k, v in new_sd.items()}

    latent, g_filters, h0w0 = infer_config(new_sd)

    # figure out n_mels/time_frames
    n_mels = args.n_mels
    time_frames = args.time_frames
    if n_mels is None or time_frames is None:
        # h0 = n_mels/16, w0 = time_frames/16, h0*w0 == h0w0
        # default kBM3: n_mels=256 (h0=16) -> w0 = h0w0/16 -> time_frames = w0*16
        if h0w0 % 16 == 0:
            h0_guess = 16
            w0_guess = h0w0 // h0_guess
            n_mels = n_mels or h0_guess * 16
            time_frames = time_frames or w0_guess * 16
        else:
            sys.exit(
                f"couldn't infer n_mels/time_frames from h0*w0={h0w0}; "
                "pass --n-mels and --time-frames explicitly."
            )

    os.makedirs(args.out, exist_ok=True)
    st_path = os.path.join(args.out, "gen_latest.safetensors")
    save_file(new_sd, st_path)

    cfg = {
        "sr": args.sr,
        "n_fft": args.n_fft,
        "hop": args.hop,
        "n_mels": n_mels,
        "time_frames": time_frames,
        "latent": latent,
        "g_filters": g_filters,
    }
    cfg_path = os.path.join(args.out, "config.json")
    with open(cfg_path, "w") as f:
        json.dump(cfg, f, indent=2)

    print(f"[convert] wrote {st_path} ({len(new_sd)} tensors)")
    print(f"[convert] wrote {cfg_path}: {cfg}")
    print(f"[convert] {n_deconv} deconv + {n_bn} BN layers mapped")
    print(f"\nRun it:\n  beatspace gui --ckpt-dir {args.out}")


if __name__ == "__main__":
    main()
