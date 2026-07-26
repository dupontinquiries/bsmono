#!/usr/bin/env python3
"""Self-test: every texture pattern is 256x256 and tiles (edge seam small)."""
import subprocess, sys, os, numpy as np
from PIL import Image
os.makedirs("/tmp/txtest",exist_ok=True); ok=True
for p in ["plating","hexmesh","circuit","rock","ground","sediment","crystal"]:
    o=f"/tmp/txtest/{p}.png"
    r=subprocess.run([sys.executable,"tools/gen_texture.py",p,o],capture_output=True,text=True)
    if r.returncode!=0: print("FAIL gen",p,r.stderr); ok=False; continue
    im=np.asarray(Image.open(o).convert("RGB")).astype(float)
    assert im.shape==(256,256,3), f"{p} wrong size {im.shape}"
    lr=np.abs(im[:,0]-im[:,-1]).mean(); tb=np.abs(im[0,:]-im[-1,:]).mean()
    tile = lr<25 and tb<25
    print(f"  {p:9} 256x256  seam L/R {lr:4.1f} T/B {tb:4.1f}  {'OK' if tile else 'SEAM!'}")
    ok = ok and tile
print("\nGEN_TEXTURE OK" if ok else "\nGEN_TEXTURE FAILED"); sys.exit(0 if ok else 1)
