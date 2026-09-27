"""Clip folders: six FLAC files plus meta.json (written last, marks the clip complete)."""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import soundfile as sf

from . import CLASSES, SR


def write_clip(clip_dir: Path, mix: np.ndarray, stems: dict[str, np.ndarray], meta: dict) -> None:
    clip_dir.mkdir(parents=True, exist_ok=True)
    for name, data in (("mix", mix), *((c, stems[c]) for c in CLASSES)):
        sf.write(clip_dir / f"{name}.flac", data.T, SR, subtype="PCM_24", format="FLAC")
    tmp = clip_dir / "meta.json.tmp"
    tmp.write_text(json.dumps(meta, indent=2, sort_keys=True))
    tmp.replace(clip_dir / "meta.json")


def is_complete(clip_dir: Path) -> bool:
    return (clip_dir / "meta.json").is_file()


def _read(path: Path) -> np.ndarray:
    data, _ = sf.read(path, dtype="float32", always_2d=True)
    return np.ascontiguousarray(data.T)


def read_clip(clip_dir: Path) -> tuple[np.ndarray, dict[str, np.ndarray], dict]:
    mix = _read(clip_dir / "mix.flac")
    stems = {c: _read(clip_dir / f"{c}.flac") for c in CLASSES}
    return mix, stems, json.loads((clip_dir / "meta.json").read_text())
