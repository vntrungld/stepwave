"""Generate a dataset split: seeded scenes → rendered stems → mix bus → clip folders."""

from __future__ import annotations

import dataclasses
import hashlib
import json
import shutil
import subprocess
from collections import OrderedDict
from concurrent.futures import ProcessPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import numpy as np
import soundfile as sf
from tqdm import tqdm

from .bus import apply_bus
from .catalog import catalog_sha256, processed_path, read_catalog
from .config import Config, config_to_dict
from .errors import DatagenError
from .hrtf import Hrtf, load_sofa
from .render import active_rms, render_stems
from .scene import Pools, clip_id, clip_seed, make_pools, sample_scene
from .writer import is_complete, write_clip


class AudioLoader:
    """Reads processed 48 kHz mono sources, keeping the most recent ones in memory."""

    def __init__(self, raw_dir: Path, capacity: int = 2048) -> None:
        self.raw_dir = raw_dir
        self.capacity = capacity
        self._cache: OrderedDict[str, np.ndarray] = OrderedDict()

    def __call__(self, rel: str) -> np.ndarray:
        if rel in self._cache:
            self._cache.move_to_end(rel)
            return self._cache[rel]
        data, _ = sf.read(processed_path(self.raw_dir, rel), dtype="float32")
        self._cache[rel] = data
        if len(self._cache) > self.capacity:
            self._cache.popitem(last=False)
        return data


@dataclass
class GenerateResult:
    written: int
    skipped: int
    failed: list[tuple[str, str]] = field(default_factory=list)


def make_clip(
    pools: Pools,
    load: AudioLoader,
    hrtf: Hrtf,
    cfg: Config,
    seed: int,
    cid: str,
    split: str,
    set_name: str,
) -> tuple[np.ndarray, dict[str, np.ndarray], dict[str, Any]]:
    scene = sample_scene(pools, cfg.scene, cfg.render, cfg.bus, seed, cid)
    stems = render_stems(scene, load, hrtf, cfg.render)
    mix, stems, bus_info = apply_bus(stems, cfg.bus, scene.loudness_rms_dbfs)
    meta = dataclasses.asdict(scene)
    meta.update(bus_info)
    meta.update(
        {
            "set": set_name,
            "split": split,
            "footstep_snr_db_definition": "pre-bus active RMS ratio",
            "footstep_snr_db_final": _final_snr_db(stems, scene.footstep_snr_db),
            "footstep_snr_db_final_definition": "post-bus active RMS ratio of the written stems",
        }
    )
    return mix, stems, meta


def _final_snr_db(stems: dict[str, np.ndarray], target: float | None) -> float | None:
    """Footsteps vs everything else in the stems as written (same active-RMS definition)."""
    if target is None:
        return None
    steps = active_rms(stems["footsteps"])
    rest = active_rms(sum(stems[c] for c in stems if c != "footsteps"))
    if steps == 0.0 or rest == 0.0:
        return None
    return float(20.0 * np.log10(steps / rest))


_STATE: dict[str, Any] = {}


def split_fingerprint(cfg: Config, catalog_sha: str, seed: int, clips: int) -> str:
    """Identity of everything that determines a split's clips (the set name aside)."""
    payload = {
        "config": config_to_dict(cfg),
        "catalog_sha256": catalog_sha,
        "seed": seed,
        "clips": clips,
    }
    return hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()


def _check_fingerprint(data_dir: Path, name: str, split: str, fingerprint: str) -> None:
    """Refuse to resume a split that was started with different settings."""
    out = data_dir / "sets" / name / split
    manifest = data_dir / "sets" / name / "manifest.json"
    found: set[str | None] = set()
    if manifest.is_file():
        entry = json.loads(manifest.read_text()).get("splits", {}).get(split)
        if entry is not None:
            found.add(entry.get("fingerprint"))
    if out.is_dir():
        for meta in out.glob("*/meta.json"):
            found.add(json.loads(meta.read_text()).get("fingerprint"))
    if found - {fingerprint}:
        raise DatagenError(
            f"sets/{name}/{split} was generated with different settings (config, catalog, "
            f"seed or hours); re-run with --force to delete and regenerate that split, "
            f"or use a new set name"
        )


def _init(
    data_dir: Path, cfg: Config, split: str, set_name: str, set_seed: int, fingerprint: str
) -> None:
    entries = read_catalog(data_dir / "catalog.csv")
    _STATE.update(
        pools=make_pools(entries, split, cfg.split.holdout_maps, cfg.split.holdout_surfaces),
        hrtf=load_sofa(data_dir / "hrtf" / cfg.hrtf.file),
        load=AudioLoader(data_dir / "raw"),
        cfg=cfg,
        split=split,
        set_name=set_name,
        set_seed=set_seed,
        fingerprint=fingerprint,
        out=data_dir / "sets" / set_name / split,
    )


def _work(index: int) -> tuple[str, str | None]:
    s = _STATE
    cid = clip_id(s["split"], index)
    try:
        seed = clip_seed(s["set_seed"], s["split"], index)
        mix, stems, meta = make_clip(
            s["pools"], s["load"], s["hrtf"], s["cfg"], seed, cid, s["split"], s["set_name"]
        )
        meta["fingerprint"] = s["fingerprint"]
        write_clip(s["out"] / cid, mix, stems, meta)
        return cid, None
    except Exception as err:  # noqa: BLE001 - report per clip, keep going
        return cid, f"{type(err).__name__}: {err}"


def _git_commit() -> str:
    try:
        out = subprocess.run(
            ["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=True
        )
        return out.stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def _write_manifest(
    data_dir: Path,
    name: str,
    split: str,
    clips: int,
    hours: float,
    seed: int,
    cfg: Config,
    fingerprint: str,
) -> None:
    path = data_dir / "sets" / name / "manifest.json"
    manifest = json.loads(path.read_text()) if path.is_file() else {"splits": {}}
    manifest["config"] = config_to_dict(cfg)
    manifest["catalog_sha256"] = catalog_sha256(data_dir / "catalog.csv")
    manifest["git_commit"] = _git_commit()
    manifest["splits"][split] = {
        "clips": clips,
        "hours": hours,
        "seed": seed,
        "fingerprint": fingerprint,
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(manifest, indent=2, sort_keys=True))


def generate(
    data_dir: Path,
    name: str,
    split: str,
    hours: float,
    seed: int,
    cfg: Config,
    workers: int,
    force: bool = False,
) -> GenerateResult:
    """Generate one split. Resumes an interrupted run with the same settings; refuses
    one whose fingerprint differs unless `force` deletes the split first."""
    catalog = data_dir / "catalog.csv"
    if not catalog.is_file():
        raise DatagenError(f"{catalog} not found; run `datagen catalog` first")
    if not (data_dir / "hrtf" / cfg.hrtf.file).is_file():
        raise DatagenError("HRTF not found; run `datagen hrtf` first")
    # Validate pools in the main process so configuration errors surface once, clearly.
    make_pools(read_catalog(catalog), split, cfg.split.holdout_maps, cfg.split.holdout_surfaces)

    clips = max(1, round(hours * 3600 / cfg.scene.clip_seconds))
    fingerprint = split_fingerprint(cfg, catalog_sha256(catalog), seed, clips)
    out = data_dir / "sets" / name / split
    if force and out.exists():
        shutil.rmtree(out)
    if not force:
        _check_fingerprint(data_dir, name, split, fingerprint)
    todo = [i for i in range(clips) if not is_complete(out / clip_id(split, i))]
    result = GenerateResult(written=0, skipped=clips - len(todo))
    init_args = (data_dir, cfg, split, name, seed, fingerprint)
    if workers == 1:
        _init(*init_args)
        outcomes = (_work(i) for i in todo)
        outcomes = list(tqdm(outcomes, total=len(todo), desc=f"mix {split}"))
    else:
        with ProcessPoolExecutor(
            max_workers=workers, initializer=_init, initargs=init_args
        ) as pool:
            outcomes = list(
                tqdm(pool.map(_work, todo, chunksize=4), total=len(todo), desc=f"mix {split}")
            )
    for cid, err in outcomes:
        if err is None:
            result.written += 1
        else:
            result.failed.append((cid, err))
    _write_manifest(data_dir, name, split, clips, hours, seed, cfg, fingerprint)
    return result
