"""Shared test fixtures (synthetic only, no game assets)."""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import soundfile as sf

REPO = Path(__file__).resolve().parents[3]
CONFIG_PATH = REPO / "training/model/configs/cs2.toml"
PROFILE = REPO / "profiles/cs2.json"

SR = 48_000


def make_clip(clip_dir: Path, seed: int, seconds: float, gain: float = 1.0) -> None:
    """Footstep noise bursts (40 ms every 0.5 s) in the first half, one gunfire burst in the
    second half, quiet ambience throughout; stems sum to the mix."""
    n = int(seconds * SR)
    rng = np.random.default_rng(seed)
    stems = {
        c: np.zeros((2, n), np.float32)
        for c in ("footsteps", "gunfire", "explosions", "ambience", "other")
    }
    for start in np.arange(0.1, seconds / 2, 0.5):
        a = int(start * SR)
        b = a + int(0.04 * SR)
        stems["footsteps"][:, a:b] = rng.uniform(-0.3, 0.3, (2, b - a))
    a = int(0.75 * seconds * SR)
    b = a + int(0.1 * SR)
    stems["gunfire"][:, a:b] = rng.uniform(-0.5, 0.5, (2, b - a))
    stems["ambience"][:] = rng.uniform(-0.01, 0.01, (2, n))
    stems = {c: (x * gain).astype(np.float32) for c, x in stems.items()}
    mix = sum(stems.values())
    clip_dir.mkdir(parents=True, exist_ok=True)
    for name, data in (("mix", mix), *stems.items()):
        sf.write(clip_dir / f"{name}.flac", data.T, SR, subtype="PCM_24", format="FLAC")
    (clip_dir / "meta.json").write_text(json.dumps({"clip_id": clip_dir.name}))


def make_set(
    data_dir: Path,
    name: str,
    clips: dict[str, int],
    seconds: float = 2.0,
    val_gain: float = 1.0,
    fingerprints: dict[str, str] | None = None,
) -> Path:
    """An M2-shaped set: sets/<name>/manifest.json plus <split>/<split>-NNN clip folders."""
    root = data_dir / "sets" / name
    fingerprints = fingerprints or {}
    splits = {}
    for split, count in clips.items():
        gain = val_gain if split == "val" else 1.0
        for i in range(count):
            seed = i + (1000 if split == "val" else 0)
            make_clip(root / split / f"{split}-{i:03d}", seed=seed, seconds=seconds, gain=gain)
        splits[split] = {"fingerprint": fingerprints.get(split, f"fp-{split}")}
    (root / "manifest.json").write_text(json.dumps({"splits": splits}))
    return root
