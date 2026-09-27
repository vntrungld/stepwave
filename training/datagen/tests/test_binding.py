from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pytest
import stepwave_py

FIXTURE = Path(__file__).resolve().parents[3] / "core/tests/fixtures/features_sine_1k.json"


def rust_sine(freq_hz: float, amplitude: float, n: int) -> np.ndarray:
    """Bit-compatible with stepwave_core::testing::sine."""
    idx = np.arange(n, dtype=np.float64)
    return np.float32(amplitude) * np.sin(2 * np.pi * freq_hz * idx / 48_000.0).astype(np.float32)


def test_constants() -> None:
    assert (stepwave_py.SAMPLE_RATE, stepwave_py.WIN, stepwave_py.HOP) == (48_000, 960, 480)
    assert (stepwave_py.BINS, stepwave_py.NUM_BANDS) == (481, 32)
    centres = stepwave_py.erb_centres_hz()
    assert centres.shape == (32,) and centres.dtype == np.float32
    assert np.all(np.diff(centres) > 0)


def test_matches_rust_fixture() -> None:
    data = json.loads(FIXTURE.read_text())
    sig = data["signal"]
    x = rust_sine(sig["freq_hz"], sig["amplitude"], sig["len"])
    got = stepwave_py.band_energies_db(x, x)
    want = np.asarray(data["frames"], dtype=np.float32)
    assert got.shape == want.shape == (10, 32)
    assert got.dtype == np.float32
    np.testing.assert_allclose(got, want, atol=1e-3)


def test_sine_peaks_in_nearest_band() -> None:
    x = rust_sine(1000.0, 0.5, 4800)
    rows = stepwave_py.band_energies_db(x, x)
    centres = stepwave_py.erb_centres_hz()
    assert int(np.argmax(rows[-1])) == int(np.argmin(np.abs(centres - 1000.0)))


def test_mismatched_lengths_raise() -> None:
    with pytest.raises(ValueError):
        stepwave_py.band_energies_db(np.zeros(960, np.float32), np.zeros(480, np.float32))


REPO = Path(__file__).resolve().parents[3]


def noise(seed: int, n: int, amplitude: float) -> np.ndarray:
    return np.random.default_rng(seed).uniform(-amplitude, amplitude, n).astype(np.float32)


def rms(x: np.ndarray) -> float:
    return float(np.sqrt(np.mean(np.asarray(x, np.float64) ** 2)))


def test_apply_zero_gains_reproduces_input() -> None:
    x, y = noise(1, 48_000, 0.3), noise(2, 48_000, 0.3)
    frames = x.size // stepwave_py.HOP
    lo, ro = stepwave_py.apply_band_gains(x, y, np.zeros((frames, 32), np.float32))
    assert lo.shape == x.shape and ro.shape == y.shape and lo.dtype == np.float32
    mid = slice(960, -960)
    for out, ref in ((lo, x), (ro, y)):
        assert 20 * np.log10(rms(out[mid] - ref[mid]) / rms(ref[mid])) < -90


def test_apply_six_db_everywhere() -> None:
    x = noise(3, 96_000, 0.05)
    frames = x.size // stepwave_py.HOP
    gains = np.full((frames, 32), 6.0, np.float32)
    lo, _ = stepwave_py.apply_band_gains(x, x, gains, limiter=False)
    mid = slice(4800, -4800)
    assert abs(20 * np.log10(rms(lo[mid]) / rms(x[mid])) - 6.0) < 0.1


def test_apply_rejects_bad_shapes() -> None:
    x = np.zeros(4800, np.float32)
    good = np.zeros((10, 32), np.float32)
    with pytest.raises(ValueError):
        stepwave_py.apply_band_gains(x, x[:2400], good)
    with pytest.raises(ValueError):
        stepwave_py.apply_band_gains(x, x, np.zeros((9, 32), np.float32))
    with pytest.raises(ValueError):
        stepwave_py.apply_band_gains(x, x, np.zeros((10, 31), np.float32))


def test_smooth_gains_uses_core_time_constants() -> None:
    g = np.zeros((20, 32), np.float32)
    g[1:10] = 6.0
    s = stepwave_py.smooth_gains_db(g)
    assert s.shape == g.shape and s.dtype == np.float32
    attack, release = np.exp(-10 / 5), np.exp(-10 / 80)
    assert s[0, 0] == 0.0
    np.testing.assert_allclose(s[1, 0], 6.0 * (1 - attack), rtol=1e-5)
    np.testing.assert_allclose(s[10, 0], s[9, 0] * release, rtol=1e-5)


def test_static_eq_gains_follow_profile() -> None:
    g = stepwave_py.static_eq_gains_db((REPO / "profiles/cs2.json").read_text())
    assert g.shape == (32,) and g.dtype == np.float32
    assert g[0] < -6.0  # lowshelf −4 dB plus preamp −4 dB at the lowest band
    centres = stepwave_py.erb_centres_hz()
    assert g[int(np.argmin(np.abs(centres - 2500)))] > g[0]
    with pytest.raises(ValueError):
        stepwave_py.static_eq_gains_db("{}")
