from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

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
        mix, stems, _ = apply_bus(stems_from(seed), CFG.bus, -15.0)
        assert tuple(stems) == CLASSES
        total = np.sum(np.stack([stems[c] for c in CLASSES]), axis=0)
        np.testing.assert_array_equal(total, mix)
        assert np.abs(mix).max() <= 10 ** (CFG.bus.peak_ceiling_dbfs / 20)
        assert mix.dtype == np.float32


def _rms_db(x: np.ndarray) -> float:
    return float(20 * np.log10(np.sqrt(np.mean(x.astype(np.float64) ** 2))))


@pytest.mark.parametrize("target", [-35.0, -25.0, -20.0, -15.0])
@pytest.mark.parametrize("seed", range(3))
def test_final_loudness_hits_target_even_when_compressed(seed: int, target: float) -> None:
    stems = stems_from(seed)  # loud: pre-scaled peaks sit well above the threshold
    mix0 = np.sum(np.stack([stems[c] for c in CLASSES]), axis=0)
    pre = mix0 * np.float32(10 ** ((target - _rms_db(mix0)) / 20))
    if target >= -25.0:
        assert compressor_gain(pre, CFG.bus).min() < 10 ** (-1 / 20)  # compression engaged
    mix, _, info = apply_bus(stems, CFG.bus, target)
    final = _rms_db(mix)
    assert abs(info["mix_rms_dbfs"] - final) < 1e-3
    ceiling_bound = np.abs(mix).max() >= 0.99 * 10 ** (CFG.bus.peak_ceiling_dbfs / 20)
    if ceiling_bound:
        assert final < target
    else:
        assert abs(final - target) < 1.0
    assert info["bus_scale"] > 0.0


def test_quiet_input_hits_target() -> None:
    mix, _, info = apply_bus(stems_from(1, amp=0.01), CFG.bus, -35.0)
    assert abs(_rms_db(mix) - (-35.0)) < 1.0
    assert info["bus_scale"] > 1.0


def test_bus_silent_input_stays_finite() -> None:
    zeros = {c: np.zeros((2, SR), np.float32) for c in CLASSES}
    mix, stems, info = apply_bus(zeros, CFG.bus, -20.0)
    assert np.all(mix == 0) and all(np.all(s == 0) for s in stems.values())
    assert info == {"mix_rms_dbfs": None, "bus_scale": 1.0}
