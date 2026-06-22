"""
Cross-reference real-world engine noise against bo-controller's
synthetic test signals.

Outputs:
  - spectrogram & time-energy plots
  - dominant fundamental tracking
  - harmonic ratios
  - estimated RT60
  - comparison plots vs. synth_passby and synth_passby_saturated
"""
import sys
import numpy as np
import scipy.signal
import scipy.io.wavfile as wavfile
import json

SR_TARGET = 48000

def load_wav(path):
    sr, data = wavfile.read(path)
    if data.dtype == np.int16:
        data = data.astype(np.float32) / 32768.0
    elif data.dtype == np.int32:
        data = data.astype(np.float32) / 2147483648.0
    if data.ndim > 1:
        data = data.mean(axis=1)
    return sr, data

def goertzel(samples, freq, sr):
    """Single-bin DFT magnitude."""
    n = len(samples)
    k = round(n * freq / sr)
    omega = 2.0 * np.pi * k / n
    coef = 2.0 * np.cos(omega)
    s1 = s2 = 0.0
    for x in samples:
        s0 = float(x) + coef * s1 - s2
        s2 = s1
        s1 = s0
    real = s1 - s2 * np.cos(omega)
    imag = s2 * np.sin(omega)
    return np.sqrt(real * real + imag * imag)

def spectrogram(signal, sr, win=2048, hop=1024):
    """STFT magnitude spectrogram."""
    f, t, Zxx = scipy.signal.stft(signal, fs=sr, window='hann',
                                  nperseg=win, noverlap=win-hop)
    return f, t, np.abs(Zxx)

def estimate_fundamental_track(signal, sr, win=2048, hop=1024,
                                f_lo=50, f_hi=400):
    """Track dominant fundamental over time."""
    f, t, S = spectrogram(signal, sr, win, hop)
    # Mask to the engine band
    band_mask = (f >= f_lo) & (f <= f_hi)
    band_f = f[band_mask]
    band_S = S[band_mask, :]
    # For each frame, find the peak frequency
    peaks = band_f[np.argmax(band_S, axis=0)]
    energies = np.max(band_S, axis=0)
    return t, peaks, energies, f, S

def band_energy(signal, sr, lo, hi, n_probes=16):
    freqs = np.linspace(lo, hi, n_probes)
    total = 0.0
    n = min(len(signal), 8192)  # cap probe length
    for fq in freqs:
        mag = goertzel(signal[:n], fq, sr)
        total += mag * mag
    return np.sqrt(total / n_probes) / max(n, 1)

def estimate_rt60(signal, sr):
    """Crude RT60 estimate via Schroeder reverse integration on
    energy envelope. Looks for the steepest decay after the peak."""
    # Hilbert envelope, downsampled.
    env = np.abs(scipy.signal.hilbert(signal))
    # Smooth with 50 ms moving average.
    smooth_win = int(0.05 * sr)
    env_smooth = np.convolve(env, np.ones(smooth_win)/smooth_win, mode='valid')
    db = 20 * np.log10(env_smooth + 1e-12)
    # Find peak
    peak_idx = np.argmax(db)
    if peak_idx > len(db) - 100:
        return None
    # From peak, find where it drops 20 dB (proxy for RT60 / 3)
    threshold = db[peak_idx] - 20
    decay_idx = np.where(db[peak_idx:] < threshold)[0]
    if len(decay_idx) == 0:
        return None
    dt_samples = decay_idx[0]
    dt_secs = dt_samples / sr
    rt60_est = dt_secs * 3
    return rt60_est

def harmonic_amplitudes(signal, sr, fundamental_hz, n_harmonics=6):
    """Goertzel magnitudes at f, 2f, 3f, ...; normalized by fundamental."""
    n = min(len(signal), 8192)
    amps = []
    for n_h in range(1, n_harmonics + 1):
        f_h = fundamental_hz * n_h
        if f_h > sr / 2:
            break
        m = goertzel(signal[:n], f_h, sr)
        amps.append(m)
    if amps and amps[0] > 0:
        amps = [a / amps[0] for a in amps]
    return amps

def synth_passby(duration_s, base_hz, max_doppler, sr, seed=17):
    """Match the synth_passby in Rust code."""
    n = int(duration_s * sr)
    rng = np.random.default_rng(seed)
    out = np.zeros(n)
    phase = 0.0
    peak_t = duration_s * 0.5
    sigma = duration_s * 0.18
    for i in range(n):
        t = i / sr
        cos_arg = min(1.0, max(0.0, t / duration_s)) * np.pi
        doppler_factor = 1.0 + max_doppler * np.cos(cos_arg)
        inst_freq = base_hz * doppler_factor
        dphase = 2 * np.pi * inst_freq / sr
        engine = (np.sin(phase) + 0.5 * np.sin(2 * phase)
                  + 0.3 * np.sin(3 * phase))
        dt = t - peak_t
        env = 1.0 / (1.0 + (dt / sigma) ** 2)
        tire = (rng.random() - 0.5) * 0.05
        out[i] = (engine / 1.8) * env * 0.5 + tire * env
        phase += dphase
        if phase > 2 * np.pi: phase -= 2 * np.pi
    return out

def main():
    sr, sig = load_wav('/home/claude/work/recording.wav')
    print(f"Loaded: sr={sr}, duration={len(sig)/sr:.2f}s, "
          f"rms={np.sqrt(np.mean(sig**2)):.4f}")
    print(f"Peak amplitude: {np.max(np.abs(sig)):.4f}")

    # Detect "active" segments where energy is well above floor
    win = 2048
    hop = 512
    n_windows = (len(sig) - win) // hop
    energies = np.zeros(n_windows)
    for i in range(n_windows):
        start = i * hop
        chunk = sig[start:start + win]
        energies[i] = np.sqrt(np.mean(chunk ** 2))

    floor = np.percentile(energies, 30)
    active_mask = energies > floor * 2.5
    print(f"\nNoise floor RMS (30th percentile): {floor:.5f}")
    print(f"Active windows: {active_mask.sum()}/{n_windows} "
          f"({100*active_mask.mean():.1f}%)")

    # ── Spectrogram + fundamental track ──
    t_track, peaks, peak_energies, freqs, S = estimate_fundamental_track(
        sig, sr, win=2048, hop=1024, f_lo=50, f_hi=400)
    # Where is the energy strongest?
    print(f"\nDominant fundamental track (active frames only):")
    energy_threshold = np.percentile(peak_energies, 70)
    active = peak_energies > energy_threshold
    if active.sum() > 0:
        active_peaks = peaks[active]
        print(f"  median: {np.median(active_peaks):.1f} Hz")
        print(f"  10-90 percentile: "
              f"{np.percentile(active_peaks, 10):.1f} - "
              f"{np.percentile(active_peaks, 90):.1f} Hz")

    # ── Pick the loudest 2s window for harmonic analysis ──
    win_2s = int(2 * sr)
    hop_2s = int(0.5 * sr)
    best_rms = 0
    best_start = 0
    for start in range(0, len(sig) - win_2s, hop_2s):
        chunk = sig[start:start + win_2s]
        rms = np.sqrt(np.mean(chunk ** 2))
        if rms > best_rms:
            best_rms = rms
            best_start = start
    print(f"\nLoudest 2s window: starts at {best_start/sr:.2f}s, "
          f"rms={best_rms:.4f}")

    loud = sig[best_start:best_start + win_2s]
    # Estimate fundamental in this window.
    f_loud, _, S_loud = scipy.signal.stft(loud, fs=sr, nperseg=2048,
                                          noverlap=1536)
    band = (f_loud >= 50) & (f_loud <= 400)
    S_band = np.abs(S_loud[band, :])
    # Sum across time, find peak.
    spectrum = S_band.sum(axis=1)
    f_band = f_loud[band]
    fund_idx = np.argmax(spectrum)
    fundamental = f_band[fund_idx]
    print(f"  fundamental in loudest window: {fundamental:.1f} Hz")

    # Harmonic amplitudes
    amps = harmonic_amplitudes(loud, sr, fundamental, n_harmonics=8)
    print(f"\nHarmonic structure (loudest window, normalized to h1=1.0):")
    for i, a in enumerate(amps, 1):
        bar = '█' * int(a * 40)
        print(f"  h{i} ({fundamental*i:5.0f} Hz): {a:.3f}  {bar}")

    # ── RT60 estimate (look for a passby tail) ──
    rt60 = estimate_rt60(sig[best_start:best_start + int(4 * sr)], sr)
    if rt60:
        print(f"\nApprox RT60 (post-peak decay): {rt60*1000:.0f} ms")

    # ── Compare to synthetic ──
    synth = synth_passby(2.0, fundamental, 0.04, sr, seed=17)
    synth_amps = harmonic_amplitudes(synth, sr, fundamental, n_harmonics=8)
    print(f"\nSynthetic comparison (synth_passby at {fundamental:.0f} Hz):")
    print(f"{'harmonic':>10} {'real':>10} {'synth':>10} {'delta':>10}")
    for i, (r, s_amp) in enumerate(zip(amps, synth_amps), 1):
        delta = r - s_amp
        flag = '!' if abs(delta) > 0.2 else ''
        print(f"  h{i:<2}     {r:>10.3f} {s_amp:>10.3f} {delta:>+10.3f}  {flag}")

    # ── Spectrogram image ──
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt

    fig, axes = plt.subplots(4, 1, figsize=(11, 13))

    # Plot 1: full waveform with active mask
    t_full = np.arange(len(sig)) / sr
    axes[0].plot(t_full, sig, color='#6080a0', linewidth=0.4)
    axes[0].axvspan(best_start/sr, (best_start + win_2s)/sr,
                    color='#d090c0', alpha=0.25,
                    label=f'loudest 2s ({best_start/sr:.1f}s)')
    axes[0].set_xlabel('Time (s)')
    axes[0].set_ylabel('Amplitude')
    axes[0].set_title('Recording waveform')
    axes[0].legend()
    axes[0].grid(alpha=0.3)
    axes[0].set_facecolor('#0a0c0e')

    # Plot 2: spectrogram (log-scale magnitude)
    db = 20 * np.log10(np.abs(S) + 1e-9)
    extent = [0, len(sig)/sr, freqs[0], freqs[-1]]
    im = axes[1].imshow(db, aspect='auto', origin='lower',
                       extent=extent, cmap='inferno',
                       vmin=db.max()-60, vmax=db.max())
    axes[1].set_ylim(0, 2000)
    axes[1].set_xlabel('Time (s)')
    axes[1].set_ylabel('Frequency (Hz)')
    axes[1].set_title('Spectrogram (0-2 kHz)')
    plt.colorbar(im, ax=axes[1], label='dB')

    # Plot 3: fundamental track
    axes[2].plot(t_track, peaks, color='#80c080', linewidth=1)
    axes[2].axhline(fundamental, color='#d090c0', linestyle='--',
                   label=f'loudest-window fundamental: {fundamental:.0f} Hz')
    axes[2].fill_between(t_track, peaks - 5, peaks + 5, alpha=0.2,
                        color='#80c080')
    axes[2].set_xlabel('Time (s)')
    axes[2].set_ylabel('Dominant frequency (Hz)')
    axes[2].set_ylim(50, 400)
    axes[2].set_title('Dominant fundamental track (50-400 Hz band)')
    axes[2].legend()
    axes[2].grid(alpha=0.3)
    axes[2].set_facecolor('#0a0c0e')

    # Plot 4: harmonic comparison
    n_h = min(len(amps), len(synth_amps))
    x = np.arange(1, n_h + 1)
    width = 0.35
    axes[3].bar(x - width/2, amps[:n_h], width, color='#80c080',
               label='Real recording')
    axes[3].bar(x + width/2, synth_amps[:n_h], width, color='#c08080',
               label=f'synth_passby ({fundamental:.0f} Hz)')
    axes[3].set_xlabel('Harmonic')
    axes[3].set_ylabel('Amplitude (normalized to h1)')
    axes[3].set_title('Harmonic structure: real vs. synth_passby')
    axes[3].legend()
    axes[3].grid(alpha=0.3, axis='y')
    axes[3].set_xticks(x)
    axes[3].set_facecolor('#0a0c0e')

    fig.patch.set_facecolor('#0e1012')
    for ax in axes:
        ax.tick_params(colors='#888c91')
        for spine in ax.spines.values():
            spine.set_color('#2a2e33')
        ax.title.set_color('#d4d4d8')
        ax.xaxis.label.set_color('#888c91')
        ax.yaxis.label.set_color('#888c91')

    plt.tight_layout()
    plt.savefig('/home/claude/work/real_vs_synth.png', dpi=110,
                facecolor=fig.get_facecolor())
    print("\nSaved plot to /home/claude/work/real_vs_synth.png")

    # ── JSON summary ──
    summary = {
        'sample_rate': sr,
        'duration_s': len(sig) / sr,
        'rms': float(np.sqrt(np.mean(sig**2))),
        'peak_amplitude': float(np.max(np.abs(sig))),
        'noise_floor_rms': float(floor),
        'active_fraction': float(active_mask.mean()),
        'fundamental_track_median_hz': float(np.median(active_peaks))
            if active.sum() > 0 else None,
        'fundamental_track_p10_hz': float(np.percentile(active_peaks, 10))
            if active.sum() > 0 else None,
        'fundamental_track_p90_hz': float(np.percentile(active_peaks, 90))
            if active.sum() > 0 else None,
        'loudest_2s_start_s': float(best_start / sr),
        'loudest_2s_rms': float(best_rms),
        'loudest_2s_fundamental_hz': float(fundamental),
        'real_harmonic_amps': [float(a) for a in amps],
        'synth_harmonic_amps': [float(a) for a in synth_amps],
        'rt60_estimate_ms': float(rt60 * 1000) if rt60 else None,
    }
    with open('/home/claude/work/analysis_summary.json', 'w') as f:
        json.dump(summary, f, indent=2)
    print("Saved summary to /home/claude/work/analysis_summary.json")

if __name__ == '__main__':
    main()
