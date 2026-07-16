#!/usr/bin/env python3
"""kBM3 model server for the latent explorer.

Serves index.html and a /generate endpoint that turns a latent vector into a
real mel + audio using a trained generator checkpoint (gen_*.pt from
gan-zai-v5.2.py). The explorer runs in demo mode without this; point it here
("connect model...") for real output.

    pip install fastapi uvicorn torch librosa soundfile numpy
    python server.py --weights checkpoints/gen_8000.pt --port 8000
    # then open http://localhost:8000  (or connect a separately-hosted UI here)

The Generator/Config/mel-inverse below mirror gan-zai-v5.2.py exactly, so a
gen_*.pt state_dict loads directly (identical architecture => identical keys).
"""
import argparse
import base64
import io
import os
import sys

# The neural vocoder lives once in the repo at ../vocoder; make it importable
# whether the server is launched from explorer/ or elsewhere.
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "vocoder"))

import numpy as np
import torch
import torch.nn as nn
import librosa
import soundfile as sf

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import HTMLResponse, JSONResponse
from pydantic import BaseModel

# ---- config (must match the trained checkpoint) ----
LATENT, G_FILTERS, N_MELS, TIME_FRAMES = 128, 256, 128, 256
SR, N_FFT, HOP = 22050, 1024, 256
DB_FLOOR, DB_CEIL = -80.0, 40.0
GL_ITERS = 32            # lower = faster/rougher audio; raise for quality
MEL_OUT_H, MEL_OUT_W = 72, 144   # downsampled mel sent to the browser


class Generator(nn.Module):
    """Identical architecture to gan-zai-v5.2.py's Generator."""
    def __init__(self):
        super().__init__()
        f, mh, mw = G_FILTERS, N_MELS // 16, TIME_FRAMES // 16
        self.f, self.min_h, self.min_w = f, mh, mw
        self.fc = nn.Linear(LATENT, f * mh * mw)
        self.main = nn.Sequential(
            nn.ConvTranspose2d(f, f // 2, 4, 2, 1, bias=False), nn.BatchNorm2d(f // 2), nn.ReLU(True),
            nn.ConvTranspose2d(f // 2, f // 4, 4, 2, 1, bias=False), nn.BatchNorm2d(f // 4), nn.ReLU(True),
            nn.ConvTranspose2d(f // 4, f // 8, 4, 2, 1, bias=False), nn.BatchNorm2d(f // 8), nn.ReLU(True),
            nn.ConvTranspose2d(f // 8, 1, 4, 2, 1, bias=False), nn.Tanh(),
        )

    def forward(self, z):
        x = self.fc(z).view(-1, self.f, self.min_h, self.min_w)
        return self.main(x)


def unit_to_audio(unit):
    db = np.clip(unit, -1, 1) * (DB_CEIL - DB_FLOOR) / 2 + (DB_CEIL + DB_FLOOR) / 2
    power = librosa.db_to_power(db, ref=1.0)
    return librosa.feature.inverse.mel_to_audio(
        power, sr=SR, n_fft=N_FFT, hop_length=HOP, n_iter=GL_ITERS)


def downsample(mel, h, w):
    fi = (np.linspace(0, mel.shape[0] - 1, h)).astype(int)
    ti = (np.linspace(0, mel.shape[1] - 1, w)).astype(int)
    return mel[np.ix_(fi, ti)]


device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
netG = Generator().to(device).eval()
VOC = None          # optional neural vocoder (set via --vocoder); else Griffin-Lim


def synth_audio(mel):
    """mel: (n_mels, time_frames) in [-1,1]. Neural vocoder if loaded (fast + far
    better), else Griffin-Lim (slow)."""
    if VOC is not None:
        from vocoder import vocode
        return vocode(mel, VOC[1], VOC[0], device)
    return unit_to_audio(mel)

app = FastAPI(title="kBM3 model server")
app.add_middleware(CORSMiddleware, allow_origins=["*"], allow_methods=["*"], allow_headers=["*"])

HTML = ""  # filled at startup


class Req(BaseModel):
    z: list[float]
    audio: bool = True


@app.get("/", response_class=HTMLResponse)
def index():
    return HTML


@app.post("/generate")
def generate(req: Req):
    z = torch.tensor(np.asarray(req.z, dtype=np.float32)[None, :LATENT], device=device)
    with torch.no_grad():
        mel = netG(z)[0, 0].cpu().numpy()          # (n_mels, time_frames), [-1,1]
    out = {"mel": downsample(mel, MEL_OUT_H, MEL_OUT_W).astype(float).tolist()}
    if req.audio:
        wav = synth_audio(mel)
        buf = io.BytesIO()
        sf.write(buf, wav, SR, format="WAV")
        out["wav"] = base64.b64encode(buf.getvalue()).decode("ascii")
    return JSONResponse(out)


def main():
    global HTML, VOC
    p = argparse.ArgumentParser()
    p.add_argument("--weights", required=True, help="path to gen_*.pt")
    p.add_argument("--vocoder", default=None, help="path to voc_*.pt (else Griffin-Lim)")
    p.add_argument("--html", default="index.html")
    p.add_argument("--port", type=int, default=8000)
    args = p.parse_args()

    state = torch.load(args.weights, map_location=device)
    if isinstance(state, dict) and "G" in state:   # combined checkpoint
        state = state["G"]
    netG.load_state_dict(state)
    print(f"loaded {args.weights} on {device}")

    if args.vocoder:
        from vocoder import load_vocoder
        VOC = load_vocoder(args.vocoder, device)     # (netG, cfg)
        print(f"loaded vocoder {args.vocoder} — audio via neural vocoder")
    else:
        print("no --vocoder: audio via Griffin-Lim (slow; pass --vocoder for real-time)")

    with open(args.html) as f:
        HTML = f.read()

    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=args.port)


if __name__ == "__main__":
    main()
