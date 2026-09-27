"""Ideal band gains from stem energies (M3 spec, "Target"); used by training and eval."""

from __future__ import annotations

import torch
import torch.nn.functional as F

from . import STEMS
from .config import TargetConfig


def active_frames(
    e_footsteps: torch.Tensor, e_mix: torch.Tensor, threshold_db: float, dilation: int
) -> torch.Tensor:
    """[..., T, 32] energies (dB) → bool [..., T]: footsteps within `threshold_db` of the mix
    in any band, widened by `dilation` frames on each side."""
    raw = ((e_footsteps - e_mix) >= threshold_db).any(dim=-1)
    if dilation <= 0:
        return raw
    frames = raw.shape[-1]
    flat = raw.reshape(-1, 1, frames).float()
    wide = F.max_pool1d(flat, kernel_size=2 * dilation + 1, stride=1, padding=dilation)
    return wide.reshape(raw.shape) > 0


def target_gain_db(
    energies: dict[str, torch.Tensor], cfg: TargetConfig
) -> tuple[torch.Tensor, torch.Tensor]:
    """energies: signal → [..., T, 32] dB. Returns (gain_db [..., T, 32], active [..., T]).

    While footsteps are active each stem's power is scaled by its configured level; otherwise
    the target is exactly 0 dB. Stem powers are summed so the ratio stays self-consistent."""
    power = {c: torch.pow(10.0, energies[c].float() / 10.0) for c in STEMS}
    total = sum(power.values())
    desired = sum(10.0 ** (db / 10.0) * power[c] for c, db in cfg.stem_db().items())
    active = active_frames(
        energies["footsteps"].float(),
        energies["mix"].float(),
        cfg.active_threshold_db,
        cfg.active_dilation_frames,
    )
    desired = torch.where(active.unsqueeze(-1), desired, total)
    lo, hi = cfg.gain_db_range
    return (10.0 * torch.log10(desired / total)).clamp(lo, hi), active
