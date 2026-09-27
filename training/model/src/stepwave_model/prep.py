"""`trainer prep`: per-band energies of every clip's mix and stems, stored as float16.

Layout of `<features>/`:
  manifest.json           constants, per-split source fingerprints and sizes,
                          train-only mix mean/std per band, and a fingerprint of all that
  <split>/<signal>.npy    float16 [frames, 32] for each of SIGNALS, clips concatenated
  <split>/index.npy       int64 [clips, 2]: frame offset, frame count
  <split>/clips.json      clip ids in index order
"""

from __future__ import annotations

import hashlib
import json
import shutil
from collections.abc import Callable, Iterable, Iterator
from concurrent.futures import ProcessPoolExecutor
from dataclasses import dataclass
from pathlib import Path
from typing import Any, TypeVar

import numpy as np
from tqdm import tqdm

from . import HOP, NUM_BANDS, SAMPLE_RATE, SIGNALS
from .errors import TrainerError

SPLITS = ("train", "val")
STD_FLOOR = 1e-3
CONSTANTS = {"sample_rate": SAMPLE_RATE, "hop": HOP, "num_bands": NUM_BANDS}

T = TypeVar("T")
R = TypeVar("R")


def _binding() -> tuple[Any, Any]:
    """soundfile and stepwave_py, imported lazily: the training box has neither."""
    try:
        import soundfile
        import stepwave_py
    except ImportError as err:
        raise TrainerError(
            "prep/eval need the audio extra: run `uv sync --extra audio` in training/model"
        ) from err
    have = (stepwave_py.SAMPLE_RATE, stepwave_py.HOP, stepwave_py.NUM_BANDS)
    if have != (SAMPLE_RATE, HOP, NUM_BANDS):
        raise TrainerError(f"stepwave_py constants {have} differ from stepwave_model; rebuild it")
    return soundfile, stepwave_py


def map_jobs(fn: Callable[[T], R], items: Iterable[T], workers: int) -> Iterator[R]:
    """Ordered map, in-process for workers <= 1, else over a process pool."""
    if workers <= 1:
        yield from map(fn, items)
        return
    with ProcessPoolExecutor(workers) as pool:
        yield from pool.map(fn, items, chunksize=4)


def read_stereo(path: Path) -> np.ndarray:
    """[2, n] float32 at 48 kHz; mono is duplicated."""
    sf, _ = _binding()
    data, sr = sf.read(path, dtype="float32", always_2d=True)
    if sr != SAMPLE_RATE:
        raise TrainerError(f"{path}: sample rate {sr}, expected {SAMPLE_RATE}")
    if data.shape[1] == 1:
        data = np.repeat(data, 2, axis=1)
    if data.shape[1] != 2:
        raise TrainerError(f"{path}: {data.shape[1]} channels, expected 1 or 2")
    return np.ascontiguousarray(data.T)


def clip_energies(clip_dir: Path) -> dict[str, np.ndarray]:
    _, sw = _binding()
    out = {}
    for s in SIGNALS:
        x = read_stereo(clip_dir / f"{s}.flac")
        out[s] = sw.band_energies_db(x[0], x[1]).astype(np.float16)
    return out


def _write_json(path: Path, data: Any) -> None:
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(data, indent=2, sort_keys=True))
    tmp.replace(path)


def _build_split(clips: list[Path], dest: Path, workers: int, desc: str) -> int:
    sf, _ = _binding()
    counts = [sf.info(str(c / "mix.flac")).frames // HOP for c in clips]
    offsets = np.concatenate([[0], np.cumsum(counts)[:-1]]).astype(np.int64)
    total = int(sum(counts))
    partial = dest.with_name(dest.name + ".partial")
    shutil.rmtree(partial, ignore_errors=True)
    partial.mkdir(parents=True)
    arrays = {
        s: np.lib.format.open_memmap(
            partial / f"{s}.npy", mode="w+", dtype=np.float16, shape=(total, NUM_BANDS)
        )
        for s in SIGNALS
    }
    results = map_jobs(clip_energies, clips, workers)
    for clip, off, n, e in tqdm(
        zip(clips, offsets, counts, results, strict=True), total=len(clips), desc=desc
    ):
        for s in SIGNALS:
            if e[s].shape[0] != n:
                raise TrainerError(f"{clip}: {s} has {e[s].shape[0]} frames, mix has {n}")
            arrays[s][off : off + n] = e[s]
    for a in arrays.values():
        a.flush()
    del arrays
    np.save(partial / "index.npy", np.stack([offsets, np.asarray(counts, np.int64)], axis=1))
    (partial / "clips.json").write_text(json.dumps([c.name for c in clips]))
    shutil.rmtree(dest, ignore_errors=True)
    partial.rename(dest)
    return total


def _mix_stats(split_dir: Path) -> tuple[list[float], list[float]]:
    mix = np.load(split_dir / "mix.npy", mmap_mode="r")
    s = np.zeros(NUM_BANDS)
    s2 = np.zeros(NUM_BANDS)
    for i in range(0, mix.shape[0], 1_000_000):
        chunk = np.asarray(mix[i : i + 1_000_000], dtype=np.float64)
        s += chunk.sum(0)
        s2 += (chunk * chunk).sum(0)
    n = max(mix.shape[0], 1)
    mean = s / n
    std = np.maximum(np.sqrt(np.maximum(s2 / n - mean**2, 0.0)), STD_FLOOR)
    return mean.tolist(), std.tolist()


def _fingerprint(manifest: dict[str, Any]) -> str:
    key = {k: manifest.get(k) for k in ("constants", "splits", "feature")}
    return hashlib.sha256(json.dumps(key, sort_keys=True).encode()).hexdigest()[:16]


def prep_set(
    data_dir: Path,
    set_name: str,
    out_dir: Path | None = None,
    workers: int = 1,
    force: bool = False,
    log: Callable[[str], None] = print,
) -> Path:
    """Build (or resume) the feature arrays for sets/<set_name>; returns the features dir."""
    _binding()
    src = data_dir / "sets" / set_name
    src_manifest = src / "manifest.json"
    if not src_manifest.is_file():
        raise TrainerError(
            f"no set at {src}: run `datagen mix {set_name} --split train` and `--split val`"
        )
    src_splits = json.loads(src_manifest.read_text()).get("splits", {})
    out = out_dir or data_dir / "features" / set_name
    out.mkdir(parents=True, exist_ok=True)
    mpath = out / "manifest.json"
    manifest: dict[str, Any] = json.loads(mpath.read_text()) if mpath.is_file() else {}
    if manifest and manifest.get("constants") != CONSTANTS:
        if not force:
            raise TrainerError(f"{out} was built with other constants; re-run with --force")
        manifest = {}
    manifest.update({"set": set_name, "constants": CONSTANTS})
    manifest.setdefault("splits", {})
    for split in SPLITS:
        entry = src_splits.get(split)
        if entry is None:
            raise TrainerError(
                f"set {set_name} has no {split} split: run `datagen mix {set_name} --split {split}`"
            )
        fingerprint = entry["fingerprint"]
        have = manifest["splits"].get(split)
        dest = out / split
        up_to_date = have and have.get("source_fingerprint") == fingerprint
        if not force and up_to_date and (dest / "index.npy").is_file():
            log(f"{split}: up to date, skipped")
            continue
        clips = sorted(d for d in (src / split).iterdir() if (d / "meta.json").is_file())
        if not clips:
            raise TrainerError(f"{src / split} has no complete clips")
        log(f"{split}: {len(clips)} clips")
        frames = _build_split(clips, dest, workers, desc=split)
        manifest["splits"][split] = {
            "source_fingerprint": fingerprint,
            "clips": len(clips),
            "frames": frames,
        }
        if split == "train":
            mean, std = _mix_stats(dest)
            manifest["feature"] = {"mean": mean, "std": std}
        manifest["fingerprint"] = _fingerprint(manifest)
        _write_json(mpath, manifest)
    return out


@dataclass(frozen=True)
class Split:
    energies: dict[str, np.ndarray]
    index: np.ndarray
    clips: list[str]

    def clip(self, i: int) -> dict[str, np.ndarray]:
        off, n = (int(v) for v in self.index[i])
        return {s: self.energies[s][off : off + n] for s in SIGNALS}

    @property
    def frames(self) -> int:
        return int(self.index[:, 1].sum())


def load_manifest(features_dir: Path) -> dict[str, Any]:
    path = features_dir / "manifest.json"
    if not path.is_file():
        raise TrainerError(f"no features at {features_dir}: run `trainer prep` first")
    manifest = json.loads(path.read_text())
    if manifest.get("constants") != CONSTANTS:
        raise TrainerError(f"{features_dir}: built with other constants; re-run prep --force")
    if "feature" not in manifest:
        raise TrainerError(f"{features_dir}: no train mean/std yet; prep the train split")
    return manifest


def load_split(features_dir: Path, split: str, mmap: bool = True) -> Split:
    d = features_dir / split
    if not (d / "index.npy").is_file():
        raise TrainerError(f"missing split {split!r} in {features_dir}: run `trainer prep`")
    mode = "r" if mmap else None
    energies = {s: np.load(d / f"{s}.npy", mmap_mode=mode) for s in SIGNALS}
    return Split(energies, np.load(d / "index.npy"), json.loads((d / "clips.json").read_text()))
