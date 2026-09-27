from __future__ import annotations

import json
import pickle
import shutil
from pathlib import Path

import numpy as np
import pytest
import torch
from typer.testing import CliRunner

from helpers import CONFIG_PATH, PROFILE, prepped, write_run
from stepwave_model import STEMS
from stepwave_model.cli import app
from stepwave_model.config import load_config
from stepwave_model.errors import TrainerError
from stepwave_model.evaluate import MODES, ModelGains, evaluate, summarize
from stepwave_model.export import export
from stepwave_model.prep import load_split
from stepwave_model.targets import active_frames


def stub_gains(e: dict[str, np.ndarray]) -> np.ndarray:
    """+6 dB on raw (undilated) footstep-active frames, 0 dB elsewhere."""
    fs = torch.from_numpy(np.asarray(e["footsteps"], np.float32))
    mix = torch.from_numpy(np.asarray(e["mix"], np.float32))
    act = active_frames(fs, mix, -25.0, 0).numpy()
    g = np.zeros((len(act), 32), np.float32)
    g[act] = 6.0
    return g


def quiet(_: str) -> None:
    pass


def test_stub_boost_is_measured(tmp_path: Path) -> None:
    feats = prepped(tmp_path, {"train": 2, "val": 2})
    cfg = load_config(CONFIG_PATH)
    out = tmp_path / "eval"
    m = evaluate(stub_gains, feats, tmp_path / "sets/t", out, cfg, PROFILE, log=quiet)
    assert set(m) == set(MODES)
    assert abs(m["model"]["footstep_gain_db"] - 6.0) < 1.0
    assert m["model"]["snr_improvement_db"] > 0.0
    assert m["model"]["false_boost_pct"] == 0.0
    assert m["off"]["footstep_gain_db"] == pytest.approx(0.0, abs=1e-6)
    assert m["off"]["snr_improvement_db"] == pytest.approx(0.0, abs=1e-6)
    report = (out / "report.md").read_text()
    assert "Footstep gain" in report and "False-boost rate" in report
    assert json.loads((out / "metrics.json").read_text())["model"] == m["model"]
    for i in range(2):
        for mode in MODES:
            assert (out / f"val_{i:02d}_{mode}.flac").is_file()
    assert (out / "band_gain.png").is_file() and (out / "gain_hist.png").is_file()


def test_summarize_math() -> None:
    energy = np.zeros((3, len(STEMS), 2, 2))
    fs = STEMS.index("footsteps")
    energy[:, :, :, :] = 1.0
    energy[2, fs, 0, 1] = 4.0  # model: footsteps x4 in power on active frames
    energy[2, STEMS.index("gunfire"), 0, 1] = 0.25
    fb = np.array([[0, 10], [0, 10], [1, 10]], float)
    acc = {"energy": energy, "fb": fb}
    m = summarize(acc)
    assert m["model"]["footstep_gain_db"] == pytest.approx(6.0206, abs=1e-3)
    assert m["model"]["gunfire_active_db"] == pytest.approx(-6.0206, abs=1e-3)
    # rest in: 4 stems x 1 = 4; rest out: 3 + 0.25 = 3.25
    want = 10 * np.log10(4.0 / 3.25) - 10 * np.log10(1.0 / 4.0)
    assert m["model"]["snr_improvement_db"] == pytest.approx(want, abs=1e-3)
    assert m["model"]["false_boost_pct"] == pytest.approx(10.0)
    assert m["off"]["footstep_gain_db"] == 0.0


def test_eval_rejects_stale_features(tmp_path: Path) -> None:
    feats = prepped(tmp_path, {"train": 1, "val": 2})
    cfg = load_config(CONFIG_PATH)
    manifest = tmp_path / "sets/t/manifest.json"
    data = json.loads(manifest.read_text())
    data["splits"]["val"]["fingerprint"] = "regenerated"
    manifest.write_text(json.dumps(data))
    with pytest.raises(TrainerError, match="re-run prep"):
        evaluate(stub_gains, feats, tmp_path / "sets/t", tmp_path / "e", cfg, PROFILE, log=quiet)
    data["splits"]["val"]["fingerprint"] = "fp-val"
    manifest.write_text(json.dumps(data))
    shutil.rmtree(tmp_path / "sets/t/val/val-001")
    with pytest.raises(TrainerError, match="re-run prep"):
        evaluate(stub_gains, feats, tmp_path / "sets/t", tmp_path / "e", cfg, PROFILE, log=quiet)


def test_model_gains_is_picklable_and_shaped(tmp_path: Path) -> None:
    feats = prepped(tmp_path, {"train": 1, "val": 1}, seconds=1.0)
    export(write_run(tmp_path / "run"), tmp_path / "m.swm")
    fn = pickle.loads(pickle.dumps(ModelGains(tmp_path / "m.swm")))
    e = load_split(feats, "val").clip(0)
    g = fn(e)
    assert g.shape == (e["mix"].shape[0], 32) and g.dtype == np.float32
    assert np.all(np.abs(g) <= 12.0)


def test_cli_eval(tmp_path: Path) -> None:
    prepped(tmp_path, {"train": 1, "val": 1}, seconds=1.0)
    export(write_run(tmp_path / "run"), tmp_path / "m.swm")
    result = CliRunner().invoke(
        app,
        [
            "eval",
            str(tmp_path / "m.swm"),
            "--set",
            "t",
            "--data-dir",
            str(tmp_path),
            "--workers",
            "1",
            "--profile",
            str(PROFILE),
            "--config",
            str(CONFIG_PATH),
            "--out",
            str(tmp_path / "e"),
        ],
    )
    assert result.exit_code == 0, result.output
    assert (tmp_path / "e/report.md").is_file()
