from __future__ import annotations

import csv
import dataclasses
import json
from pathlib import Path

import numpy as np
import pytest
import torch
from typer.testing import CliRunner

import stepwave_model.train as train_mod
from helpers import CONFIG_PATH, prepped
from stepwave_model.cli import app
from stepwave_model.config import Config, load_config
from stepwave_model.dataset import Batches
from stepwave_model.errors import TrainerError
from stepwave_model.features import model_input
from stepwave_model.model import BandMaskNet
from stepwave_model.prep import load_manifest, load_split
from stepwave_model.targets import target_gain_db
from stepwave_model.train import pick_device, train, validate

CPU = torch.device("cpu")


def small_cfg(**train_kw: object) -> Config:
    cfg = load_config(CONFIG_PATH)
    kw = {"seq_frames": 50, "batch_size": 8, "epochs": 10, "lr": 3e-3, **train_kw}
    return dataclasses.replace(cfg, train=dataclasses.replace(cfg.train, **kw))


def quiet(_: str) -> None:
    pass


def test_window_matches_full_clip_features(tmp_path: Path) -> None:
    feats = prepped(tmp_path, {"train": 1, "val": 1}, seconds=2.5)
    cfg = load_config(CONFIG_PATH)
    d = cfg.target.active_dilation_frames
    feature = load_manifest(feats)["feature"]
    split = load_split(feats, "train")
    count = int(split.index[0, 1])
    seq = count - 1 - 2 * d
    b = Batches(split, feature, cfg.target, CPU)
    x, target, active = b.window(np.random.default_rng(0), batch=1, seq=seq)
    assert x.shape == (1, seq, 64) and target.shape == (1, seq, 32) and active.shape == (1, seq)
    e = torch.from_numpy(np.asarray(split.clip(0)["mix"], np.float32))
    full = model_input(e, torch.tensor(feature["mean"]), torch.tensor(feature["std"]))
    torch.testing.assert_close(x[0], full[1 + d : 1 + d + seq])


def test_window_targets_match_full_clip_with_dilation(tmp_path: Path) -> None:
    """A footstep near a window's edge must still widen activity inside the window: the
    window's target/active must equal the matching slice of the full-clip computation,
    which only holds if the window reads `active_dilation_frames` extra context on each
    side (dataset.py:44-59, Important finding #1)."""
    feats = prepped(tmp_path, {"train": 1, "val": 1}, seconds=2.5)
    cfg = load_config(CONFIG_PATH)
    d = cfg.target.active_dilation_frames
    feature = load_manifest(feats)["feature"]
    split = load_split(feats, "train")
    count = int(split.index[0, 1])
    # forced start: the only eligible window spans the whole clip, so its target/active
    # slice must match the full-clip computation exactly.
    seq = count - 1 - 2 * d
    b = Batches(split, feature, cfg.target, CPU)
    _, target, active = b.window(np.random.default_rng(0), batch=1, seq=seq)
    e = {s: torch.from_numpy(np.asarray(v, np.float32)) for s, v in split.clip(0).items()}
    full_target, full_active = target_gain_db(e, cfg.target)
    torch.testing.assert_close(target[0], full_target[1 + d : 1 + d + seq])
    assert torch.equal(active[0], full_active[1 + d : 1 + d + seq])


def test_window_rejects_short_clips(tmp_path: Path) -> None:
    feats = prepped(tmp_path, {"train": 1, "val": 1}, seconds=1.0)
    cfg = load_config(CONFIG_PATH)
    b = Batches(load_split(feats, "train"), load_manifest(feats)["feature"], cfg.target, CPU)
    with pytest.raises(TrainerError, match="train.seq_frames"):
        b.window(np.random.default_rng(0), batch=2, seq=400)


def test_training_reduces_val_loss(tmp_path: Path) -> None:
    feats = prepped(tmp_path)
    cfg = small_cfg()
    torch.manual_seed(cfg.train.seed)
    fresh = BandMaskNet(cfg.target.gain_db_range)
    val = Batches(load_split(feats, "val"), load_manifest(feats)["feature"], cfg.target, CPU)
    before, _ = validate(fresh, val, cfg)
    best = train(feats, tmp_path / "run", cfg, device="cpu", log=quiet)
    ck = torch.load(best, weights_only=False)
    assert ck["val_loss"] < before
    run = tmp_path / "run"
    for name in ("best.pt", "last.pt", "train.json", "log.csv"):
        assert (run / name).is_file()
    info = json.loads((run / "train.json").read_text())
    assert info["device"] == "cpu" and info["features"]["fingerprint"] == ck["features_fingerprint"]
    rows = list(csv.DictReader((run / "log.csv").open()))
    assert rows and set(rows[0]) == {
        "epoch",
        "train_loss",
        "val_loss",
        "false_boost_pct",
        "seconds",
    }


def test_resume_continues_and_checks_features(tmp_path: Path) -> None:
    feats = prepped(tmp_path)
    run = tmp_path / "run"
    train(feats, run, small_cfg(epochs=2, early_stop_patience=99), device="cpu", log=quiet)
    train(
        feats,
        run,
        small_cfg(epochs=3, early_stop_patience=99),
        device="cpu",
        resume=True,
        log=quiet,
    )
    assert torch.load(run / "last.pt", weights_only=False)["epoch"] == 2
    assert len(list(csv.DictReader((run / "log.csv").open()))) == 3
    manifest = feats / "manifest.json"
    data = json.loads(manifest.read_text())
    data["fingerprint"] = "0" * 16
    manifest.write_text(json.dumps(data))
    with pytest.raises(TrainerError, match="fingerprint"):
        train(feats, run, small_cfg(epochs=4), device="cpu", resume=True, log=quiet)


def test_max_steps_and_nan_abort(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    feats = prepped(tmp_path)
    train(feats, tmp_path / "a", small_cfg(), device="cpu", max_steps=2, log=quiet)
    assert (tmp_path / "a/last.pt").is_file()

    def nan_loss(pred: torch.Tensor, *_: object) -> torch.Tensor:
        return pred.sum() * float("nan")

    monkeypatch.setattr(train_mod, "mask_loss", nan_loss)
    with pytest.raises(TrainerError, match="step 0"):
        train(feats, tmp_path / "b", small_cfg(), device="cpu", log=quiet)


def test_pick_device() -> None:
    assert pick_device("cpu") == CPU
    assert pick_device("auto").type in ("cpu", "cuda")
    with pytest.raises(TrainerError):
        pick_device("tpu")
    if not torch.cuda.is_available():
        with pytest.raises(TrainerError, match="CUDA"):
            pick_device("cuda")


def test_cli_train(tmp_path: Path) -> None:
    feats = prepped(tmp_path)
    result = CliRunner().invoke(
        app,
        [
            "train",
            str(feats),
            "--out",
            str(tmp_path / "run"),
            "--device",
            "cpu",
            "--max-steps",
            "2",
            "--config",
            str(CONFIG_PATH),
        ],
    )
    assert result.exit_code == 0, result.output
    assert "epoch 1/" in result.output
