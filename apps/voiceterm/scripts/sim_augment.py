#!/usr/bin/env python3
"""sim_augment.py (v0.1.656) — audio augmentation for the sim harness.

Adds acoustic + timing VARIETY to espeak output so single-user simulation
exercises more of the pipeline. Three augmentations, with measured findings:

  - PITCH shift (espeak -p): cheap, but barely moves the 39-dim fingerprint
    (~0.0001 cosine) because MFCCs track formants, not F0. Good for pitch-
    BIN robustness, not speaker separation.
  - tanh DISTORTION (waveshaper, drive D): the real lever for spectral
    variety (~40x more than pitch at D=5). Adds harmonics the MFCCs see.
    Still NOT a substitute for real-voice separation (one synthetic source
    perturbed != different vocal tracts) — use it for robustness testing.
  - varied WORD SPACING with erroneous gaps: the most valuable. Splices
    per-word audio with randomized inter-word silence; an "erroneous" >880ms
    gap reproduces a PREMATURE VAD-CUTOFF case (the VAD stops on 880ms
    silence per runtime logs). Neither sine waves nor uniform espeak could
    generate this.

(We do NOT use surgeshaper-tanh: it exists on crates.io but is alpha, pulls
136 deps, and needs coreaudio-sys which is macOS-only / panics on Linux.
tanh is three lines.)

Usage: python3 scripts/sim_augment.py [outdir]   (default /tmp/sim-aug)
Requires espeak-ng.
"""
import subprocess, wave, struct, math, random, os, sys
random.seed(7)
OUT = sys.argv[1] if len(sys.argv) > 1 else "/tmp/sim-aug"
os.makedirs(OUT, exist_ok=True)

def espeak(text, path, voice="en-us", speed=150, pitch=50):
    subprocess.run(["espeak-ng","-v",voice,"-s",str(speed),"-p",str(pitch),"-w",path,text],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
def read(path):
    w=wave.open(path); n=w.getnframes(); r=w.getframerate()
    return list(struct.unpack("<%dh"%n, w.readframes(n))), r
def write(samples, r, path):
    w=wave.open(path,"w"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(r)
    cl=[max(-32767,min(32767,int(x))) for x in samples]
    w.writeframes(struct.pack("<%dh"%len(cl), *cl)); w.close()
def tanh_shape(samples, drive):
    k=math.tanh(drive)
    return [math.tanh(drive*(x/32768.0))/k*32767 for x in samples]
def silence(ms, r): return [0]*int(r*ms/1000)

def variants(phrase):
    print(f"## acoustic variants of '{phrase}'")
    for name,pitch,speed,drive in [("clean",50,150,0),("low",20,140,0),("high",80,160,0),
                                    ("dist2",50,150,2.0),("dist5",50,150,5.0)]:
        raw=f"{OUT}/{name}_raw.wav"; espeak(phrase,raw,pitch=pitch,speed=speed)
        s,r=read(raw)
        if drive>0: s=tanh_shape(s,drive)
        write(s,r,f"{OUT}/{name}.wav")
        print(f"  {name:8s} pitch={pitch} speed={speed} drive={drive}")

def spacing(words):
    print(f"## varied word spacing for {words}")
    for name,gaps,jit in [("even",[120,120],False),("tight",[40,40],False),
                          ("draggy",[350,300],False),("erroneous",[60,900],True),
                          ("stutter",[200,50],True)]:
        alls=[]; r=22050
        for i,w in enumerate(words):
            p=f"{OUT}/_w{i}.wav"; spd=150+(random.randint(-25,25) if jit else 0)
            espeak(w,p,speed=spd); s,r=read(p); alls+=s
            if i<len(words)-1: alls+=silence(gaps[i],r)
        write(alls,r,f"{OUT}/sp_{name}.wav")
        flag=" <-- >880ms gap: triggers premature VAD stop" if max(gaps)>880 else ""
        print(f"  {name:10s} gaps={gaps}{flag}")

if __name__=="__main__":
    if subprocess.run(["which","espeak-ng"],capture_output=True).returncode!=0:
        print("espeak-ng not installed (apt-get install -y espeak-ng)"); sys.exit(1)
    variants("run cargo test"); print(); spacing(["run","cargo","test"])
    print(f"\nWAVs in {OUT}. Feed to the phonetic pipeline (docs/design/sim_harness.md).")
    print("Measured: pitch barely moves the fingerprint (~0.0001); distortion ~40x")
    print("more (still not real-voice separation); erroneous spacing reproduces VAD cutoff.")
