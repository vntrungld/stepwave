"""Scan extracted sounds, label them by rule, resample to 48 kHz mono, and measure them."""

from __future__ import annotations

import csv
import hashlib
from collections import Counter
from concurrent.futures import ProcessPoolExecutor
from dataclasses import astuple, dataclass
from math import gcd
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import resample_poly
from tqdm import tqdm

from . import CLASSES, SR
from .rules import Rule, classify

AUDIO_EXTS = {".wav", ".mp3", ".flac", ".ogg"}
MIN_DURATION_S = 0.02
SILENCE_DBFS = -80.0
HEADER = ["path", "class", "surface", "map", "duration_s", "peak_dbfs", "rms_dbfs"]


@dataclass(frozen=True)
class CatalogEntry:
    path: str
    cls: str
    surface: str
    map: str
    duration_s: float
    peak_dbfs: float
    rms_dbfs: float


def processed_path(raw_dir: Path, rel: str) -> Path:
    return raw_dir / "_48k" / f"{rel}.flac"


def _db(x: float) -> float:
    return float(20.0 * np.log10(max(x, 1e-12)))


def load_mono_48k(path: Path) -> np.ndarray:
    data, sr = sf.read(path, dtype="float32", always_2d=True)
    mono = data.mean(axis=1)
    if sr != SR:
        g = gcd(SR, sr)
        mono = resample_poly(mono, SR // g, sr // g)
    return np.asarray(mono, dtype=np.float32)


def _process(args: tuple[Path, str, str, str, str]) -> tuple[CatalogEntry | None, str | None]:
    raw_dir, rel, cls, surface, map_name = args
    try:
        x = load_mono_48k(raw_dir / rel)
    except Exception as err:  # noqa: BLE001 - any decoder failure drops the file
        return None, f"undecodable: {err}"
    duration = len(x) / SR
    if duration < MIN_DURATION_S:
        return None, f"too short ({duration * 1000:.1f} ms)"
    peak = _db(float(np.abs(x).max()))
    if peak < SILENCE_DBFS:
        return None, f"silent (peak {peak:.1f} dBFS)"
    rms = _db(float(np.sqrt(np.mean(x.astype(np.float64) ** 2))))
    out = processed_path(raw_dir, rel)
    out.parent.mkdir(parents=True, exist_ok=True)
    sf.write(out, x, SR, subtype="PCM_24", format="FLAC")
    return (
        CatalogEntry(
            rel, cls, surface, map_name, round(duration, 6), round(peak, 3), round(rms, 3)
        ),
        None,
    )


def _scan(raw_dir: Path) -> list[str]:
    rels = []
    for p in raw_dir.rglob("*"):
        rel = p.relative_to(raw_dir).as_posix()
        if (
            p.is_file()
            and p.suffix.lower() in AUDIO_EXTS
            and not rel.startswith("_48k/")
            and not any(part.startswith(".") for part in rel.split("/"))
        ):
            rels.append(rel)
    return sorted(rels)


def build_catalog(
    raw_dir: Path, rules: list[Rule], workers: int | None = None
) -> tuple[list[CatalogEntry], list[tuple[str, str]]]:
    jobs = []
    for rel in _scan(raw_dir):
        label = classify(rel, rules)
        jobs.append((raw_dir, rel, label.cls, label.surface, label.map))
    if workers == 1:
        results = [_process(job) for job in tqdm(jobs, desc="catalog")]
    else:
        with ProcessPoolExecutor(max_workers=workers) as pool:
            results = list(
                tqdm(pool.map(_process, jobs, chunksize=64), total=len(jobs), desc="catalog")
            )
    entries: list[CatalogEntry] = []
    dropped: list[tuple[str, str]] = []
    for job, (entry, reason) in zip(jobs, results, strict=True):
        if entry is not None:
            entries.append(entry)
        else:
            dropped.append((job[1], reason or "unknown"))
    entries.sort(key=lambda e: e.path)
    return entries, dropped


def write_catalog(entries: list[CatalogEntry], path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(HEADER)
        for e in entries:
            writer.writerow(astuple(e))


def read_catalog(path: Path) -> list[CatalogEntry]:
    with path.open(newline="") as f:
        rows = list(csv.DictReader(f))
    return [
        CatalogEntry(
            r["path"],
            r["class"],
            r["surface"],
            r["map"],
            float(r["duration_s"]),
            float(r["peak_dbfs"]),
            float(r["rms_dbfs"]),
        )
        for r in rows
    ]


def catalog_sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_catalog_report(
    entries: list[CatalogEntry], dropped: list[tuple[str, str]], path: Path
) -> None:
    lines = [
        "# Catalog report",
        "",
        "## Files per class",
        "",
        "| class | files | minutes |",
        "|---|---|---|",
    ]
    for cls in CLASSES:
        members = [e for e in entries if e.cls == cls]
        lines.append(f"| {cls} | {len(members)} | {sum(e.duration_s for e in members) / 60:.1f} |")
    for title, key in (("Footstep surfaces", "surface"), ("Ambience maps", "map")):
        cls = "footsteps" if key == "surface" else "ambience"
        counts = Counter(getattr(e, key) for e in entries if e.cls == cls)
        lines += ["", f"## {title}", "", "| name | files |", "|---|---|"]
        lines += [f"| {name or '(none)'} | {n} |" for name, n in sorted(counts.items())]
    others = [e.path for e in entries if e.cls == "other"]
    lines += ["", f"## Unmatched → other ({len(others)})", ""]
    lines += [f"- `{p}`" for p in others]
    lines += ["", f"## Dropped ({len(dropped)})", ""]
    lines += [f"- `{p}`: {reason}" for p, reason in dropped]
    path.write_text("\n".join(lines) + "\n")
