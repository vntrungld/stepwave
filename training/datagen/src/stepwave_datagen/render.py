"""Render a Scene into one stereo stem per class."""

from __future__ import annotations

from collections.abc import Callable

import numpy as np
from scipy.signal import butter, fftconvolve, sosfilt

from . import CLASSES, SR
from .config import RenderConfig
from .hrtf import Hrtf
from .scene import Scene


def distance_gain(d: float) -> float:
    return 1.0 / max(d, 0.5)


def lpf_cutoff_hz(d: float, cfg: RenderConfig) -> float:
    return float(
        np.clip(cfg.lpf_max_hz * (1.0 / max(d, 1e-3)) ** 0.5, cfg.lpf_min_hz, cfg.lpf_max_hz)
    )


def _lowpass(x: np.ndarray, cutoff_hz: float, cfg: RenderConfig) -> np.ndarray:
    if cutoff_hz >= cfg.lpf_max_hz:
        return x
    sos = butter(4, cutoff_hz, fs=SR, output="sos")
    return sosfilt(sos, x).astype(np.float32)


def synthetic_rir(rt60_s: float, seed: int) -> np.ndarray:
    """Decorrelated stereo noise with a 60 dB exponential decay over rt60_s, unit energy."""
    n = max(int(rt60_s * SR), 1)
    t = np.arange(n) / SR
    env = 10.0 ** (-3.0 * t / rt60_s)
    ir = np.random.default_rng(seed).standard_normal((2, n)) * env
    ir /= np.sqrt((ir**2).sum(axis=1, keepdims=True))
    return ir.astype(np.float32)


def active_rms(x: np.ndarray, floor_db: float = -60.0) -> float:
    """RMS over samples whose envelope is within floor_db of the peak (silence excluded)."""
    env = np.abs(x).max(axis=0)
    peak = float(env.max()) if env.size else 0.0
    if peak <= 0.0:
        return 0.0
    mask = env > peak * 10.0 ** (floor_db / 20.0)
    return float(np.sqrt(np.mean(x[:, mask].astype(np.float64) ** 2)))


def _place(buf: np.ndarray, y: np.ndarray, onset: int) -> None:
    n = buf.shape[1]
    if onset >= n:
        return
    end = min(n, onset + y.shape[1])
    buf[:, onset:end] += y[:, : end - onset]


def render_stems(
    scene: Scene, load: Callable[[str], np.ndarray], hrtf: Hrtf, cfg: RenderConfig
) -> dict[str, np.ndarray]:
    n = int(round(scene.clip_seconds * SR))
    dry = {c: np.zeros((2, n), np.float64) for c in CLASSES}
    send = {c: np.zeros((2, n), np.float64) for c in CLASSES}
    for ev in scene.events:
        x = np.asarray(load(ev.source), dtype=np.float32)
        if ev.cls == "ambience":
            idx = (int(ev.offset_s * SR) + np.arange(n)) % len(x)
            x = x[idx]
        x = _lowpass(x * distance_gain(ev.distance_m), lpf_cutoff_hz(ev.distance_m, cfg), cfg)
        if ev.spatial:
            h = hrtf.nearest(ev.azimuth_deg, ev.elevation_deg)
            y = np.stack([fftconvolve(x, h[0]), fftconvolve(x, h[1])])
        else:
            y = np.stack([x, x])
        onset = int(round(ev.onset_s * SR))
        _place(dry[ev.cls], (1.0 - ev.wet) * y, onset)
        if ev.wet > 0.0:
            _place(send[ev.cls], ev.wet * y, onset)
    rir = synthetic_rir(scene.rt60_s, scene.rir_seed)
    stems: dict[str, np.ndarray] = {}
    for c in CLASSES:
        stem = dry[c]
        if np.any(send[c]):
            stem = stem + np.stack([fftconvolve(send[c][ch], rir[ch])[:n] for ch in (0, 1)])
        stems[c] = stem.astype(np.float32)
    _apply_footstep_snr(stems, scene.footstep_snr_db)
    return stems


def _apply_footstep_snr(stems: dict[str, np.ndarray], snr_db: float | None) -> None:
    if snr_db is None:
        return
    steps = active_rms(stems["footsteps"])
    rest = active_rms(sum(stems[c] for c in CLASSES if c != "footsteps"))
    if steps == 0.0 or rest == 0.0:
        return
    scale = 10.0 ** (snr_db / 20.0) * rest / steps
    stems["footsteps"] = (stems["footsteps"] * scale).astype(np.float32)
