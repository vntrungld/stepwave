from __future__ import annotations

import dataclasses

import numpy as np
import torch

from helpers import CONFIG_PATH
from stepwave_model import STEMS
from stepwave_model.config import load_config
from stepwave_model.features import model_input
from stepwave_model.targets import active_frames, target_gain_db

CFG = load_config(CONFIG_PATH).target
FLOOR = -100.0


def energies(frames: int = 40, **levels: float | np.ndarray) -> dict[str, torch.Tensor]:
    """Flat-spectrum stem energies (dB); each level is a scalar or a per-frame array."""
    e = {s: torch.full((frames, 32), FLOOR) for s in STEMS}
    for s, v in levels.items():
        col = torch.as_tensor(np.broadcast_to(np.asarray(v, np.float32), (frames,)).copy())
        e[s] = col[:, None].expand(frames, 32).clone()
    e["mix"] = 10 * torch.log10(sum(torch.pow(10.0, e[s] / 10) for s in STEMS))
    return e


def test_no_footsteps_is_exactly_zero() -> None:
    gain, active = target_gain_db(energies(gunfire=-20.0, ambience=-30.0), CFG)
    assert not active.any()
    assert torch.equal(gain, torch.zeros_like(gain))


def test_footsteps_alone_get_strength() -> None:
    gain, active = target_gain_db(energies(footsteps=-20.0), CFG)
    assert active.all()
    torch.testing.assert_close(gain, torch.full_like(gain, 6.0), atol=1e-3, rtol=0)


def test_gunfire_ducked_while_footsteps_active() -> None:
    gain, active = target_gain_db(energies(footsteps=-44.0, gunfire=-20.0), CFG)
    assert active.all()
    assert torch.all((gain + 6.0).abs() < 0.3)


def test_below_threshold_is_inactive() -> None:
    gain, active = target_gain_db(energies(footsteps=-46.5, gunfire=-20.0), CFG)
    assert not active.any() and torch.equal(gain, torch.zeros_like(gain))


def test_gain_is_clipped_to_range() -> None:
    loud = dataclasses.replace(CFG, strength_db=20.0)
    gain, _ = target_gain_db(energies(footsteps=-20.0), loud)
    assert torch.allclose(gain, torch.full_like(gain, 12.0))
    quiet = dataclasses.replace(CFG, gunfire_db=-40.0)
    gain, _ = target_gain_db(
        energies(footsteps=-60.0, gunfire=-20.0),
        dataclasses.replace(quiet, active_threshold_db=-50.0),
    )
    assert torch.all(gain >= -12.0) and torch.any(gain == -12.0)


def test_dilation_widens_activity() -> None:
    fs = np.full(60, FLOOR, np.float32)
    fs[20] = -20.0
    e = energies(60, footsteps=fs, ambience=-40.0)
    active = active_frames(e["footsteps"], e["mix"], -25.0, 15)
    assert active.nonzero().flatten().tolist() == list(range(5, 36))
    batch = active_frames(torch.stack([e["footsteps"]] * 2), torch.stack([e["mix"]] * 2), -25.0, 15)
    assert batch.shape == (2, 60) and torch.equal(batch[1], active)
    assert active_frames(e["footsteps"], e["mix"], -25.0, 0).nonzero().flatten().tolist() == [20]


def test_model_input_normalises_and_deltas() -> None:
    e = torch.randn(3, 10, 32) * 10 - 50
    mean, std = torch.full((32,), -50.0), torch.full((32,), 10.0)
    x = model_input(e, mean, std)
    assert x.shape == (3, 10, 64)
    torch.testing.assert_close(x[..., :32], (e + 50.0) / 10.0)
    assert torch.equal(x[:, 0, 32:], torch.zeros(3, 32))
    torch.testing.assert_close(x[:, 1:, 32:], (e[:, 1:] - e[:, :-1]) / 10.0)
