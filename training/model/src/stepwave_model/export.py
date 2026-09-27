"""`trainer export`: checkpoint → .swm v1, verified against the numpy reference."""

from __future__ import annotations

from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import numpy as np
import torch

from . import HOP, NUM_BANDS, SAMPLE_RATE, swm_ref
from .config import config_from_dict, config_to_dict
from .errors import TrainerError
from .model import DENSE, HIDDEN, INPUTS, BandMaskNet, param_count
from .swm import ARCH, read_swm, write_swm

PARITY_FRAMES = 1000
PARITY_TOL_DB = 1e-3


def export(run_dir: Path, out: Path, which: str = "best") -> dict[str, Any]:
    ck_path = run_dir / f"{which}.pt"
    if not ck_path.is_file():
        raise TrainerError(f"{ck_path} not found")
    ck = torch.load(ck_path, map_location="cpu", weights_only=False)
    cfg = config_from_dict(ck["config"])
    model = BandMaskNet(cfg.target.gain_db_range)
    model.load_state_dict(ck["model"])
    model.eval()
    duck = cfg.target.stem_db()
    header = {
        "arch": ARCH,
        "gate_order": "rzn",
        "sizes": {"inputs": INPUTS, "dense": DENSE, "hidden": HIDDEN, "bands": NUM_BANDS},
        "gain_db_range": list(cfg.target.gain_db_range),
        "sample_rate": SAMPLE_RATE,
        "hop": HOP,
        "num_bands": NUM_BANDS,
        "feature": {
            "mean": list(ck["feature"]["mean"]),
            "std": list(ck["feature"]["std"]),
            "delta": "raw_diff_over_std",
        },
        "strength_db": cfg.target.strength_db,
        "duck_db": {k: v for k, v in duck.items() if k != "footsteps"},
        "target": config_to_dict(cfg)["target"],
        "param_count": param_count(model),
        "features_fingerprint": ck["features_fingerprint"],
        "created": datetime.now(UTC).isoformat(timespec="seconds"),
        "source": {"checkpoint": which, "epoch": ck["epoch"], "val_loss": ck["val_loss"]},
    }
    tensors = {k: v.detach().cpu().numpy() for k, v in model.state_dict().items()}
    write_swm(out, header, tensors)

    x = np.random.default_rng(0).standard_normal((PARITY_FRAMES, INPUTS)).astype(np.float32)
    with torch.no_grad():
        want = model(torch.from_numpy(x)[None])[0][0].numpy()
    written = read_swm(out)
    err = float(np.max(np.abs(swm_ref.forward(written, x) - want)))
    if err > PARITY_TOL_DB:
        out.unlink()
        raise TrainerError(
            f"parity check failed: numpy reference differs from torch by {err:.2e} dB "
            f"(> {PARITY_TOL_DB}); {out} removed"
        )
    return written.header
