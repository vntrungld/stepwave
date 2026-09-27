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
