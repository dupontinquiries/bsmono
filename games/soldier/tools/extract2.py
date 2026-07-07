import numpy as np, sys
from PIL import Image
from scipy.ndimage import median_filter
import mcblocks as m

def extract_named(region_dir, x0, z0, size):
    import os, glob
    H=np.full((size,size),-999,np.int32)
    names=np.empty((size,size),object); names[:]='air'
    regions={}
    for p in glob.glob(os.path.join(region_dir,'r.*.mca')):
        if os.path.getsize(p)==0: continue
        rx,rz=m.region_coords(p); regions[(rx,rz)]=p
    for bz in range(z0,z0+size,16):
        for bx in range(x0,x0+size,16):
            cx,cz=bx>>4,bz>>4; rx,rz=cx>>5,cz>>5
            p=regions.get((rx,rz))
            if not p: continue
            data=open(p,'rb').read(); ci=(cz&31)*32+(cx&31)
            try: nbt=m.read_chunk_nbt(data,ci)
            except Exception: nbt=None
            if nbt is None: continue
            hm=nbt.get('Heightmaps')
            if not hm or 'WORLD_SURFACE' not in hm: continue
            heights=m.decode_hm(hm['WORLD_SURFACE'])
            secs={int(s['Y']):s for s in nbt.get('sections',[]) if 'block_states' in s}
            for lz in range(16):
                for lx in range(16):
                    gx,gz=(cx*16+lx)-x0,(cz*16+lz)-z0
                    if not(0<=gx<size and 0<=gz<size): continue
                    hv=int(heights[lz*16+lx]); ty=hv+m.MIN_Y-1
                    H[gz,gx]=hv; sec=secs.get(ty>>4)
                    if sec is not None: names[gz,gx]=m.section_block(sec,lx,ty&15,lz)
    return H,names

def classify_grid(H, names):
    Hf=H.astype(np.float32); Hf[H<-900]=np.nan
    fill=np.nan_to_num(Hf, nan=np.nanmedian(Hf))
    ground=median_filter(fill, size=25)
    above=fill-ground
    C=np.zeros(H.shape,np.uint8)
    for z in range(H.shape[0]):
        for x in range(H.shape[1]):
            C[z,x]=m.classify(names[z,x], above[z,x])
    return C, above

PALETTE={0:(60,62,66),1:(225,205,170),2:(110,108,112),3:(40,90,150),4:(70,120,60),5:(220,200,140)}
def render_class(C, path):
    rgb=np.zeros((*C.shape,3),np.uint8)
    for k,c in PALETTE.items(): rgb[C==k]=c
    Image.fromarray(rgb).save(path)

if __name__=='__main__':
    rd,x0,z0,size,name=sys.argv[1],int(sys.argv[2]),int(sys.argv[3]),int(sys.argv[4]),sys.argv[5]
    H,names=extract_named(rd,x0,z0,size)
    C,above=classify_grid(H,names)
    np.save(f'/tmp/refs/{name}_H.npy',H); np.save(f'/tmp/refs/{name}_C.npy',C)
    render_class(C, f'/tmp/refs/{name}_clean.png')
    tot=C.size
    for k,lbl in [(0,'open'),(1,'building'),(2,'street'),(3,'water'),(4,'nature'),(5,'sand')]:
        print(f'  {lbl:9} {100*(C==k).sum()/tot:5.1f}%', end='')
    print()
