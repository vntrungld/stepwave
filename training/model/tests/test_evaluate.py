from __future__ import annotations

import json
import pickle
import shutil
from pathlib import Path

import numpy as np
import pytest
import torch
from typer.testing import CliRunner

import stepwave_model.evaluate as evaluate_mod
from helpers import CONFIG_PATH, PROFILE, prepped, write_run
from stepwave_model import STEMS
from stepwave_model.cli import _eval_target, app
from stepwave_model.config import config_to_dict, load_config
from stepwave_model.errors import TrainerError
from stepwave_model.evaluate import MODES, ModelGains, _db, evaluate, summarize
from stepwave_model.export import export
from stepwave_model.prep import load_split
from stepwave_model.swm import read_swm
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
    assert set(m) == {*MODES, "real"}
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


def test_workers_match_serial(tmp_path: Path) -> None:
    feats = prepped(tmp_path, {"train": 1, "val": 3}, seconds=1.0)
    cfg = load_config(CONFIG_PATH)
    m1 = evaluate(
        stub_gains,
        feats,
        tmp_path / "sets/t",
        tmp_path / "e1",
        cfg,
        PROFILE,
        workers=1,
        log=quiet,
    )
    m2 = evaluate(
        stub_gains,
        feats,
        tmp_path / "sets/t",
        tmp_path / "e2",
        cfg,
        PROFILE,
        workers=2,
        log=quiet,
    )
    assert m1 == m2


def test_db_and_summarize_undefined_ratio_is_none() -> None:
    assert _db(0.0, 1.0) is None
    assert _db(1.0, 0.0) is None
    assert _db(1.0, 1.0) == pytest.approx(0.0)
    energy = np.zeros((3, len(STEMS), 2, 2))
    energy[:, STEMS.index("footsteps"), :, :] = 1.0  # only footsteps ever sound
    fb = np.array([[0, 10], [0, 10], [0, 10]], float)
    m = summarize({"energy": energy, "fb": fb})
    for mode in MODES:
        assert m[mode]["gunfire_active_db"] is None
        assert m[mode]["gunfire_inactive_db"] is None
        assert m[mode]["footstep_gain_db"] == pytest.approx(0.0)
        # no masking energy at all anywhere -> SNR is undefined, not 0 dB
        assert m[mode]["snr_improvement_db"] is None


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


def test_synthetic_results_survive_a_broken_real_eval(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A finished synthetic eval must not be thrown away by a later failure while
    processing real recordings (evaluate.py:403-412, Minor finding #5)."""
    feats = prepped(tmp_path, {"train": 1, "val": 1}, seconds=1.0)
    cfg = load_config(CONFIG_PATH)
    out = tmp_path / "eval"

    def boom(*_a: object, **_k: object) -> list[dict[str, object]]:
        raise RuntimeError("boom")

    monkeypatch.setattr(evaluate_mod, "evaluate_real", boom)
    with pytest.raises(RuntimeError, match="boom"):
        evaluate(
            stub_gains,
            feats,
            tmp_path / "sets/t",
            out,
            cfg,
            PROFILE,
            real_dir=tmp_path / "real",
            log=quiet,
        )
    assert (out / "metrics.json").is_file()
    assert (out / "report.md").is_file()
    assert json.loads((out / "metrics.json").read_text())["real"] == []


def test_export_writes_target_header(tmp_path: Path) -> None:
    export(write_run(tmp_path / "run"), tmp_path / "m.swm")
    model = read_swm(tmp_path / "m.swm")
    want = json.loads(json.dumps(config_to_dict(load_config(CONFIG_PATH))["target"]))
    assert model.header["target"] == want


def test_eval_target_prefers_header_over_config() -> None:
    cfg = load_config(CONFIG_PATH)
    header_target = config_to_dict(cfg)["target"]
    logs: list[str] = []
    other = load_config(CONFIG_PATH).target
    resolved = _eval_target({"target": header_target}, other, Path("c.toml"), logs.append)
    assert resolved == cfg.target
    assert not logs


def test_eval_target_falls_back_with_warning() -> None:
    cfg = load_config(CONFIG_PATH)
    logs: list[str] = []
    resolved = _eval_target({}, cfg.target, Path("c.toml"), logs.append)
    assert resolved == cfg.target
    assert any("c.toml" in line and "target" in line for line in logs)


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


def test_cli_eval_uses_header_target_over_mismatched_config(tmp_path: Path) -> None:
    """--config's target must not silently override what the model was trained with
    (Minor finding #3); a --config whose [target] table is missing entirely still works
    because the header supplies it, and no fallback warning is printed."""
    prepped(tmp_path, {"train": 1, "val": 1}, seconds=1.0)
    export(write_run(tmp_path / "run"), tmp_path / "m.swm")
    bad_config = tmp_path / "bad.toml"
    bad_config.write_text(CONFIG_PATH.read_text().replace("strength_db = 6.0", "strength_db = 1.0"))
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
            str(bad_config),
            "--out",
            str(tmp_path / "e"),
        ],
    )
    assert result.exit_code == 0, result.output
    assert "warning" not in result.output
