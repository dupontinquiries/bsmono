"""master_take -- cut a recording session into the game's voice lines,
clean them, give each ancestor its own vocal chain, level and encode them.

    python3 master_take.py SESSION.opus RNNOISE_MODEL OUT_AUDIO_ROOT [voice ...]

SESSION is the performer's take (anything ffmpeg reads). RNNOISE_MODEL is an
RNNoise model for ffmpeg's arnndn (used: GregorR/rnnoise-models,
somnolent-hogwash-2018-09-01/sh.rnnn). Files land at
OUT_AUDIO_ROOT/voice/<ancestor>/<key>.opus -- the paths in
recording_sheet.tsv, which the game loads (soundbank.rs) and
tests/voice_assets.rs checks.

TAKES holds, per line, the window of the session the chosen take lies in
(seconds). For clawbound-males-kessler-take001 they were found with a speech
recogniser (sherpa-onnx, NVIDIA parakeet-tdt-0.6b-v2) over silero-VAD
segments; the last clean take of each line in Kessler's first pass through
each ancestor was chosen. Needs ffmpeg with rubberband and arnndn, numpy,
soundfile, pyloudnorm.
"""
import subprocess, numpy as np, soundfile as sf, pyloudnorm as pyln, os, csv, json, sys, tempfile
SR = 48000
SESSION, RNN, ROOT = (os.path.abspath(a) for a in sys.argv[1:4])
ONLY = sys.argv[4:]
HERE = os.path.dirname(os.path.abspath(__file__))
os.chdir(tempfile.mkdtemp(prefix='master_take_'))
subprocess.run(['ffmpeg', '-v', 'error', '-y', '-i', SESSION, '-ac', '1', '-ar', str(SR), 'take48.wav'], check=True)
TAKE = 'take48.wav'
# (voice, key, start, end): a window a little wider than the take
TAKES = [
 ('goldenmane','select',38.2,41.6),('goldenmane','cast1',41.7,43.9),('goldenmane','cast2',44.6,47.3),
 ('goldenmane','cast3',48.6,51.1),('goldenmane','cast4',52.9,56.3),('goldenmane','kill.1',58.5,61.2),
 ('goldenmane','kill.2',69.8,72.3),('goldenmane','death.1',89.2,92.4),('goldenmane','death.2',103.0,106.2),
 ('oldroar','select',407.2,411.0),('oldroar','cast1',402.1,404.3),('oldroar','cast2',221.4,223.1),
 ('oldroar','cast3',232.8,235.8),('oldroar','cast4',250.0,253.6),('oldroar','kill.1',281.4,284.1),
 ('oldroar','kill.2',343.1,346.4),('oldroar','death.1',385.7,388.6),('oldroar','death.2',381.0,384.3),
 ('twofang','select',499.2,504.2),('twofang','cast1',504.6,506.0),('twofang','cast2',507.2,509.7),
 ('twofang','cast3',512.3,514.4),('twofang','cast4',516.2,519.4),('twofang','kill.1',521.9,524.4),
 ('twofang','kill.2',531.6,535.0),('twofang','death.1',538.6,541.5),('twofang','death.2',557.0,559.7),
 ('windclaw','select',672.9,675.8),('windclaw','cast1',681.0,682.8),('windclaw','cast2',691.2,693.0),
 ('windclaw','cast3',643.5,645.0),('windclaw','cast4',703.7,707.0),('windclaw','kill.1',721.9,724.5),
 ('windclaw','kill.2',729.0,731.3),('windclaw','death.1',744.2,747.0),('windclaw','death.2',753.0,755.2),
]
# Clean-up shared by all: rumble out, clicks out, a speech denoiser (RNNoise,
# 'somnolent hogwash' general model), then a light spectral gate.
CLEAN = f"highpass=f=75,adeclick,arnndn=m={RNN}:mix=0.85,afftdn=nr=8:nf=-60"
# Each ancestor's chain (after cleaning). rubberband keeps formants for the
# smaller shifts and lets them follow for Oldroar's, so he sounds bigger,
# not just lower.
CHAINS = {
 # warm, commanding baritone: a touch lower; a clean low-mid boom under it
 # (thunder, not grit -- no saturation anywhere in this chain); a three-voice
 # chorus for the pride behind him; presence; a hall
 'goldenmane': "rubberband=pitch=0.944:formant=preserved,"
               "equalizer=f=110:t=q:w=0.8:g=4,equalizer=f=200:t=q:w=0.9:g=3.5,equalizer=f=450:t=q:w=1.4:g=-2.5,"
               "equalizer=f=2800:t=q:w=1.2:g=2.5,"
               "chorus=0.7:0.9:22|37|53:0.35|0.3|0.25:0.23|0.37|0.51:1.6|2.1|2.6,"
               "acompressor=threshold=-24dB:ratio=3:attack=10:release=150:makeup=2,"
               "aecho=0.8:0.55:38|71:0.22|0.14",
 # vast, slow, gravelly: down a major third with the formants, slowed; a
 # deep sub under the voice; the grit kept low in the spectrum and the top
 # rolled off after it (the saturation's harmonics were the fizz up top)
 'oldroar':    "rubberband=pitch=0.84:tempo=0.94:formant=shifted,"
               "asubboost=dry=1:wet=0.9:decay=0.6:feedback=0.5:cutoff=90:slope=0.6,"
               "equalizer=f=48:t=q:w=0.8:g=5,equalizer=f=90:t=q:w=1:g=4,equalizer=f=2200:t=q:w=1.2:g=1.5,"
               "asoftclip=type=tanh:threshold=0.85:output=1,"
               "lowpass=f=5200:p=2,lowpass=f=5200,equalizer=f=3800:t=q:w=1.5:g=-3,deesser=i=0.5:f=0.4,"
               "acompressor=threshold=-26dB:ratio=4:attack=15:release=250:makeup=2,"
               "aecho=0.85:0.5:90|170|260:0.20|0.13|0.08",
 # clipped, deadly: a little lower, dry and close, bitten mids, hard comp,
 # and a viper's rattle: a fast tremolo with a flicker of vibrato;
 # almost no room
 'twofang':    "rubberband=pitch=0.917:formant=preserved,"
               "highpass=f=110,equalizer=f=250:t=q:w=1:g=-3,equalizer=f=1800:t=q:w=1.0:g=4,equalizer=f=5500:t=q:w=1:g=2,"
               "asoftclip=type=atan:threshold=0.8,"
               "acompressor=threshold=-28dB:ratio=6:attack=3:release=60:makeup=3,"
               "aecho=0.9:0.3:14:0.06",  # + viper() below
 # bright, fast, a little wild: up, quicker, air, a breath of wind; a
 # small room only
 'windclaw':   "rubberband=pitch=1.12:tempo=1.06:formant=preserved,"
               "highpass=f=140,equalizer=f=300:t=q:w=1:g=-2,treble=g=5:f=6000,"
               "aexciter=level_in=1:level_out=1:amount=1.5:drive=6:blend=0:freq=5000,"
               "chorus=0.8:0.9:28:0.35:0.4:1.8,"
               "acompressor=threshold=-24dB:ratio=3:attack=5:release=90:makeup=2,"
               "aecho=0.8:0.35:24|47:0.06|0.035",
}

def viper(y):
    """Twofang's rattle: a fast tremolo (13 Hz, 45% deep) and a flicker of
    vibrato (6.5 Hz, +-0.35 ms of delay, about +-14 cents). Done here, not
    with ffmpeg's tremolo/vibrato, whose start-up spike (-14 dBFS, 20 ms)
    came out louder than the line and threw the trim."""
    t = np.arange(len(y)) / SR
    trem = 1.0 - 0.45 * (0.5 + 0.5 * np.sin(2 * np.pi * 13.0 * t))
    d = 0.00035 * SR * (1.0 + np.sin(2 * np.pi * 6.5 * t))
    idx = np.clip(np.arange(len(y)) - d, 0, len(y) - 1)
    i0 = np.floor(idx).astype(int); i1 = np.minimum(i0 + 1, len(y) - 1); fr = idx - i0
    return ((y[i0] * (1 - fr) + y[i1] * fr) * trem).astype(np.float32)

POST = {'twofang': viper}

# The god and the beast under each voice, laid in BEFORE the chain so the
# chain's pitch, EQ, compression and room glue them to the voice (beast.py).
sys.path.insert(0, HERE)
import beast as B


def goldenmane_layers(c, key):
    # a second, octave-down self speaking with him (the god), and a lion's
    # growl in his throat -- a thread of it always, swelling on the shouts
    low = B.at(-9, B.octave(c, 0.5), c)
    g = B.cross(c, B.growl_carrier(len(c), f0=62, jitter=0.15, double=0.3, seed=11))
    gate = B.loud_gate(c, 14) ** 1.5
    return c + low + B.at(-9, g, c) * (0.3 + 1.2 * gate)


def oldroar_layers(c, key):
    # older and deeper: the octave-down self louder, and a slow cave-lion
    # rumble (a 38 Hz growl, very irregular) under every word
    low = B.at(-5, B.octave(c, 0.5), c)
    g = B.cross(c, B.growl_carrier(len(c), f0=38, jitter=0.28, double=0.5, seed=12))
    gate = B.loud_gate(c, 18)
    return c + low + B.at(-6, B.lowpass(g, 900), c) * (0.5 + 0.8 * gate)


def twofang_layers(c, key):
    # the sabertooth hisses through its words (the voice's envelope on
    # noise, only the top), snarls on its loud ones, and the lines it has
    # time for trail off in a hiss
    hiss = B.bandpass(B.cross(c, B.noise(len(c), seed=13)), 2800, 9000)
    g = B.cross(c, B.growl_carrier(len(c), f0=92, jitter=0.3, double=0.45, seed=14))
    gate = B.loud_gate(c, 10) ** 2
    y = c + B.at(-12, hiss, c) + B.at(-6, g, c) * gate
    if not key.startswith('cast'):
        n = int(0.45 * SR)
        tail = B.bandpass(B.noise(n, seed=15), 2500, 8000)
        tail *= np.exp(-np.arange(n) / (0.12 * SR)) * np.minimum(1, np.arange(n) / (0.02 * SR))
        y = np.concatenate([y, B.at(-8, tail, c) * 1.6])
    return y


def windclaw_layers(c, key):
    # a caracal of the sky: a breath of wind through the words, a faint
    # fifth-up spirit voice, and its chirrup -- before the select call,
    # after a kill (the laugh in it), and on "Up, up!"
    wind = B.bandpass(B.cross(c, B.noise(len(c), seed=16)), 500, 3000)
    hi = B.octave(c, 1.5)
    y = c + B.at(-16, wind, c) + B.at(-17, hi, c)
    ch = B.chirp()
    ch = ch * (np.abs(c).max() * 0.55)
    gap = np.zeros(int(0.05 * SR), np.float32)
    if key == 'select':
        y = np.concatenate([ch, gap, y])
    elif key.startswith('kill') or key == 'cast3':
        up = B.chirp(0.13, 620, 1250, seed=4) * (np.abs(c).max() * 0.5)
        y = np.concatenate([y, gap, up])
    return y.astype(np.float32)


PRE = {'goldenmane': goldenmane_layers, 'oldroar': oldroar_layers, 'twofang': twofang_layers, 'windclaw': windclaw_layers}
sheet = {}
for row in csv.DictReader(open(os.path.join(HERE, 'recording_sheet.tsv')), delimiter='\t'):
    sheet[row['file']] = row
meter = pyln.Meter(SR, block_size=0.2)
def ff(inp, out, af):
    subprocess.run(['ffmpeg','-v','error','-y','-i',inp,'-af',af,'-ar',str(SR),'-ac','1',out], check=True)
def trim(x, pre=0.06, post=0.18, floor_db=-38, start_db=None):
    h = int(0.01*SR)
    env = np.array([np.sqrt((x[i:i+h]**2).mean()+1e-12) for i in range(0, len(x)-h, h)])
    db = 20*np.log10(env+1e-12); on = np.where(db > db.max()+floor_db)[0]
    st = np.where(db > db.max()+(start_db if start_db is not None else floor_db))[0][0]
    if start_db is not None:
        # the line proper starts where the sound first comes within 20 dB of
        # its peak; walk back from there through the continuous rise (a soft
        # first syllable, a plosive's burst), not across a gap to a stray
        # breath or click before it
        main = np.where(db > db.max()-20)[0][0]
        k = main
        while k > 0 and db[k-1] > db.max()+start_db:
            k -= 1
        st = k
        # and the end the same way, forward from the line's last loud frame
        # through its decay (gaps of up to 30 ms bridged), not on to a breath
        # or a noise after it
        last = np.where(db > db.max()-20)[0][-1]
        k, gap = last, 0
        while k + 1 < len(db) and gap <= 3:
            k += 1
            gap = 0 if db[k] > db.max() + floor_db + 5 else gap + 1
        on = np.array([k - gap])
    a = max(0, st*h - int(pre*SR)); b = min(len(x), (on[-1]+1)*h + int(post*SR))
    y = x[a:b].copy(); fi, fo = int(0.01*SR), int(0.06*SR)
    y[:fi] *= np.linspace(0,1,fi); y[-fo:] *= np.linspace(1,0,fo)
    return y
full, _ = sf.read(TAKE, dtype='float32')
report = []
for who, key, s, e in TAKES:
    if ONLY and who not in ONLY: continue
    rel = f'voice/{who}/{key}.opus'
    energy = int(sheet[rel]['energy'])
    raw = full[int((s-0.5)*SR):int((e+0.5)*SR)]
    sf.write('tmp_raw.wav', raw, SR)
    ff('tmp_raw.wav', 'tmp_clean.wav', CLEAN)
    c, _ = sf.read('tmp_clean.wav', dtype='float32')
    c = c[int(0.5*SR):len(c)-int(0.5*SR)]
    c = trim(c)
    if who in PRE:
        c = PRE[who](c, key).astype(np.float32)
        c /= max(1.0, float(np.abs(c).max()) / 0.9)
    sf.write('tmp_cut.wav', c, SR)
    # the chain, then room for the tail to ring out
    ff('tmp_cut.wav', 'tmp_fx.wav', "apad=pad_dur=0.5," + CHAINS[who])
    y, _ = sf.read('tmp_fx.wav', dtype='float32')
    if who in POST:
        y = POST[who](y)
    y = trim(y, pre=0.09, post=0.25, floor_db=-45, start_db=-36)
    # level: -18 LUFS at energy 3, 1.5 LU a step up or down the sheet's scale
    target = -18.0 + 1.5*(energy-3)
    lufs = meter.integrated_loudness(np.pad(y, (0, max(0, int(0.45*SR)-len(y)))))
    y = y * 10**((target-lufs)/20)
    sf.write('tmp_lvl.wav', y, SR)
    os.makedirs(os.path.join(ROOT, 'voice', who), exist_ok=True)
    dst = os.path.join(ROOT, rel)
    ff('tmp_lvl.wav', 'tmp_lim.wav', "alimiter=limit=0.79:attack=2:release=40:level=disabled")
    subprocess.run(['ffmpeg','-v','error','-y','-i','tmp_lim.wav','-c:a','libopus','-b:a','64k','-application','audio','-ar','48000','-ac','1',dst], check=True)
    z, _ = sf.read('tmp_lim.wav', dtype='float32')
    report.append({'file': rel, 'line': sheet[rel]['line'], 'from_s': s, 'to_s': e, 'secs': round(len(z)/SR,2), 'lufs': round(meter.integrated_loudness(np.pad(z,(0,max(0,int(0.45*SR)-len(z))))),1), 'peak_db': round(20*np.log10(np.abs(z).max()),1), 'bytes': os.path.getsize(dst)})
    print(report[-1], flush=True)
json.dump(report, open(os.path.join(ROOT, 'voice', 'master_report.json'), 'w'), indent=1, default=float)
