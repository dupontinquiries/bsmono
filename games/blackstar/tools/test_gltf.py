#!/usr/bin/env python3
"""test_gltf.py -- guards gltf_to_iqm.py: builds a minimal glTF + GLB cube in memory,
converts each to IQM, and checks the output is a load-compatible, unit-slot IQM. Runs as
the `gltf_roundtrip` ctest so the Blender import path can't silently break."""
import sys, os, json, struct, base64, subprocess, tempfile
HERE = os.path.dirname(os.path.abspath(__file__))

POS = [(-1,-1,-1),(1,-1,-1),(1,1,-1),(-1,1,-1),(-1,-1,1),(1,-1,1),(1,1,1),(-1,1,1)]
IDX = [0,1,2,0,2,3,4,6,5,4,7,6,0,4,5,0,5,1,1,5,6,1,6,2,2,6,7,2,7,3,3,7,4,3,4,0]

def _buffers():
    p = b"".join(struct.pack("<3f",*v) for v in POS); pad = (-len(p)) % 4
    i = b"".join(struct.pack("<H",k) for k in IDX)
    return p, pad, i

def make_gltf(path):
    p, pad, i = _buffers(); buf = p + b"\x00"*pad + i
    g = {"asset":{"version":"2.0"},
         "buffers":[{"byteLength":len(buf),"uri":"data:application/octet-stream;base64,"+base64.b64encode(buf).decode()}],
         "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":len(p)},
                        {"buffer":0,"byteOffset":len(p)+pad,"byteLength":len(i)}],
         "accessors":[{"bufferView":0,"componentType":5126,"count":8,"type":"VEC3"},
                      {"bufferView":1,"componentType":5123,"count":36,"type":"SCALAR"}],
         "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1,"mode":4}]}],
         "nodes":[{"mesh":0,"scale":[0.5,0.5,0.5]}], "scenes":[{"nodes":[0]}], "scene":0}
    open(path,"w").write(json.dumps(g))

def make_glb(path):
    p, pad, i = _buffers(); b = p + b"\x00"*pad + i; b += b"\x00"*((-len(b))%4)
    g = {"asset":{"version":"2.0"},"buffers":[{"byteLength":len(b)}],
         "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":len(p)},
                        {"buffer":0,"byteOffset":len(p)+pad,"byteLength":len(i)}],
         "accessors":[{"bufferView":0,"componentType":5126,"count":8,"type":"VEC3"},
                      {"bufferView":1,"componentType":5123,"count":36,"type":"SCALAR"}],
         "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1,"mode":4}]}],
         "nodes":[{"mesh":0}],"scenes":[{"nodes":[0]}],"scene":0}
    js = json.dumps(g).encode(); js += b" "*((-len(js))%4)
    out = b"glTF"+struct.pack("<II",2,12+8+len(js)+8+len(b))
    out += struct.pack("<II",len(js),0x4E4F534A)+js
    out += struct.pack("<II",len(b),0x004E4942)+b
    open(path,"wb").write(out)

def check_iqm(path):
    d = open(path,"rb").read()
    assert d[:16] == b"INTERQUAKEMODEL\x00", "bad IQM magic"
    ver, filesize = struct.unpack("<II", d[16:24])
    assert ver == 2, f"IQM version {ver} != 2"
    assert filesize == len(d), "IQM filesize header mismatch"
    nv = struct.unpack("<I", d[48:52])[0]        # num_vertexes
    assert nv == 8, f"expected 8 verts, got {nv}"
    # unit-slot: every position component within [-1.06, 1.06]
    ofs_va = struct.unpack("<I", d[52:56])[0]    # ofs_vertexarrays
    # positions array offset is the 5th field of the first vertexarray (idx 4)
    va0 = struct.unpack("<5I", d[ofs_va:ofs_va+20]); pos_ofs = va0[4]
    for k in range(nv):
        x,y,z = struct.unpack("<3f", d[pos_ofs+k*12:pos_ofs+k*12+12])
        assert max(abs(x),abs(y),abs(z)) <= 1.06, "not unit-slot normalised"

def run(src, out):
    subprocess.run([sys.executable, os.path.join(HERE,"gltf_to_iqm.py"), src, "--out", out],
                   check=True, stdout=subprocess.DEVNULL)
    check_iqm(out)

def main():
    with tempfile.TemporaryDirectory() as t:
        gp = os.path.join(t,"c.gltf"); make_gltf(gp); run(gp, os.path.join(t,"a.iqm"))
        bp = os.path.join(t,"c.glb");  make_glb(bp);  run(bp, os.path.join(t,"b.iqm"))
    print("gltf_to_iqm: .gltf and .glb both convert to valid unit-slot IQM")
    print("GLTF ROUNDTRIP OK")

if __name__ == "__main__":
    main()
