"""Model input from mix band energies. M4's Rust port must reproduce this exactly."""

from __future__ import annotations

import torch


def model_input(e_mix: torch.Tensor, mean: torch.Tensor, std: torch.Tensor) -> torch.Tensor:
    """[..., T, 32] mix energies (dB) → [..., T, 64]: (E − mean)/std, then
    (E[t] − E[t−1])/std with 0 at t = 0."""
    e = e_mix.float()
    delta = torch.zeros_like(e)
    delta[..., 1:, :] = (e[..., 1:, :] - e[..., :-1, :]) / std
    return torch.cat([(e - mean) / std, delta], dim=-1)
