import struct, sys, json
src, dst = sys.argv[1], sys.argv[2]
b=open(src,'rb').read()
H=struct.unpack_from('<27I', b, 16)
nv, ofs_va, nva = H[8], H[9], H[7]
nt, ofs_t = H[10], H[11]
pofs=None
for k in range(nva):
    t,fl,fmt,sz,ofs=struct.unpack_from('<5I', b, ofs_va+k*20)
    if t==0: pofs=ofs
V=[struct.unpack_from('<3f', b, pofs+i*12) for i in range(nv)]
I=list(struct.unpack_from(f"<{nt*3}I", b, ofs_t))
pb=b''.join(struct.pack('<3f',*v) for v in V)
ib=b''.join(struct.pack('<I',i) for i in I)
while len(pb)%4: pb+=b'\0'
bin_=pb+ib
lo=[min(v[a] for v in V) for a in range(3)]; hi=[max(v[a] for v in V) for a in range(3)]
g={"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],
   "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1}]}],
   "accessors":[{"bufferView":0,"componentType":5126,"count":nv,"type":"VEC3","min":lo,"max":hi},
                {"bufferView":1,"componentType":5125,"count":len(I),"type":"SCALAR"}],
   "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":len(pb)},
                  {"buffer":0,"byteOffset":len(pb),"byteLength":len(ib)}],
   "buffers":[{"byteLength":len(bin_)}]}
js=json.dumps(g).encode()
while len(js)%4: js+=b' '
out=struct.pack('<III',0x46546C67,2,12+8+len(js)+8+len(bin_))
out+=struct.pack('<II',len(js),0x4E4F534A)+js+struct.pack('<II',len(bin_),0x004E4942)+bin_
open(dst,'wb').write(out)
print(f"wrote {dst}: {nv} verts {nt} tris, {len(out)} bytes")
