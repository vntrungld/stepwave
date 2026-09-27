from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

from helpers import make_sofa
from stepwave_datagen.errors import DatagenError
from stepwave_datagen.hrtf import download, load_sofa


def test_load_resamples_to_48k(tmp_path: Path) -> None:
    path = tmp_path / "h.sofa"
    make_sofa(path, [(0, 0), (90, 0)], taps=147)
    h = load_sofa(path)
    assert h.ir.dtype == np.float32
    assert h.ir.shape == (2, 2, 160)


def test_nearest_direction(tmp_path: Path) -> None:
    path = tmp_path / "h.sofa"
    make_sofa(
        path, [(0, 0), (90, 0), (180, 0), (270, 0)], gains=[(1, 1), (1, 0.1), (1, 1), (0.1, 1)]
    )
    h = load_sofa(path)
    left = h.nearest(80.0, 10.0)
    assert left[0].sum() > left[1].sum()
    assert np.array_equal(h.nearest(355.0, 0.0), h.ir[0])
    assert np.array_equal(h.nearest(-90.0, 0.0), h.ir[3])


def test_cartesian_positions(tmp_path: Path) -> None:
    import h5py

    path = tmp_path / "c.sofa"
    make_sofa(path, [(0, 0), (90, 0)])
    with h5py.File(path, "r+") as f:
        f["SourcePosition"][...] = np.array([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]])
        f["SourcePosition"].attrs["Type"] = "cartesian"
    h = load_sofa(path)
    assert np.array_equal(h.nearest(90.0, 0.0), h.ir[1])


def test_bad_shape_rejected(tmp_path: Path) -> None:
    import h5py

    path = tmp_path / "bad.sofa"
    with h5py.File(path, "w") as f:
        f["Data.IR"] = np.zeros((2, 3, 8))
        f["Data.SamplingRate"] = np.array([48_000.0])
        f["SourcePosition"] = np.zeros((2, 3))
    with pytest.raises(DatagenError, match="two receivers"):
        load_sofa(path)


def test_download_skips_existing(tmp_path: Path) -> None:
    dest = tmp_path / "h.sofa"
    dest.write_bytes(b"x")
    assert download("http://invalid.invalid/h.sofa", dest) is False
