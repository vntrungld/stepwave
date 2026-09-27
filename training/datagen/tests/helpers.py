"""Synthetic fixtures shared by tests (no game assets)."""

from __future__ import annotations

import dataclasses
from pathlib import Path

import h5py
import numpy as np
import soundfile as sf

from stepwave_datagen.catalog import CatalogEntry, processed_path, write_catalog
from stepwave_datagen.config import Config, load_config


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


TINY = [
    ("sounds/footsteps/concrete/1.wav", "footsteps", "concrete", "", 0.15),
    ("sounds/footsteps/concrete/2.wav", "footsteps", "concrete", "", 0.15),
    ("sounds/footsteps/wood/1.wav", "footsteps", "wood", "", 0.15),
    ("sounds/ambient/dust2/bed.wav", "ambience", "", "dust2", 1.0),
    ("sounds/ambient/mirage/bed.wav", "ambience", "", "mirage", 1.0),
    ("sounds/weapons/ak.wav", "gunfire", "", "", 0.2),
    ("sounds/weapons/he.wav", "explosions", "", "", 0.5),
    ("sounds/ui/click.wav", "other", "", "", 0.1),
]


def make_tiny_data(data_dir: Path, cfg_path: Path) -> Config:
    raw = data_dir / "raw"
    entries = []
    for i, (rel, cls, surface, map_, seconds) in enumerate(TINY):
        x = np.random.default_rng(i).uniform(-0.3, 0.3, int(seconds * 48_000)).astype(np.float32)
        out = processed_path(raw, rel)
        out.parent.mkdir(parents=True, exist_ok=True)
        sf.write(out, x, 48_000, subtype="PCM_24", format="FLAC")
        entries.append(CatalogEntry(rel, cls, surface, map_, seconds, -10.5, -15.0))
    write_catalog(entries, data_dir / "catalog.csv")
    cfg = load_config(cfg_path)
    make_sofa(
        data_dir / "hrtf" / cfg.hrtf.file,
        [(0, 0), (90, 0), (180, 0), (270, 0)],
        gains=[(1, 1), (1, 0.2), (1, 1), (0.2, 1)],
    )
    return dataclasses.replace(
        cfg,
        scene=dataclasses.replace(cfg.scene, clip_seconds=2.0),
        split=dataclasses.replace(cfg.split, holdout_maps=("mirage",), holdout_surfaces=("wood",)),
    )
