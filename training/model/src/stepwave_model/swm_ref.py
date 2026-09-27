"""Pure-numpy forward pass of an .swm model: the reference M4's Rust port is tested against.

This docstring, not just the code below, is the spec: M4's f32 Rust port must reproduce
this behaviour and match this reference within 1e-3 dB (see export.PARITY_TOL_DB) on real
exported weights, per band, per frame.

Input features (`features()`), from mix band energies E (dB, shape [T, 32]) and the
header's `feature.mean`/`feature.std` (shape [32]):

    norm[t]  = (E[t] - mean) / std
    delta[t] = (E[t] - E[t-1]) / std, with delta[0] = 0 (no real previous frame)
    input[t] = concat(norm[t], delta[t])                       -> 64 values per frame

Model (`forward()`):

  1. Dense(64 -> 64), then ReLU.
  2. Two GRU layers, gru1 then gru2 (hidden size = header's `sizes.hidden`), each run with
     a *zero initial hidden state* (this reference never carries hidden state across
     separate `forward()` calls, so a streaming Rust port must reset it at the start of
     each stream/clip, not once ever). Gates are PyTorch's, stacked (r, z, n):

         r = sigmoid(W_ir x + b_ir + W_hr h + b_hr)
         z = sigmoid(W_iz x + b_iz + W_hz h + b_hz)
         n = tanh(W_in x + b_in + r * (W_hn h + b_hn))
         h' = (1 - z) * n + z * h

  3. Dense(hidden -> 32), then sigmoid, mapped onto the header's `gain_db_range` [lo, hi]:

         gain_db = lo + (hi - lo) * sigmoid(...)

Tensor layout: `SwmModel.tensors` is keyed, shaped and ordered exactly as
`swm.tensor_layout()` returns, matching the .swm header's `tensors` field (header order):
inp.{weight,bias}; for gru1 then gru2, {weight_ih_l0, weight_hh_l0, bias_ih_l0,
bias_hh_l0} (input-hidden and hidden-hidden weights/biases for the stacked r/z/n gates,
each of shape (3 * hidden, ...)); out.{weight,bias}. All tensors are row-major f32.
"""

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
