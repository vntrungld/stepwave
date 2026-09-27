"""HRTF loading from SOFA (HDF5) files and nearest-direction lookup."""

from __future__ import annotations

import urllib.request
from dataclasses import dataclass
from math import gcd
from pathlib import Path

import h5py
import numpy as np
from scipy.signal import resample_poly

from . import SR
from .errors import DatagenError


def unit_vectors(azimuth_deg: np.ndarray, elevation_deg: np.ndarray) -> np.ndarray:
    az = np.radians(np.asarray(azimuth_deg, dtype=np.float64))
    el = np.radians(np.asarray(elevation_deg, dtype=np.float64))
    return np.stack([np.cos(el) * np.cos(az), np.cos(el) * np.sin(az), np.sin(el)], axis=-1)


@dataclass(frozen=True)
class Hrtf:
    directions: np.ndarray  # (M, 3) unit vectors
    ir: np.ndarray  # (M, 2, N) float32 at 48 kHz

    def nearest(self, azimuth_deg: float, elevation_deg: float) -> np.ndarray:
        v = unit_vectors(np.array(azimuth_deg), np.array(elevation_deg))
        return self.ir[int(np.argmax(self.directions @ v))]


def load_sofa(path: Path) -> Hrtf:
    if not path.is_file():
        raise DatagenError(f"HRTF file not found: {path}; run `datagen hrtf` first")
    with h5py.File(path, "r") as f:
        ir = np.asarray(f["Data.IR"], dtype=np.float64)
        sr = float(np.asarray(f["Data.SamplingRate"]).ravel()[0])
        pos_ds = f["SourcePosition"]
        pos = np.asarray(pos_ds, dtype=np.float64)
        kind = pos_ds.attrs.get("Type", b"spherical")
    kind = kind.decode() if isinstance(kind, bytes) else str(kind)
    if ir.ndim != 3 or ir.shape[1] != 2:
        raise DatagenError(f"{path}: expected Data.IR shaped (M, 2, N) with two receivers")
    if kind.lower() == "cartesian":
        directions = pos / np.linalg.norm(pos, axis=1, keepdims=True)
    else:
        directions = unit_vectors(pos[:, 0], pos[:, 1])
    rate = round(sr)
    if rate != SR:
        g = gcd(SR, rate)
        ir = resample_poly(ir, SR // g, rate // g, axis=-1)
    return Hrtf(directions, ir.astype(np.float32))


def download(url: str, dest: Path) -> bool:
    if dest.is_file():
        return False
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".part")
    try:
        urllib.request.urlretrieve(url, tmp)
    except OSError as err:
        raise DatagenError(f"could not download HRTF from {url}: {err}") from err
    tmp.replace(dest)
    return True
