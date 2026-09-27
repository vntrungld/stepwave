"""Game-like mix bus: loudness, compression and a peak ceiling, applied identically to stems."""

from __future__ import annotations

import numpy as np

from . import CLASSES
from .config import BusConfig

_BLOCK = 48  # 1 ms at 48 kHz


def compressor_gain(mix: np.ndarray, cfg: BusConfig) -> np.ndarray:
    """Stereo-linked feed-forward compressor gain per sample, from 1 ms block peaks."""
    n = mix.shape[1]
    blocks = -(-n // _BLOCK)
    padded = np.zeros((2, blocks * _BLOCK), np.float32)
    padded[:, :n] = np.abs(mix)
    peaks = padded.reshape(2, blocks, _BLOCK).max(axis=(0, 2))
    level_db = 20.0 * np.log10(np.maximum(peaks, 1e-9))
    target = -np.maximum(level_db - cfg.threshold_dbfs, 0.0) * (1.0 - 1.0 / cfg.ratio)
    attack = np.exp(-1.0 / cfg.attack_ms)
    release = np.exp(-1.0 / cfg.release_ms)
    reduction = np.empty(blocks)
    state = 0.0
    for i, t in enumerate(target):
        coef = attack if t < state else release
        state = t + (state - t) * coef
        reduction[i] = state
    centres = (np.arange(blocks) + 0.5) * _BLOCK
    gain_db = np.interp(np.arange(n), centres, reduction)
    return (10.0 ** (gain_db / 20.0)).astype(np.float32)


def _sum(stems: dict[str, np.ndarray]) -> np.ndarray:
    return np.sum(np.stack([stems[c] for c in CLASSES]), axis=0, dtype=np.float32)


def apply_bus(
    stems: dict[str, np.ndarray], cfg: BusConfig, loudness_rms_dbfs: float
) -> tuple[np.ndarray, dict[str, np.ndarray]]:
    mix0 = _sum(stems)
    rms = float(np.sqrt(np.mean(mix0.astype(np.float64) ** 2)))
    if rms == 0.0:
        return mix0, {c: stems[c].copy() for c in CLASSES}
    scale = 10.0 ** (loudness_rms_dbfs / 20.0) / rms
    gain = compressor_gain(mix0 * scale, cfg) * np.float32(scale)
    ceiling = 10.0 ** (cfg.peak_ceiling_dbfs / 20.0)
    out = {c: (stems[c] * gain).astype(np.float32) for c in CLASSES}
    mix = _sum(out)
    peak = float(np.abs(mix).max())
    if peak > ceiling:
        fix = np.float32(ceiling / peak * 0.999)
        out = {c: (out[c] * fix).astype(np.float32) for c in CLASSES}
        mix = _sum(out)
    return mix, out
