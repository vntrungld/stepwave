"""Synthetic fixtures shared by tests (no game assets)."""

from __future__ import annotations

from pathlib import Path

import h5py
import numpy as np


def make_sofa(
    path: Path,
    positions: list[tuple[float, float]],
    gains: list[tuple[float, float]] | None = None,
    sr: float = 44_100.0,
    taps: int = 32,
) -> None:
    """SOFA-shaped HDF5: one impulse per ear at sample 0, scaled by `gains[i] = (left, right)`."""
    gains = gains or [(1.0, 1.0)] * len(positions)
    ir = np.zeros((len(positions), 2, taps))
    for i, (gl, gr) in enumerate(gains):
        ir[i, 0, 0] = gl
        ir[i, 1, 0] = gr
    pos = np.array([[az, el, 1.2] for az, el in positions])
    path.parent.mkdir(parents=True, exist_ok=True)
    with h5py.File(path, "w") as f:
        f["Data.IR"] = ir
        f["Data.SamplingRate"] = np.array([sr])
        ds = f.create_dataset("SourcePosition", data=pos)
        ds.attrs["Type"] = "spherical"
