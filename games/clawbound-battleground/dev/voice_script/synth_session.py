"""synth_session -- stand-in voices for the ancestors nobody has recorded yet.

    python3 synth_session.py KOKORO_DIR OUT_DIR [ancestor ...]

Speaks every line of each unrecorded ancestor (recording_sheet.tsv) with
the Kokoro-82M text-to-speech model (sherpa-onnx's kokoro-multi-lang-v1_0
release: model.onnx, voices.bin, tokens.txt, espeak-ng-data, dict,
lexicon-*.txt), one Kokoro speaker per ancestor, cast from the sheet's
voice notes and each ancestor's pronouns, into one session file with
silence between the lines. It writes OUT_DIR/session.wav and
OUT_DIR/takes.json, the manifest master_take.py takes in place of a
recording:

    python3 master_take.py OUT_DIR/takes.json OUT_AUDIO_ROOT

so the stand-ins go through the same layers, chains, levels and checks as
a real take, and a real session later replaces them file for file.
"""
import csv
import json
import os
import re
import sys

import numpy as np
import soundfile as sf

HERE = os.path.dirname(os.path.abspath(__file__))
SR = 48000

# ancestor -> (Kokoro speaker, speed). She/her and he/him from the sheets.
# Speakers are from the upper grades of Kokoro's own voice notes (VOICES.md
# in hexgrad/Kokoro-82M grades each by its training data: af_heart A,
# af_bella A-, bf_emma/af_nicole B-, the C+ group af_aoede, af_kore,
# am_fenrir, am_michael, am_puck); the D-grade ones first cast here
# (af_river, bf_alice, am_onyx) were swapped out -- they were
# the flattest and the most garbled under the chains. Speeds stay at 0.9 or
# above: below that Kokoro's durations stretch vowels unevenly, which read
# as a machine drawling.
# (Riptail, Stonewhisper, Rainspots and Faryowl are recorded now: Wendy's
# session, wendy_take001.json. Their stand-ins are gone from the cast so a
# re-run cannot overwrite the performance.)
CAST = {
    # a tiger: low, unhurried, close -- British, slowed
    'duskstripe': ('bf_emma', 0.92),
    # the moonless woods: hushed, breathy
    'nightpelt': ('af_nicole', 0.9),
    # radiant, ceremonial: the warmest voice in the model
    'suneye': ('af_heart', 0.92),
    # quiet authority, cold and clear
    'snowhush': ('af_kore', 0.95),
    # serene, faintly amused
    'stillsmoke': ('bm_george', 0.9),
    # a young daredevil: bright, quick
    'cometail': ('am_puck', 1.08),
    # a kitten god: a young voice, pitched up with its formants in
    # master_take (sproutkit's chain) until it is small
    'sproutkit': ('am_liam', 1.1),
    # (Hollowstep is recorded: Nelson's session, nelson_take001.json)
    # the lamp-keeper: a big warm voice (a stand-in for all but his
    # bellow, which Nelson recorded: nelson_bellow001.json)
    'brasshowl': ('am_michael', 1.0),
}


def speakable(text):
    # all-caps words are shouted, not spelled; "--" is a beat
    text = re.sub(r'\b([A-Z]{2,})\b', lambda m: m.group(1).capitalize(), text)
    return text.replace('--', ',')


def words(s):
    return re.sub(r"[^a-z' ]", ' ', s.lower()).split()


def edit_distance(a, b):
    d = list(range(len(b) + 1))
    for i in range(1, len(a) + 1):
        p, d[0] = d[:], i
        for j in range(1, len(b) + 1):
            d[j] = min(p[j] + 1, d[j - 1] + 1, p[j - 1] + (a[i - 1] != b[j - 1]))
    return d[-1]


def asr():
    """(samples at SR, line) -> (word errors, what was heard), with the
    Parakeet recogniser from $PARAKEET_DIR (sherpa-onnx's
    nemo-parakeet-tdt-0.6b-v2-int8). Without it every take scores 0 and
    the base speed is kept."""
    d = os.environ.get('PARAKEET_DIR')
    if not d:
        return lambda x, line: (0, '')
    import sherpa_onnx
    from scipy.signal import resample_poly
    d = d.rstrip('/') + '/'
    rec = sherpa_onnx.OfflineRecognizer.from_transducer(
        encoder=d + 'encoder.int8.onnx', decoder=d + 'decoder.int8.onnx', joiner=d + 'joiner.int8.onnx',
        tokens=d + 'tokens.txt', model_type='nemo_transducer', num_threads=2)

    def hear(x, line):
        s = rec.create_stream()
        s.accept_waveform(16000, resample_poly(x, 1, SR // 16000).astype(np.float32))
        rec.decode_stream(s)
        return edit_distance(words(line), words(s.result.text)), s.result.text
    return hear


def main():
    kdir, out = sys.argv[1], sys.argv[2]
    only = sys.argv[3:]
    import sherpa_onnx
    d = kdir.rstrip('/') + '/'
    cfg = sherpa_onnx.OfflineTtsConfig(
        model=sherpa_onnx.OfflineTtsModelConfig(
            kokoro=sherpa_onnx.OfflineTtsKokoroModelConfig(
                model=d + 'model.onnx', voices=d + 'voices.bin', tokens=d + 'tokens.txt',
                data_dir=d + 'espeak-ng-data', dict_dir=d + 'dict',
                lexicon=d + 'lexicon-us-en.txt,' + d + 'lexicon-gb-en.txt'),
            num_threads=2),
        max_num_sentences=1)
    tts = sherpa_onnx.OfflineTts(cfg)
    import onnxruntime as ort
    meta = ort.InferenceSession(d + 'model.onnx', providers=['CPUExecutionProvider']).get_modelmeta().custom_metadata_map
    sid = {name: i for i, name in enumerate(meta['speaker_names'].split(','))}

    rows = [r for r in csv.DictReader(open(os.path.join(HERE, 'recording_sheet.tsv')), delimiter='\t')
            if r['voice'] in CAST and (not only or r['voice'] in only)]
    gap = np.zeros(int(1.5 * SR), np.float32)
    parts, takes, t = [gap], [], len(gap) / SR
    heard = asr()
    from scipy.signal import resample_poly
    for r in rows:
        speaker, speed = CAST[r['voice']]
        # Kokoro is deterministic for a text, speaker and speed, so the
        # takes differ in speed: a few percent either way moves where the
        # model puts its stresses and pauses. Keep the take a recogniser
        # hears as the line (fewest word errors), the base speed on a tie.
        best = None
        for k, sp in enumerate((1.0, 0.96, 1.04, 0.92)):
            a = tts.generate(speakable(r['line']), sid=sid[speaker], speed=speed * sp)
            x = np.asarray(a.samples, np.float32)
            # 24 kHz -> 48 kHz with a proper low-pass (np.interp here before
            # left an image of the whole voice folded above 12 kHz, a hiss
            # the chains' treble boosts then lifted)
            x = resample_poly(x, SR // 1000, a.sample_rate // 1000).astype(np.float32)
            errs, said = heard(x, r['line'])
            if best is None or errs < best[0]:
                best = (errs, k, x, said)
            if errs == 0:
                break
        errs, k, x, said = best
        key = r['file'].split('/')[-1][:-len('.opus')]
        takes.append([r['voice'], key, round(t - 0.2, 3), round(t + len(x) / SR + 0.2, 3)])
        parts += [x, gap]
        t += (len(x) + len(gap)) / SR
        print(r['voice'], key, speaker, f'take {k}, {errs} word errors ({said!r}), {len(x) / SR:.2f} s', flush=True)
    os.makedirs(out, exist_ok=True)
    session = os.path.join(out, 'session.wav')
    sf.write(session, np.concatenate(parts), SR)
    json.dump({'session': os.path.abspath(session), 'clean': False, 'source': 'kokoro-82M (sherpa-onnx kokoro-multi-lang-v1_0)',
               'cast': CAST, 'takes': takes}, open(os.path.join(out, 'takes.json'), 'w'), indent=1)


if __name__ == '__main__':
    main()
