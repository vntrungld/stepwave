from __future__ import annotations

import builtins
import importlib
import struct
import sys
from pathlib import Path

import numpy as np
import pytest
import torch
from typer.testing import CliRunner

import stepwave_model.export as export_mod
from helpers import CONFIG_PATH, write_run
from stepwave_model import swm_ref
from stepwave_model.cli import app
from stepwave_model.errors import TrainerError
from stepwave_model.export import export
from stepwave_model.features import model_input
from stepwave_model.model import BandMaskNet
from stepwave_model.swm import MAGIC, read_swm, tensor_layout, write_swm


def random_tensors(seed: int = 0) -> dict[str, np.ndarray]:
    rng = np.random.default_rng(seed)
    return {n: rng.standard_normal(s).astype(np.float32) for n, s in tensor_layout()}


def test_round_trip(tmp_path: Path) -> None:
    t = random_tensors()
    write_swm(tmp_path / "m.swm", {"arch": "dense-gru2-dense", "x": 1}, t)
    m = read_swm(tmp_path / "m.swm")
    assert m.header["x"] == 1
    assert [d["name"] for d in m.header["tensors"]] == [n for n, _ in tensor_layout()]
    for name, value in t.items():
        np.testing.assert_array_equal(m.tensors[name], value)
    assert (tmp_path / "m.swm").read_bytes()[:4] == MAGIC


def test_reader_rejects_corruption(tmp_path: Path) -> None:
    path = tmp_path / "m.swm"
    write_swm(path, {"arch": "dense-gru2-dense"}, random_tensors())
    good = path.read_bytes()
    cases = {
        "magic": b"XXXX" + good[4:],
        "version": good[:4] + struct.pack("<I", 2) + good[8:],
        "truncated": good[:-8],
        "trailing": good + b"\0\0\0\0",
    }
    for word, data in cases.items():
        path.write_bytes(data)
        with pytest.raises(TrainerError, match=word):
            read_swm(path)


def test_writer_rejects_bad_tensors(tmp_path: Path) -> None:
    t = random_tensors()
    t["out.bias"] = np.full(32, np.nan, np.float32)
    with pytest.raises(TrainerError, match="non-finite"):
        write_swm(tmp_path / "a.swm", {"arch": "dense-gru2-dense"}, t)
    t = random_tensors()
    t["out.bias"] = np.zeros(31, np.float32)
    with pytest.raises(TrainerError, match="shape"):
        write_swm(tmp_path / "b.swm", {"arch": "dense-gru2-dense"}, t)


def test_numpy_reference_matches_torch(tmp_path: Path) -> None:
    run = write_run(tmp_path / "run", seed=3)
    header = export(run, tmp_path / "m.swm")
    model = read_swm(tmp_path / "m.swm")
    net = BandMaskNet()
    net.load_state_dict(torch.load(run / "best.pt", weights_only=False)["model"])
    x = np.random.default_rng(1).standard_normal((300, 64)).astype(np.float32) * 3
    with torch.no_grad():
        want = net(torch.from_numpy(x)[None])[0][0].numpy()
    np.testing.assert_allclose(swm_ref.forward(model, x), want, atol=1e-4)
    assert header["param_count"] == 109_792 and header["gate_order"] == "rzn"
    assert header["feature"]["mean"] == [-60.0] * 32


def test_numpy_features_match_torch() -> None:
    e = np.random.default_rng(2).standard_normal((50, 32)).astype(np.float32) * 10 - 60
    mean, std = np.full(32, -60.0), np.full(32, 15.0)
    want = model_input(torch.from_numpy(e), torch.tensor(mean).float(), torch.tensor(std).float())
    np.testing.assert_allclose(swm_ref.features(e, mean, std), want.numpy(), atol=1e-5)


def test_swm_ref_docstring_specifies_the_maths_for_the_m4_port() -> None:
    """swm_ref's module docstring is the spec M4's Rust port is written and checked
    against; it must fully specify the maths, not just point at the code (Minor
    finding #8)."""
    doc = (swm_ref.__doc__ or "").lower()
    required = [
        "norm",  # input features: (E - mean) / std
        "delta",  # input features: (E[t] - E[t-1]) / std, 0 at t = 0
        "delta[0] = 0",
        "dense",
        "relu",
        "gru",
        "r, z, n",  # PyTorch gate order
        "zero initial hidden state",
        "sigmoid",
        "gain_db_range",
        "header order",  # tensor layout matches the .swm header's tensor order
        "1e-3 db",  # M4's f32 port must match this reference within this tolerance
    ]
    missing = [term for term in required if term not in doc]
    assert not missing, f"swm_ref docstring is missing: {missing}"


def test_export_parity_failure_removes_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    run = write_run(tmp_path / "run")
    real = swm_ref.forward
    monkeypatch.setattr(export_mod.swm_ref, "forward", lambda m, x: real(m, x) + 1.0)
    with pytest.raises(TrainerError, match="parity"):
        export(run, tmp_path / "m.swm")
    assert not (tmp_path / "m.swm").exists()


def test_train_and_export_import_without_audio_extra(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    real_import = builtins.__import__

    def blocked(name: str, *args: object, **kwargs: object) -> object:
        if name.split(".")[0] in ("stepwave_py", "soundfile"):
            raise ImportError(f"blocked {name}")
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", blocked)
    for mod in ("stepwave_py", "soundfile"):
        monkeypatch.delitem(sys.modules, mod, raising=False)
    for mod in [m for m in sys.modules if m.startswith("stepwave_model")]:
        monkeypatch.delitem(sys.modules, mod)
    exp = importlib.import_module("stepwave_model.export")
    importlib.import_module("stepwave_model.train")
    exp.export(write_run(tmp_path / "run"), tmp_path / "m.swm")
    assert (tmp_path / "m.swm").is_file()


def test_cli_export(tmp_path: Path) -> None:
    run = write_run(tmp_path / "run")
    result = CliRunner().invoke(app, ["export", str(run), "--out", str(tmp_path / "m.swm")])
    assert result.exit_code == 0, result.output
    missing = CliRunner().invoke(app, ["export", str(tmp_path / "nope")])
    assert missing.exit_code == 1 and "error:" in missing.output
    assert CONFIG_PATH.is_file()
