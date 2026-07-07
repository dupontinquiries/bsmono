#!/usr/bin/env python3
"""gen_box.py -- generates data/box.iqm, a unit cube (half-extent 1) so robot
parts and cover render as boxes instead of the stock cube.iqm (which is a
sphere). Single material 'wall.png'. IQM v2, static, no joints.

Regen: python3 tools/gen_box.py"""
import struct, os

verts, tris = [], []
def quad(a,b,c,d,n):
    base=len(verts)
    ax = 0 if abs(n[0])<0.9 else 2
    ay = 1 if abs(n[1])<0.9 else 2
    for p in (a,b,c,d):
        verts.append((p,(p[ax]*0.5+0.5, p[ay]*0.5+0.5), n))
    tris.append((base,base+1,base+2)); tris.append((base,base+2,base+3))

h=1.0
quad((-h,h,-h),(-h,h,h),(h,h,h),(h,h,-h),(0,1,0))      # top
quad((-h,-h,h),(-h,-h,-h),(h,-h,-h),(h,-h,h),(0,-1,0))  # bottom
quad((-h,-h,h),(h,-h,h),(h,h,h),(-h,h,h),(0,0,1))       # +z
quad((h,-h,-h),(-h,-h,-h),(-h,h,-h),(h,h,-h),(0,0,-1))  # -z
quad((h,-h,h),(h,-h,-h),(h,h,-h),(h,h,h),(1,0,0))       # +x
quad((-h,-h,-h),(-h,-h,h),(-h,h,h),(-h,h,-h),(-1,0,0))  # -x

def u(*a): return struct.pack("<"+"I"*len(a), *a)
mat=b"wall.png\x00"; text=b"\x00"+mat+b"box\x00"
mat_off=1; name_off=1+len(mat)
num_v,num_t=len(verts),len(tris)
ofs=16+4+4*26
ofs_text=ofs; ofs+=len(text); ofs+=(-ofs)%4
ofs_meshes=ofs; ofs+=1*6*4
ofs_va=ofs; ofs+=3*5*4
ofs_pos=ofs; ofs+=num_v*12
ofs_uv=ofs; ofs+=num_v*8
ofs_nrm=ofs; ofs+=num_v*12
ofs_tri=ofs; ofs+=num_t*12
filesize=ofs

out=b"INTERQUAKEMODEL\x00"+u(2,filesize,0)
out+=u(len(text),ofs_text)+u(1,ofs_meshes)+u(3,num_v,ofs_va)+u(num_t,ofs_tri,0)
out+=u(0,0)+u(0,0)+u(0,0)+u(0,0,0,0)+u(0,0)+u(0,0)
pad=ofs_text+len(text); out+=text+b"\x00"*((-(pad))%4)
out+=u(name_off,mat_off,0,num_v,0,num_t)
out+=u(0,0,7,3,ofs_pos)+u(1,0,7,2,ofs_uv)+u(2,0,7,3,ofs_nrm)
for p,_,_ in verts: out+=struct.pack("<3f",*p)
for _,uv,_ in verts: out+=struct.pack("<2f",*uv)
for _,_,n in verts: out+=struct.pack("<3f",*n)
for t in tris: out+=u(*t)
assert len(out)==filesize,(len(out),filesize)
dst=os.path.join(os.path.dirname(__file__),"..","engine_src","data","box.iqm")
open(dst,"wb").write(out)
print(f"wrote {dst}: {num_v} verts {num_t} tris {filesize} bytes")
