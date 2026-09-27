"""Deterministic random-weights .swm fixture + reference gains for core's M4 tests.

Run from the repo root:
    uv run --project training/model python training/model/scripts/make_swm_fixture.py
Re-running reproduces byte-identical files. Contains no game-derived data.
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import torch

from stepwave_model import HOP, NUM_BANDS, SAMPLE_RATE, swm_ref
from stepwave_model.model import DENSE, HIDDEN, INPUTS, BandMaskNet, param_count
from stepwave_model.swm import ARCH, read_swm, write_swm

OUT = Path("core/tests/fixtures")
FRAMES = 200


def main() -> None:
    torch.manual_seed(1234)
    model = BandMaskNet((-12.0, 12.0))
    rng = np.random.default_rng(1234)
    mean = rng.uniform(-80.0, -30.0, NUM_BANDS)
    std = rng.uniform(5.0, 20.0, NUM_BANDS)
    header = {
        "arch": ARCH,
        "gate_order": "rzn",
        "sizes": {"inputs": INPUTS, "dense": DENSE, "hidden": HIDDEN, "bands": NUM_BANDS},
        "gain_db_range": [-12.0, 12.0],
        "sample_rate": SAMPLE_RATE,
        "hop": HOP,
        "num_bands": NUM_BANDS,
        "feature": {
            "mean": [float(v) for v in mean],
            "std": [float(v) for v in std],
            "delta": "raw_diff_over_std",
        },
        "strength_db": 6.0,
        "duck_db": {"gunfire": -6.0, "explosions": -6.0, "ambience": -3.0, "other": 0.0},
        "param_count": param_count(model),
        "features_fingerprint": "fixture",
        "created": "fixture",
        "source": {"checkpoint": "random", "epoch": 0, "val_loss": 0.0},
    }
    tensors = {k: v.detach().numpy() for k, v in model.state_dict().items()}
    OUT.mkdir(parents=True, exist_ok=True)
    write_swm(OUT / "model_random.swm", header, tensors)

    frames = rng.uniform(-100.0, 0.0, (FRAMES, NUM_BANDS)).astype(np.float32)
    gains = swm_ref.gains_from_energies(read_swm(OUT / "model_random.swm"), frames)
    expected = {
        "frames": [[float(v) for v in row] for row in frames],
        "gains": [[round(float(v), 6) for v in row] for row in gains],
        "strength_db": 6.0,
        "gain_db_range": [-12.0, 12.0],
    }
    (OUT / "model_random_expected.json").write_text(json.dumps(expected) + "\n")
    print(f"wrote {OUT}/model_random.swm and model_random_expected.json")


if __name__ == "__main__":
    main()
