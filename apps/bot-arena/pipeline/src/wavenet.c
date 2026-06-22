/* wavenet.c — a throwback: WaveNet-style character language model in C99.
 *
 * WaveNet (van den Oord et al., DeepMind 2016) generates sequences with *dilated
 * causal convolutions* instead of recurrence or attention. This is a faithful, tiny,
 * dependency-free char-level version:
 *
 *   embedding → L gated dilated-causal-conv layers (dilation 1,2,4,…) with residual
 *   and skip connections → ReLU(sum of skips) → 1x1 conv → logits → softmax.
 *
 * Each layer's "gated activation unit" is the WaveNet signature:
 *     z = tanh(conv_f(x)) ⊙ sigmoid(conv_g(x))
 * Kernel size 2, so layer l (dilation d=2^l) mixes x[t] and x[t-d]; stacking L layers
 * gives a receptive field of 2^L characters with only L layers — the whole point.
 *
 * Also includes `render`: text laid out as a 2D image (each char a pixel), optionally
 * coloured by the model's learned embedding — the "text as 2D images" idea, which is
 * really the doorway to PixelCNN (WaveNet's 2D masked-conv sibling).
 *
 *   wavenet train  <corpus_dir> <model.bin> [steps] [seqlen] [channels] [layers] [lr]
 *   wavenet gen    <model.bin> [n_chars] [seed_text]
 *   wavenet render <corpus_dir|file> <out.ppm> [width] [model.bin]
 *
 * Build: cc -std=c99 -O3 -o wavenet wavenet.c -lm
 */
#define _POSIX_C_SOURCE 200809L
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <math.h>
#include <time.h>
#include <dirent.h>
#include <sys/stat.h>

/* ---- tiny RNG (xorshift) ---------------------------------------------- */
static uint64_t RNG = 88172645463325252ull;
static double rnd(void){ RNG^=RNG<<13; RNG^=RNG>>7; RNG^=RNG<<17; return (RNG>>11)*(1.0/9007199254740992.0); }
static double randn(void){ double u=rnd(),v=rnd(); return sqrt(-2*log(u+1e-12))*cos(6.28318530718*v); }

/* ---- corpus + vocab --------------------------------------------------- */
static char *read_all(const char *path, size_t *len){
    FILE *f=fopen(path,"rb"); if(!f) return NULL;
    fseek(f,0,SEEK_END); long n=ftell(f); fseek(f,0,SEEK_SET);
    char *b=malloc(n+1); if(!b){fclose(f);return NULL;}
    size_t r=fread(b,1,n,f); fclose(f); b[r]=0; if(len)*len=r; return b;
}
/* concatenate every regular file under dir (one level) into one buffer */
static char *read_corpus(const char *path, size_t *outlen){
    struct stat st;
    if(stat(path,&st)==0 && S_ISREG(st.st_mode)) return read_all(path,outlen);
    DIR *d=opendir(path); if(!d) return NULL;
    size_t cap=1<<16, len=0; char *buf=malloc(cap); struct dirent *e;
    while((e=readdir(d))){
        if(e->d_name[0]=='.') continue;
        char fp[4096]; snprintf(fp,sizeof fp,"%s/%s",path,e->d_name);
        struct stat fs; if(stat(fp,&fs)!=0||!S_ISREG(fs.st_mode)) continue;
        size_t fl; char *fb=read_all(fp,&fl); if(!fb) continue;
        while(len+fl+2>cap){cap*=2; buf=realloc(buf,cap);}
        memcpy(buf+len,fb,fl); len+=fl; buf[len++]='\n'; free(fb);
    }
    closedir(d); buf[len]=0; if(outlen)*outlen=len; return buf;
}

/* byte→index vocab built from the corpus */
typedef struct { int map[256]; int inv[256]; int V; } Vocab;
static void build_vocab(const char *s, size_t n, Vocab *vo){
    int seen[256]={0}; for(size_t i=0;i<n;i++) seen[(unsigned char)s[i]]=1;
    for(int i=0;i<256;i++) vo->map[i]=-1;
    vo->V=0; for(int i=0;i<256;i++) if(seen[i]){ vo->map[i]=vo->V; vo->inv[vo->V]=i; vo->V++; }
    if(vo->V==0){ vo->map[(int)'\n']=0; vo->inv[0]='\n'; vo->V=1; }
}

/* ---- parameter blocks (flat, for a uniform Adam loop) ----------------- */
typedef struct { float *w,*g,*m,*v; int n; } Par;
#define MAXP 64
static Par PARS[MAXP]; static int NPAR=0;
static float *mkpar(int n, double scale){
    Par *p=&PARS[NPAR++]; p->n=n;
    p->w=calloc(n,sizeof(float)); p->g=calloc(n,sizeof(float));
    p->m=calloc(n,sizeof(float)); p->v=calloc(n,sizeof(float));
    for(int i=0;i<n;i++) p->w[i]=(float)(randn()*scale);
    return p->w;
}
static void zero_grads(void){ for(int i=0;i<NPAR;i++) memset(PARS[i].g,0,PARS[i].n*sizeof(float)); }
static void adam(float lr,int t){
    float b1=0.9f,b2=0.999f,eps=1e-8f;
    float c1=1.0f-(float)pow(b1,t), c2=1.0f-(float)pow(b2,t);
    for(int i=0;i<NPAR;i++){ Par*p=&PARS[i];
        for(int j=0;j<p->n;j++){ float g=p->g[j];
            p->m[j]=b1*p->m[j]+(1-b1)*g; p->v[j]=b2*p->v[j]+(1-b2)*g*g;
            float mh=p->m[j]/c1, vh=p->v[j]/c2; p->w[j]-=lr*mh/(sqrtf(vh)+eps);
        } }
}

/* ---- model ------------------------------------------------------------ */
typedef struct {
    int V,C,L;
    float *emb;                 /* V*C */
    float **Wf,**Wg,**Wr,**Ws;  /* per layer: Wf/Wg = C*C*2, Wr/Ws = C*C */
    float **bf,**bg,**br,**bs;  /* per layer biases: C each */
    float *Wh1,*bh1;            /* C*C, C */
    float *Wh2,*bh2;            /* V*C, V */
} Model;

static void model_init(Model*M,int V,int C,int L){
    M->V=V; M->C=C; M->L=L; NPAR=0;
    double s=1.0/sqrt((double)C);
    M->emb=mkpar(V*C, 1.0);
    M->Wf=malloc(L*sizeof(float*)); M->Wg=malloc(L*sizeof(float*));
    M->Wr=malloc(L*sizeof(float*)); M->Ws=malloc(L*sizeof(float*));
    M->bf=malloc(L*sizeof(float*)); M->bg=malloc(L*sizeof(float*));
    M->br=malloc(L*sizeof(float*)); M->bs=malloc(L*sizeof(float*));
    for(int l=0;l<L;l++){
        M->Wf[l]=mkpar(C*C*2,s); M->Wg[l]=mkpar(C*C*2,s);
        M->Wr[l]=mkpar(C*C,s);   M->Ws[l]=mkpar(C*C,s);
        M->bf[l]=mkpar(C,0);     M->bg[l]=mkpar(C,0);
        M->br[l]=mkpar(C,0);     M->bs[l]=mkpar(C,0);
    }
    M->Wh1=mkpar(C*C,s); M->bh1=mkpar(C,0);
    M->Wh2=mkpar(V*C,s); M->bh2=mkpar(V,0);
}

static float sigm(float x){ return 1.0f/(1.0f+expf(-x)); }

/* grad-array accessors (defined after fb; declared here) */
static float* PARS_emb_g(Model*M);
static float* PARS_Wf_g(Model*M,int l); static float* PARS_Wg_g(Model*M,int l);
static float* PARS_Wr_g(Model*M,int l); static float* PARS_Ws_g(Model*M,int l);
static float* PARS_bf_g(Model*M,int l); static float* PARS_bg_g(Model*M,int l);
static float* PARS_br_g(Model*M,int l); static float* PARS_bs_g(Model*M,int l);
static float* PARS_Wh1_g(Model*M); static float* PARS_bh1_g(Model*M);
static float* PARS_Wh2_g(Model*M); static float* PARS_bh2_g(Model*M);

/* forward+backward over one sequence of token ids (len T); returns mean loss.
 * targets[t] = ids[t+1]; the last position has no target. */
static double fb(Model*M,const int*ids,int T,int train){
    int C=M->C,L=M->L,V=M->V;
    /* activation caches */
    float *cur=calloc((size_t)(L+1)*T*C,sizeof(float)); /* cur[l][t][c] */
    float *af =calloc((size_t)L*T*C,sizeof(float));
    float *ag =calloc((size_t)L*T*C,sizeof(float));
    float *zz =calloc((size_t)L*T*C,sizeof(float));
    float *skip=calloc((size_t)T*C,sizeof(float));
    #define CUR(l,t,c) cur[(((size_t)(l)*T+(t))*C)+(c)]
    #define AF(l,t,c)  af[(((size_t)(l)*T+(t))*C)+(c)]
    #define AG(l,t,c)  ag[(((size_t)(l)*T+(t))*C)+(c)]
    #define ZZ(l,t,c)  zz[(((size_t)(l)*T+(t))*C)+(c)]
    #define SK(t,c)    skip[((size_t)(t)*C)+(c)]

    for(int t=0;t<T;t++) for(int c=0;c<C;c++) CUR(0,t,c)=M->emb[ids[t]*C+c];

    for(int l=0;l<L;l++){ int d=1<<l;
        for(int t=0;t<T;t++){
            const float*xt=&CUR(l,t,0);
            const float*xd=(t-d>=0)?&CUR(l,t-d,0):NULL;
            for(int c=0;c<C;c++){
                float vf=M->bf[l][c], vg=M->bg[l][c];
                const float*wf=&M->Wf[l][c*C*2], *wg=&M->Wg[l][c*C*2];
                for(int k=0;k<C;k++){
                    vf+=wf[k*2]*xt[k]; vg+=wg[k*2]*xt[k];
                    if(xd){ vf+=wf[k*2+1]*xd[k]; vg+=wg[k*2+1]*xd[k]; }
                }
                AF(l,t,c)=vf; AG(l,t,c)=vg;
                ZZ(l,t,c)=tanhf(vf)*sigm(vg);
            }
            /* residual + skip (1x1 convs over z) */
            for(int c=0;c<C;c++){
                float r=M->br[l][c], sk=M->bs[l][c];
                const float*wr=&M->Wr[l][c*C], *ws=&M->Ws[l][c*C];
                for(int k=0;k<C;k++){ r+=wr[k]*ZZ(l,t,k); sk+=ws[k]*ZZ(l,t,k); }
                CUR(l+1,t,c)=CUR(l,t,c)+r;   /* residual */
                SK(t,c)+=sk;                 /* accumulate skip */
            }
        }
    }

    /* head: relu(skip) -> 1x1 (relu) -> logits -> softmax CE */
    float *sR=calloc((size_t)T*C,sizeof(float));   /* relu(skip) */
    float *hd=calloc((size_t)T*C,sizeof(float));    /* relu(Wh1 sR) */
    float *hpre=calloc((size_t)T*C,sizeof(float));  /* pre-relu */
    double loss=0; int count=0;
    float *dlogit=calloc(V,sizeof(float));
    /* we need per-t backward; do forward storing, then loop again for grads */
    for(int t=0;t<T;t++){
        for(int c=0;c<C;c++) sR[t*C+c]=SK(t,c)>0?SK(t,c):0;
        for(int c=0;c<C;c++){ float a=M->bh1[c]; const float*w=&M->Wh1[c*C];
            for(int k=0;k<C;k++) a+=w[k]*sR[t*C+k]; hpre[t*C+c]=a; hd[t*C+c]=a>0?a:0; }
    }

    /* zero grads for accumulation if training */
    if(train) zero_grads();
    float *dSK=calloc((size_t)T*C,sizeof(float)); /* grad into skip accumulator */

    for(int t=0;t<T-1;t++){
        int tgt=ids[t+1];
        float logit[512]; float mx=-1e30f;
        for(int v=0;v<V;v++){ float a=M->bh2[v]; const float*w=&M->Wh2[v*C];
            for(int k=0;k<C;k++) a+=w[k]*hd[t*C+k]; logit[v]=a; if(a>mx)mx=a; }
        float sum=0; for(int v=0;v<V;v++){ logit[v]=expf(logit[v]-mx); sum+=logit[v]; }
        loss+=-log((logit[tgt]/sum)+1e-12); count++;
        if(!train) continue;
        for(int v=0;v<V;v++) dlogit[v]=logit[v]/sum - (v==tgt?1.0f:0.0f);
        /* head grads */
        float dhd[512]={0};
        for(int v=0;v<V;v++){ float dl=dlogit[v]; float*gw=&PARS_Wh2_g(M)[v*C];
            for(int k=0;k<C;k++){ gw[k]+=dl*hd[t*C+k]; dhd[k]+=dl*M->Wh2[v*C+k]; }
            PARS_bh2_g(M)[v]+=dl; }
        for(int c=0;c<C;c++){ float dpre=(hpre[t*C+c]>0)?dhd[c]:0;
            float*gw=&PARS_Wh1_g(M)[c*C];
            for(int k=0;k<C;k++){ gw[k]+=dpre*sR[t*C+k]; }
            PARS_bh1_g(M)[c]+=dpre;
            /* into sR via Wh1 */
            for(int k=0;k<C;k++) dSK[t*C+k]+=dpre*M->Wh1[c*C+k];
        }
    }
    /* relu on skip */
    for(int t=0;t<T;t++) for(int c=0;c<C;c++) if(SK(t,c)<=0) dSK[t*C+c]=0;

    if(train){
        /* backward through layers (reverse). dcur for layer l+1 starts as residual grad. */
        float *dcur=calloc((size_t)T*C,sizeof(float));   /* grad wrt CUR(l+1) */
        float *dcur_in=calloc((size_t)T*C,sizeof(float));/* grad wrt CUR(l)   */
        /* top: skip flows into every layer's z via Ws; residual chain via dcur.
           Initialize dcur as 0 (loss reaches cur only through skip and head). */
        for(int l=L-1;l>=0;l--){ int d=1<<l;
            memset(dcur_in,0,(size_t)T*C*sizeof(float));
            for(int t=0;t<T;t++){
                /* dz from residual path (dcur) and skip path (dSK) */
                float dz[512]={0};
                for(int c=0;c<C;c++){
                    /* residual: CUR(l+1,t,c) = CUR(l,t,c) + r[c]; r = Wr z */
                    float dr=dcur[t*C+c];
                    /* identity residual to lower layer */
                    dcur_in[t*C+c]+=dr;
                    /* Wr grads + dz */
                    float*gwr=&PARS_Wr_g(M,l)[c*C];
                    for(int k=0;k<C;k++){ gwr[k]+=dr*ZZ(l,t,k); dz[k]+=dr*M->Wr[l][c*C+k]; }
                    PARS_br_g(M,l)[c]+=dr;
                    /* skip path */
                    float ds=dSK[t*C+c];
                    float*gws=&PARS_Ws_g(M,l)[c*C];
                    for(int k=0;k<C;k++){ gws[k]+=ds*ZZ(l,t,k); dz[k]+=ds*M->Ws[l][c*C+k]; }
                    PARS_bs_g(M,l)[c]+=ds;
                }
                /* through gated activation z = tanh(af)*sig(ag) */
                for(int c=0;c<C;c++){
                    float th=tanhf(AF(l,t,c)), sg=sigm(AG(l,t,c));
                    float daf=dz[c]*sg*(1-th*th);
                    float dag=dz[c]*th*(sg*(1-sg));
                    /* conv backward: af = bf + sum_k Wf[c][k][0]*xt[k] + Wf[c][k][1]*xtd[k] */
                    float*gwf=&PARS_Wf_g(M,l)[c*C*2], *gwg=&PARS_Wg_g(M,l)[c*C*2];
                    PARS_bf_g(M,l)[c]+=daf; PARS_bg_g(M,l)[c]+=dag;
                    for(int k=0;k<C;k++){
                        float xt=CUR(l,t,k);
                        gwf[k*2]+=daf*xt; gwg[k*2]+=dag*xt;
                        dcur_in[t*C+k]+=daf*M->Wf[l][c*C*2+k*2]+dag*M->Wg[l][c*C*2+k*2];
                        if(t-d>=0){ float xd=CUR(l,t-d,k);
                            gwf[k*2+1]+=daf*xd; gwg[k*2+1]+=dag*xd;
                            dcur_in[(t-d)*C+k]+=daf*M->Wf[l][c*C*2+k*2+1]+dag*M->Wg[l][c*C*2+k*2+1];
                        }
                    }
                }
            }
            memcpy(dcur,dcur_in,(size_t)T*C*sizeof(float));
        }
        /* embedding grads */
        for(int t=0;t<T;t++){ float*ge=&PARS_emb_g(M)[ids[t]*C]; for(int c=0;c<C;c++) ge[c]+=dcur[t*C+c]; }
        free(dcur); free(dcur_in);
    }

    free(cur);free(af);free(ag);free(zz);free(skip);free(sR);free(hd);free(hpre);free(dlogit);free(dSK);
    #undef CUR
    #undef AF
    #undef AG
    #undef ZZ
    #undef SK
    return count?loss/count:0;
}

/* grad-array accessors (PARS order matches model_init order) */
static float* PARS_emb_g(Model*M){ (void)M; return PARS[0].g; }
/* layer l blocks start at index 1 + l*8, in order Wf,Wg,Wr,Ws,bf,bg,br,bs */
static float* PARS_Wf_g(Model*M,int l){ (void)M; return PARS[1+l*8+0].g; }
static float* PARS_Wg_g(Model*M,int l){ (void)M; return PARS[1+l*8+1].g; }
static float* PARS_Wr_g(Model*M,int l){ (void)M; return PARS[1+l*8+2].g; }
static float* PARS_Ws_g(Model*M,int l){ (void)M; return PARS[1+l*8+3].g; }
static float* PARS_bf_g(Model*M,int l){ (void)M; return PARS[1+l*8+4].g; }
static float* PARS_bg_g(Model*M,int l){ (void)M; return PARS[1+l*8+5].g; }
static float* PARS_br_g(Model*M,int l){ (void)M; return PARS[1+l*8+6].g; }
static float* PARS_bs_g(Model*M,int l){ (void)M; return PARS[1+l*8+7].g; }
static float* PARS_Wh1_g(Model*M){ return PARS[1+M->L*8+0].g; }
static float* PARS_bh1_g(Model*M){ return PARS[1+M->L*8+1].g; }
static float* PARS_Wh2_g(Model*M){ return PARS[1+M->L*8+2].g; }
static float* PARS_bh2_g(Model*M){ return PARS[1+M->L*8+3].g; }

/* ---- save / load ------------------------------------------------------ */
static void save(const char*path,Model*M,Vocab*vo){
    FILE*f=fopen(path,"wb"); if(!f){perror("save");exit(1);}
    fwrite("WNET1",1,5,f);
    fwrite(&M->V,4,1,f); fwrite(&M->C,4,1,f); fwrite(&M->L,4,1,f);
    fwrite(vo->inv,4,256,f);
    for(int i=0;i<NPAR;i++) fwrite(PARS[i].w,sizeof(float),PARS[i].n,f);
    fclose(f);
}
static int load(const char*path,Model*M,Vocab*vo){
    FILE*f=fopen(path,"rb"); if(!f) return 0;
    char mg[5]; if(fread(mg,1,5,f)!=5||memcmp(mg,"WNET1",5)){fclose(f);return 0;}
    int V,C,L; if(fread(&V,4,1,f)!=1||fread(&C,4,1,f)!=1||fread(&L,4,1,f)!=1){fclose(f);return 0;}
    int inv[256]; if(fread(inv,4,256,f)!=256){fclose(f);return 0;}
    for(int i=0;i<256;i++) vo->map[i]=-1;
    vo->V=V; for(int i=0;i<V;i++){ vo->inv[i]=inv[i]; vo->map[inv[i]]=i; }
    model_init(M,V,C,L);
    for(int i=0;i<NPAR;i++) if(fread(PARS[i].w,sizeof(float),PARS[i].n,f)!=(size_t)PARS[i].n){fclose(f);return 0;}
    fclose(f); return 1;
}

/* ---- generation ------------------------------------------------------- */
static int argmax_sample(Model*M,const int*ctx,int T,double temp){
    /* run forward (no train) and read logits at last position */
    int C=M->C,L=M->L,V=M->V;
    float *cur=calloc((size_t)(L+1)*T*C,sizeof(float));
    float *zz=calloc((size_t)L*T*C,sizeof(float));
    float *skip=calloc((size_t)T*C,sizeof(float));
    #define C2(l,t,c) cur[(((size_t)(l)*T+(t))*C)+(c)]
    #define Z2(l,t,c) zz[(((size_t)(l)*T+(t))*C)+(c)]
    #define S2(t,c) skip[((size_t)(t)*C)+(c)]
    for(int t=0;t<T;t++) for(int c=0;c<C;c++) C2(0,t,c)=M->emb[ctx[t]*C+c];
    for(int l=0;l<L;l++){ int d=1<<l;
        for(int t=0;t<T;t++){ const float*xt=&C2(l,t,0); const float*xd=(t-d>=0)?&C2(l,t-d,0):NULL;
            for(int c=0;c<C;c++){ float vf=M->bf[l][c],vg=M->bg[l][c];
                const float*wf=&M->Wf[l][c*C*2],*wg=&M->Wg[l][c*C*2];
                for(int k=0;k<C;k++){ vf+=wf[k*2]*xt[k]; vg+=wg[k*2]*xt[k]; if(xd){vf+=wf[k*2+1]*xd[k];vg+=wg[k*2+1]*xd[k];} }
                Z2(l,t,c)=tanhf(vf)*sigm(vg); }
            for(int c=0;c<C;c++){ float r=M->br[l][c],sk=M->bs[l][c];
                const float*wr=&M->Wr[l][c*C],*ws=&M->Ws[l][c*C];
                for(int k=0;k<C;k++){ r+=wr[k]*Z2(l,t,k); sk+=ws[k]*Z2(l,t,k); }
                C2(l+1,t,c)=C2(l,t,c)+r; S2(t,c)+=sk; }
        } }
    int t=T-1; float sR[1024],hd[1024];
    for(int c=0;c<C;c++) sR[c]=S2(t,c)>0?S2(t,c):0;
    for(int c=0;c<C;c++){ float a=M->bh1[c]; const float*w=&M->Wh1[c*C]; for(int k=0;k<C;k++) a+=w[k]*sR[k]; hd[c]=a>0?a:0; }
    float logit[512],mx=-1e30f;
    for(int v=0;v<V;v++){ float a=M->bh2[v]; const float*w=&M->Wh2[v*C]; for(int k=0;k<C;k++) a+=w[k]*hd[k]; logit[v]=a/(float)temp; if(logit[v]>mx)mx=logit[v]; }
    float sum=0; for(int v=0;v<V;v++){ logit[v]=expf(logit[v]-mx); sum+=logit[v]; }
    double r=rnd()*sum,acc=0; int pick=V-1;
    for(int v=0;v<V;v++){ acc+=logit[v]; if(r<=acc){pick=v;break;} }
    free(cur);free(zz);free(skip);
    #undef C2
    #undef Z2
    #undef S2
    return pick;
}

/* ---- text-as-2D-image renderer (PPM P6) ------------------------------- */
/* Lays the text into a width×height grid (row-major) and colours each cell.
 * With a model: RGB = first 3 dims of that char's learned embedding (squashed),
 * so characters the model considers similar get similar colours and repeated
 * structure (spaces, line starts) becomes visible. Without a model: grayscale by
 * byte value. This is the on-ramp to PixelCNN (masked 2D convs over this grid). */
static void render(const char*src,const char*out,int width,Model*M,Vocab*vo,int have_model){
    size_t n; char*txt=read_corpus(src,&n); if(!txt){fprintf(stderr,"render: cannot read %s\n",src);exit(1);}
    if(width<=0) width=80;
    int W=width, H=(int)((n+W-1)/W); if(H<1)H=1; if(H>4000)H=4000;
    int scale=4; /* pixel block size so it's visible */
    FILE*f=fopen(out,"wb"); if(!f){perror("render");exit(1);}
    fprintf(f,"P6\n%d %d\n255\n",W*scale,H*scale);
    unsigned char *row=malloc((size_t)W*scale*3);
    for(int gy=0; gy<H; gy++){
        for(int sy=0; sy<scale; sy++){
            for(int gx=0; gx<W; gx++){
                size_t idx=(size_t)gy*W+gx; unsigned char r=16,g=18,b=22;
                if(idx<n){ unsigned char ch=(unsigned char)txt[idx];
                    if(have_model && vo->map[ch]>=0){
                        int id=vo->map[ch]; const float*e=&M->emb[id*M->C];
                        r=(unsigned char)(127+120*tanhf(e[0]));
                        g=(unsigned char)(127+120*tanhf(M->C>1?e[1]:e[0]));
                        b=(unsigned char)(127+120*tanhf(M->C>2?e[2]:e[0]));
                    } else { r=g=b=ch; }
                }
                for(int sx=0; sx<scale; sx++){ size_t o=(size_t)(gx*scale+sx)*3; row[o]=r;row[o+1]=g;row[o+2]=b; }
            }
            fwrite(row,1,(size_t)W*scale*3,f);
        }
    }
    free(row); fclose(f); free(txt);
    fprintf(stderr,"rendered %zu chars as a %dx%d image → %s%s\n", n, W*scale, H*scale, out,
            have_model?" (coloured by learned embedding)":" (grayscale by byte)");
}

/* ---- main ------------------------------------------------------------- */
static double secs(clock_t t0){ return (double)(clock()-t0)/CLOCKS_PER_SEC; }

int main(int argc,char**argv){
    if(argc<2){ fprintf(stderr,
        "usage:\n  %s train <corpus> <model.bin> [steps] [seqlen] [channels] [layers] [lr]\n"
        "  %s gen <model.bin> [n] [seed]\n"
        "  %s render <corpus|file> <out.ppm> [width] [model.bin]\n",argv[0],argv[0],argv[0]); return 1; }

    if(!strcmp(argv[1],"train")){
        if(argc<4){fprintf(stderr,"train needs <corpus> <model.bin>\n");return 1;}
        int steps  = argc>4?atoi(argv[4]):400;
        int T      = argc>5?atoi(argv[5]):64;
        int C      = argc>6?atoi(argv[6]):24;
        int L      = argc>7?atoi(argv[7]):6;
        float lr   = argc>8?atof(argv[8]):3e-3f;
        size_t n; char*txt=read_corpus(argv[2],&n);
        if(!txt||n<(size_t)T+2){fprintf(stderr,"corpus too small\n");return 1;}
        Vocab vo; build_vocab(txt,n,&vo);
        int *ids=malloc(n*sizeof(int)); for(size_t i=0;i<n;i++) ids[i]=vo.map[(unsigned char)txt[i]];
        Model M; model_init(&M,vo.V,C,L);
        fprintf(stderr,"wavenet: V=%d C=%d L=%d receptive=%d seqlen=%d  (%zu chars)\n",
                vo.V,C,L,(1<<L),T,n);
        clock_t t0=clock(); double ema=0; int init=0;
        int seq[4096];
        for(int s=1;s<=steps;s++){
            size_t start=(size_t)(rnd()*(n-T-1));
            for(int i=0;i<T;i++) seq[i]=ids[start+i];
            double loss=fb(&M,seq,T,1);
            adam(lr,s);
            if(!init){ema=loss;init=1;} else ema=0.98*ema+0.02*loss;
            if(s%20==0||s==steps) fprintf(stderr,"  step %4d/%d  loss=%.4f ema=%.4f ppl=%.2f  %.1fs\n",
                                            s,steps,loss,ema,exp(ema),secs(t0));
        }
        save(argv[3],&M,&vo);
        fprintf(stderr,"saved → %s\n",argv[3]);
        free(txt); free(ids); return 0;
    }

    if(!strcmp(argv[1],"gen")){
        if(argc<3){fprintf(stderr,"gen needs <model.bin>\n");return 1;}
        int n=argc>3?atoi(argv[3]):200;
        const char*seed=argc>4?argv[4]:"the ";
        Model M; Vocab vo; if(!load(argv[2],&M,&vo)){fprintf(stderr,"cannot load %s\n",argv[2]);return 1;}
        int T=64; int ctx[4096]; int len=0;
        for(const char*p=seed;*p&&len<T;p++){ int id=vo.map[(unsigned char)*p]; ctx[len++]=(id>=0?id:0); }
        if(len==0){ ctx[len++]=0; }
        RNG ^= (uint64_t)time(NULL);
        for(int i=0;i<len;i++) putchar(vo.inv[ctx[i]]);
        for(int i=0;i<n;i++){
            int wlen=len<T?len:T; int *window=&ctx[len-wlen];
            /* pad to T on the left with the first token if short */
            int buf[64]; for(int j=0;j<T;j++) buf[j]= j<T-wlen? window[0] : window[j-(T-wlen)];
            int nx=argmax_sample(&M,buf,T,0.9);
            putchar(vo.inv[nx]);
            if(len<4000) ctx[len++]=nx;
        }
        putchar('\n'); return 0;
    }

    if(!strcmp(argv[1],"render")){
        if(argc<4){fprintf(stderr,"render needs <corpus|file> <out.ppm>\n");return 1;}
        int width=argc>4?atoi(argv[4]):80;
        Model M; Vocab vo; int have=0;
        if(argc>5) have=load(argv[5],&M,&vo);
        render(argv[2],argv[3],width,&M,&vo,have);
        return 0;
    }

    fprintf(stderr,"unknown command %s\n",argv[1]); return 1;
}
