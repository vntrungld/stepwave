from __future__ import annotations

import torch

from stepwave_model.model import BandMaskNet, false_boost_frames, mask_loss, param_count


def test_shapes_range_and_size() -> None:
    torch.manual_seed(0)
    net = BandMaskNet()
    gain, (h1, h2) = net(torch.randn(2, 50, 64) * 5)
    assert gain.shape == (2, 50, 32)
    assert gain.min() >= -12.0 and gain.max() <= 12.0
    assert h1.shape == (1, 2, 96) and h2.shape == (1, 2, 96)
    assert param_count(net) == 109_792


def test_causal_and_streamable() -> None:
    torch.manual_seed(0)
    net = BandMaskNet().eval()
    x = torch.randn(1, 60, 64)
    y = x.clone()
    y[:, 30:] = torch.randn(1, 30, 64)
    with torch.no_grad():
        a, _ = net(x)
        b, _ = net(y)
        first, state = net(x[:, :25])
        second, _ = net(x[:, 25:], state)
    torch.testing.assert_close(a[:, :30], b[:, :30])
    torch.testing.assert_close(torch.cat([first, second], 1), a, atol=1e-5, rtol=1e-5)


def test_false_boost_penalty() -> None:
    target = torch.zeros(1, 10, 32)
    up, down = torch.ones(1, 10, 32), -torch.ones(1, 10, 32)
    none_active = torch.zeros(1, 10, dtype=torch.bool)
    all_active = torch.ones(1, 10, dtype=torch.bool)
    assert mask_loss(up, target, none_active, 4.0).item() == 5.0
    assert mask_loss(up, target, all_active, 4.0).item() == 1.0
    assert mask_loss(down, target, none_active, 4.0).item() == 1.0


def test_false_boost_frames() -> None:
    pred = torch.zeros(1, 4, 32)
    pred[0, 1, 5] = 3.0
    pred[0, 2, 0] = 3.0
    active = torch.tensor([[False, False, True, False]])
    assert false_boost_frames(pred, active, 2.0) == (1, 3)
