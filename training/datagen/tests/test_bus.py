from __future__ import annotations

from pathlib import Path

import numpy as np

from stepwave_datagen import CLASSES, SR
from stepwave_datagen.bus import apply_bus, compressor_gain
from stepwave_datagen.config import load_config

CFG = load_config(Path(__file__).resolve().parents[1] / "configs/cs2.toml")


def stems_from(seed: int, amp: float = 0.3) -> dict[str, np.ndarray]:
    rng = np.random.default_rng(seed)
    return {
        c: (rng.uniform(-amp, amp, (2, 2 * SR)) * (i + 1) / 5).astype(np.float32)
        for i, c in enumerate(CLASSES)
    }


def test_compressor_leaves_quiet_signal_alone() -> None:
    quiet = np.full((2, SR), 10 ** (-30 / 20), dtype=np.float32)
    np.testing.assert_allclose(compressor_gain(quiet, CFG.bus), 1.0, atol=1e-6)


def test_compressor_reduces_loud_signal_by_ratio() -> None:
    loud = np.full((2, SR), 10 ** (-6 / 20), dtype=np.float32)  # 12 dB over threshold
    g = compressor_gain(loud, CFG.bus)
    settled = 20 * np.log10(g[-1])
    assert abs(settled - (-12.0 * (1 - 1 / 3.0))) < 0.1


def test_stems_sum_to_mix_and_ceiling_holds() -> None:
    for seed in range(5):
        mix, stems = apply_bus(stems_from(seed), CFG.bus, -15.0)
        assert tuple(stems) == CLASSES
        total = np.sum(np.stack([stems[c] for c in CLASSES]), axis=0)
        np.testing.assert_array_equal(total, mix)
        assert np.abs(mix).max() <= 10 ** (CFG.bus.peak_ceiling_dbfs / 20)
        assert mix.dtype == np.float32


def test_loudness_target_before_compression_is_approached() -> None:
    mix, _ = apply_bus(stems_from(1, amp=0.01), CFG.bus, -35.0)
    rms_db = 20 * np.log10(np.sqrt(np.mean(mix.astype(np.float64) ** 2)))
    assert abs(rms_db - (-35.0)) < 1.0


def test_bus_silent_input_stays_finite() -> None:
    zeros = {c: np.zeros((2, SR), np.float32) for c in CLASSES}
    mix, stems = apply_bus(zeros, CFG.bus, -20.0)
    assert np.all(mix == 0) and all(np.all(s == 0) for s in stems.values())
