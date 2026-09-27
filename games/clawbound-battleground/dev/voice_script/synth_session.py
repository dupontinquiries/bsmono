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
CAST = {
    # a tiger: low, unhurried, close -- British, slowed
    'duskstripe': ('bf_emma', 0.88),
    # a leopard: easy, fluid, amused
    'rainspots': ('af_bella', 1.0),
    # the moonless woods: hushed, breathy
    'nightpelt': ('af_nicole', 0.9),
    # projected, singing, theatrical
    'faryowl': ('af_aoede', 0.95),
    # radiant, ceremonial
    'suneye': ('bf_isabella', 0.9),
    # quiet authority, cold and clear
    'snowhush': ('af_river', 0.95),
    # a grey ghost of the crags: dry, sparse
    'stonewhisper': ('bf_alice', 0.88),
    # a jaguar of the drowned forest: heavy, deep
    'riptail': ('am_onyx', 0.88),
    # serene, faintly amused
    'stillsmoke': ('bm_george', 0.9),
    # a young daredevil: bright, quick
    'cometail': ('am_puck', 1.08),
    # a kitten god: a young voice, pitched up with its formants in
    # master_take (sproutkit's chain) until it is small
    'sproutkit': ('am_liam', 1.1),
}


def speakable(text):
    # all-caps words are shouted, not spelled; "--" is a beat
    text = re.sub(r'\b([A-Z]{2,})\b', lambda m: m.group(1).capitalize(), text)
    return text.replace('--', ',')


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
    for r in rows:
        speaker, speed = CAST[r['voice']]
        a = tts.generate(speakable(r['line']), sid=sid[speaker], speed=speed)
        x = np.asarray(a.samples, np.float32)
        # 24 kHz -> 48 kHz
        x = np.interp(np.arange(0, len(x), a.sample_rate / SR), np.arange(len(x)), x).astype(np.float32)
        key = r['file'].split('/')[-1][:-len('.opus')]
        takes.append([r['voice'], key, round(t - 0.2, 3), round(t + len(x) / SR + 0.2, 3)])
        parts += [x, gap]
        t += (len(x) + len(gap)) / SR
        print(r['voice'], key, speaker, f'{len(x) / SR:.2f} s', flush=True)
    os.makedirs(out, exist_ok=True)
    session = os.path.join(out, 'session.wav')
    sf.write(session, np.concatenate(parts), SR)
    json.dump({'session': os.path.abspath(session), 'clean': False, 'source': 'kokoro-82M (sherpa-onnx kokoro-multi-lang-v1_0)',
               'cast': CAST, 'takes': takes}, open(os.path.join(out, 'takes.json'), 'w'), indent=1)


if __name__ == '__main__':
    main()
