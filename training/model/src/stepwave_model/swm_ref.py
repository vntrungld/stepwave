"""Pure-numpy forward pass of an .swm model: the reference M4's Rust port is tested against."""

from __future__ import annotations

import numpy as np

from .swm import SwmModel


def _sigmoid(x: np.ndarray) -> np.ndarray:
    return 1.0 / (1.0 + np.exp(-x))


def features(e_mix: np.ndarray, mean: np.ndarray, std: np.ndarray) -> np.ndarray:
    """[T, 32] mix energies (dB) → [T, 64]; same formula as features.model_input."""
    e = np.asarray(e_mix, np.float64)
    mean, std = np.asarray(mean, np.float64), np.asarray(std, np.float64)
    delta = np.zeros_like(e)
    delta[1:] = (e[1:] - e[:-1]) / std
    return np.concatenate([(e - mean) / std, delta], axis=1)


def _gru(
    x: np.ndarray, w_ih: np.ndarray, w_hh: np.ndarray, b_ih: np.ndarray, b_hh: np.ndarray
) -> np.ndarray:
    """PyTorch GRU, gates stacked (r, z, n): n = tanh(W_in x + b_in + r ⊙ (W_hn h + b_hn))."""
    hidden = w_hh.shape[1]
    h = np.zeros(hidden)
    out = np.empty((x.shape[0], hidden))
    gi_all = x @ w_ih.T + b_ih
    for t in range(x.shape[0]):
        gi, gh = gi_all[t], w_hh @ h + b_hh
        r = _sigmoid(gi[:hidden] + gh[:hidden])
        z = _sigmoid(gi[hidden : 2 * hidden] + gh[hidden : 2 * hidden])
        n = np.tanh(gi[2 * hidden :] + r * gh[2 * hidden :])
        h = (1.0 - z) * n + z * h
        out[t] = h
    return out


def forward(model: SwmModel, x: np.ndarray) -> np.ndarray:
    """[T, 64] model input → [T, 32] gains (dB), float32."""
    w = {k: v.astype(np.float64) for k, v in model.tensors.items()}
    y = np.maximum(np.asarray(x, np.float64) @ w["inp.weight"].T + w["inp.bias"], 0.0)
    for g in ("gru1", "gru2"):
        y = _gru(
            y,
            w[f"{g}.weight_ih_l0"],
            w[f"{g}.weight_hh_l0"],
            w[f"{g}.bias_ih_l0"],
            w[f"{g}.bias_hh_l0"],
        )
    lo, hi = model.header["gain_db_range"]
    return (lo + (hi - lo) * _sigmoid(y @ w["out.weight"].T + w["out.bias"])).astype(np.float32)


def gains_from_energies(model: SwmModel, e_mix: np.ndarray) -> np.ndarray:
    """[T, 32] mix energies (dB) → [T, 32] gains (dB), using the header's normalisation."""
    f = model.header["feature"]
    return forward(model, features(e_mix, f["mean"], f["std"]))
