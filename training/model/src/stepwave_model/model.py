"""Causal band-mask network and its loss (M3 spec, "Model" and "Loss")."""

from __future__ import annotations

import torch
from torch import nn

from . import NUM_BANDS

INPUTS = 64
DENSE = 64
HIDDEN = 96


class BandMaskNet(nn.Module):
    """Dense(64→64, ReLU) → GRU(96) → GRU(96) → Dense(96→32) → sigmoid → gain range (dB)."""

    def __init__(self, gain_db_range: tuple[float, float] = (-12.0, 12.0)) -> None:
        super().__init__()
        self.lo, self.hi = gain_db_range
        self.inp = nn.Linear(INPUTS, DENSE)
        self.gru1 = nn.GRU(DENSE, HIDDEN, batch_first=True)
        self.gru2 = nn.GRU(HIDDEN, HIDDEN, batch_first=True)
        self.out = nn.Linear(HIDDEN, NUM_BANDS)

    def forward(
        self,
        x: torch.Tensor,
        state: tuple[torch.Tensor, torch.Tensor] | None = None,
    ) -> tuple[torch.Tensor, tuple[torch.Tensor, torch.Tensor]]:
        h1, h2 = state if state is not None else (None, None)
        y = torch.relu(self.inp(x))
        y, h1 = self.gru1(y, h1)
        y, h2 = self.gru2(y, h2)
        gain = self.lo + (self.hi - self.lo) * torch.sigmoid(self.out(y))
        return gain, (h1, h2)


def param_count(model: nn.Module) -> int:
    return sum(p.numel() for p in model.parameters())


def mask_loss(
    pred: torch.Tensor, target: torch.Tensor, active: torch.Tensor, false_boost_weight: float
) -> torch.Tensor:
    """MSE on dB gains plus `false_boost_weight` × mean squared boost on inactive frames."""
    mse = torch.mean((pred - target) ** 2)
    inactive = ~active
    if not bool(inactive.any()):
        return mse
    boost = torch.relu(pred[inactive])
    return mse + false_boost_weight * torch.mean(boost**2)


def false_boost_frames(
    pred: torch.Tensor, active: torch.Tensor, threshold_db: float
) -> tuple[int, int]:
    """(inactive frames with any band above threshold_db, inactive frames)."""
    inactive = ~active
    boosted = (pred.amax(dim=-1) > threshold_db) & inactive
    return int(boosted.sum()), int(inactive.sum())
