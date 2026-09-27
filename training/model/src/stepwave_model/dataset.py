"""Training windows and validation clips built on the fly from prepped energies."""

from __future__ import annotations

from collections import defaultdict
from collections.abc import Iterator
from typing import Any

import numpy as np
import torch

from . import NUM_BANDS, SIGNALS
from .config import TargetConfig
from .errors import TrainerError
from .features import model_input
from .prep import Split
from .targets import target_gain_db

Batch = tuple[torch.Tensor, torch.Tensor, torch.Tensor]


class Batches:
    def __init__(
        self,
        split: Split,
        feature: dict[str, Any],
        target_cfg: TargetConfig,
        device: torch.device,
    ) -> None:
        self.split = split
        self.cfg = target_cfg
        self.device = device
        self.mean = torch.tensor(feature["mean"], dtype=torch.float32, device=device)
        self.std = torch.tensor(feature["std"], dtype=torch.float32, device=device)

    def _prepare(self, e: dict[str, np.ndarray]) -> Batch:
        t = {
            s: torch.from_numpy(np.asarray(v, dtype=np.float32)).to(self.device)
            for s, v in e.items()
        }
        target, active = target_gain_db(t, self.cfg)
        return model_input(t["mix"], self.mean, self.std), target, active

    def window(self, rng: np.random.Generator, batch: int, seq: int) -> Batch:
        """Random `seq`-frame windows. One extra leading frame is read so the first delta
        is real, then dropped."""
        offsets, counts = self.split.index[:, 0], self.split.index[:, 1]
        eligible = np.flatnonzero(counts >= seq + 1)
        if eligible.size == 0:
            raise TrainerError(
                f"no clip has {seq + 1} frames (longest {int(counts.max())}); "
                "lower train.seq_frames"
            )
        pick = rng.choice(eligible, size=batch)
        starts = offsets[pick] + (rng.random(batch) * (counts[pick] - seq)).astype(np.int64)
        rows = (starts[:, None] + np.arange(seq + 1)).ravel()
        e = {s: self.split.energies[s][rows].reshape(batch, seq + 1, NUM_BANDS) for s in SIGNALS}
        x, target, active = self._prepare(e)
        return x[:, 1:], target[:, 1:], active[:, 1:]

    def clips(self, max_batch: int = 32) -> Iterator[Batch]:
        """Every clip in full, batched by equal length."""
        by_len: dict[int, list[int]] = defaultdict(list)
        for i, n in enumerate(self.split.index[:, 1]):
            by_len[int(n)].append(i)
        for _, ids in sorted(by_len.items()):
            for k in range(0, len(ids), max_batch):
                chunk = [self.split.clip(i) for i in ids[k : k + max_batch]]
                yield self._prepare({s: np.stack([c[s] for c in chunk]) for s in SIGNALS})
