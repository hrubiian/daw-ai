#!/usr/bin/env python3
"""Repeatable signal descriptors; numpy + ffmpeg. No perceptual quality score."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np


def decode(path, start=0, seconds=None, sr=22050):
    cmd = ['ffmpeg', '-v', 'error', '-i', str(path), '-ss', str(start)]
    if seconds is not None:
        cmd += ['-t', str(seconds)]
    raw = subprocess.check_output(cmd + ['-f', 'f32le', '-ac', '2', '-ar', str(sr), '-'])
    return np.frombuffer(raw, dtype='<f4').reshape(-1, 2).astype(np.float64), sr


def describe(path, start=0, seconds=None):
    x, sr = decode(path, start, seconds)
    if len(x) < 4096 or not np.isfinite(x).all():
        raise ValueError('Need finite audio of at least 4096 frames')
    mono = x.mean(axis=1)
    frames = np.lib.stride_tricks.sliding_window_view(mono, 2048)[::256]
    power = abs(np.fft.rfft(frames * np.hanning(2048), axis=1)) ** 2
    freq = np.fft.rfftfreq(2048, 1 / sr)
    total = power.sum(axis=0)
    limits = [20, 80, 250, 500, 2000, 6000, 10000, 11025]
    bands = np.array([total[(freq >= a) & (freq < b)].sum() for a, b in zip(limits, limits[1:])])
    bands /= max(bands.sum(), 1e-30)
    # Positive log-spectral flux, not a claim about identified drum onsets.
    logmag = np.log1p(np.sqrt(power))
    flux = np.maximum(np.diff(logmag, axis=0), 0).mean(axis=1)
    flux -= flux.mean()
    ac = np.correlate(flux, flux, mode='full')[len(flux)-1:]
    hop_s = 256 / sr
    lo, hi = round(60 / 180 / hop_s), round(60 / 60 / hop_s)
    candidates = sorted(range(lo, min(hi, len(ac)-1)), key=lambda i: ac[i], reverse=True)
    chosen = []
    for lag in candidates:
        bpm = 60 / (lag * hop_s)
        if all(abs(bpm-v['bpm']) > 3 for v in chosen):
            chosen.append({'bpm': round(bpm, 1), 'autocorrelation': round(float(ac[lag]/max(ac[0], 1e-30)), 3)})
        if len(chosen) == 3:
            break
    windows = [x[i:i+sr] for i in range(0, len(x)-sr+1, sr)]
    rms = [float(np.sqrt(np.mean(w*w))) for w in windows]
    db = 20*np.log10(np.maximum(rms, 1e-12))
    # Candidate changes at two-second frame boundaries, separated by >=4 s.
    blocks = np.array([power[i:i+round(2/hop_s)].sum(axis=0) for i in range(0, len(power), round(2/hop_s))])
    blocks /= np.maximum(np.linalg.norm(blocks, axis=1, keepdims=True), 1e-30)
    novelty = 1 - (blocks[1:]*blocks[:-1]).sum(axis=1)
    changes = []
    for i in np.argsort(novelty)[::-1]:
        t = int(2*(i+1))
        if all(abs(t-v['seconds']) >= 4 for v in changes):
            changes.append({'seconds': t, 'spectral_change': round(float(novelty[i]), 3)})
        if len(changes) == 4:
            break
    peak = float(abs(x).max())
    return dict(path=str(path), sha256=hashlib.sha256(Path(path).read_bytes()).hexdigest(),
                start_seconds=start, duration_seconds=round(len(x)/sr, 3), sample_rate=sr,
                sample_peak_dbfs=round(float(20*np.log10(max(peak, 1e-12))), 2),
                rms_dbfs=round(float(20*np.log10(max(np.sqrt(np.mean(x*x)), 1e-12))), 2),
                clipped_decoded_samples=int((abs(x) >= 1).sum()),
                crest_db=round(float(20*np.log10(max(peak, 1e-12)/max(np.sqrt(np.mean(x*x)), 1e-12))), 2),
                side_mid_ratio=round(float(np.sqrt(np.mean(((x[:,0]-x[:,1])/2)**2))/max(np.sqrt(np.mean(mono**2)), 1e-12)), 3),
                centroid_hz=round(float((total*freq).sum()/max(total.sum(), 1e-30))),
                band_energy_percent={f'{a}-{b}':round(float(v*100),2) for a,b,v in zip(limits,limits[1:],bands)},
                one_second_rms_dbfs=[round(float(v), 2) for v in db],
                one_second_rms_p90_p10_db=round(float(np.percentile(db,90)-np.percentile(db,10)),2),
                tempo_candidates_unverified=chosen,
                excerpt_change_candidates_unverified=sorted(changes, key=lambda v:v['seconds']))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('files', nargs='+', type=Path)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--start', type=float, default=0)
    p.add_argument('--seconds', type=float)
    a = p.parse_args()
    results = [describe(f, a.start, a.seconds) for f in a.files]
    a.output.parent.mkdir(parents=True, exist_ok=True)
    a.output.write_text(json.dumps(dict(method='ffmpeg decoded stereo 22050Hz; numpy descriptors; tempo/meter/sections unverified; not a quality score', results=results), indent=2)+'\n')
    for r in results:
        print(Path(r['path']).name, {k:r[k] for k in ['duration_seconds','rms_dbfs','crest_db','centroid_hz','tempo_candidates_unverified']})


if __name__ == '__main__':
    main()
