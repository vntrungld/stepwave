from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pytest
import stepwave_py
from typer.testing import CliRunner

from helpers import make_set
from stepwave_model import SIGNALS
from stepwave_model.cli import app
from stepwave_model.errors import TrainerError
from stepwave_model.prep import load_manifest, load_split, prep_set, read_stereo


def quiet(_: str) -> None:
    pass


def test_prep_writes_arrays(tmp_path: Path) -> None:
    make_set(tmp_path, "t", {"train": 3, "val": 2}, seconds=2.0)
    out = prep_set(tmp_path, "t", log=quiet)
    assert out == tmp_path / "features" / "t"
    m = load_manifest(out)
    assert m["constants"] == {"sample_rate": 48_000, "hop": 480, "num_bands": 32}
    assert m["splits"]["train"] == {"source_fingerprint": "fp-train", "clips": 3, "frames": 600}
    assert m["splits"]["val"] == {"source_fingerprint": "fp-val", "clips": 2, "frames": 400}
    assert len(m["fingerprint"]) == 16
    tr = load_split(out, "train")
    assert tr.index.tolist() == [[0, 200], [200, 200], [400, 200]]
    assert tr.clips == ["train-000", "train-001", "train-002"]
    assert tr.frames == 600
    for s in SIGNALS:
        assert tr.energies[s].dtype == np.float16 and tr.energies[s].shape == (600, 32)
    x = read_stereo(tmp_path / "sets/t/train/train-001/footsteps.flac")
    want = stepwave_py.band_energies_db(x[0], x[1])
    np.testing.assert_allclose(tr.clip(1)["footsteps"], want, atol=0.1)


def test_mean_std_from_train_only(tmp_path: Path) -> None:
    make_set(tmp_path, "t", {"train": 2, "val": 2}, val_gain=10.0)
    out = prep_set(tmp_path, "t", log=quiet)
    feature = load_manifest(out)["feature"]
    mix = np.asarray(load_split(out, "train").energies["mix"], np.float64)
    np.testing.assert_allclose(feature["mean"], mix.mean(0), atol=1e-3)
    np.testing.assert_allclose(feature["std"], np.maximum(mix.std(0), 1e-3), atol=1e-3)


def test_resume_skips_and_force_rebuilds(tmp_path: Path) -> None:
    make_set(tmp_path, "t", {"train": 2, "val": 1})
    out = prep_set(tmp_path, "t", log=quiet)
    before = (out / "train/mix.npy").stat().st_mtime_ns
    logs: list[str] = []
    prep_set(tmp_path, "t", log=logs.append)
    assert "train: up to date, skipped" in logs and "val: up to date, skipped" in logs
    assert (out / "train/mix.npy").stat().st_mtime_ns == before
    logs.clear()
    prep_set(tmp_path, "t", force=True, log=logs.append)
    assert "train: 2 clips" in logs


def test_changed_source_fingerprint_rebuilds_that_split(tmp_path: Path) -> None:
    make_set(tmp_path, "t", {"train": 2, "val": 1})
    prep_set(tmp_path, "t", log=quiet)
    manifest = tmp_path / "sets/t/manifest.json"
    data = json.loads(manifest.read_text())
    data["splits"]["val"]["fingerprint"] = "fp-val-2"
    manifest.write_text(json.dumps(data))
    logs: list[str] = []
    out = prep_set(tmp_path, "t", log=logs.append)
    assert "train: up to date, skipped" in logs and "val: 1 clips" in logs
    assert load_manifest(out)["splits"]["val"]["source_fingerprint"] == "fp-val-2"


def test_stale_partial_dir_is_replaced(tmp_path: Path) -> None:
    make_set(tmp_path, "t", {"train": 2, "val": 1})
    junk = tmp_path / "features/t/train.partial"
    junk.mkdir(parents=True)
    (junk / "mix.npy").write_bytes(b"junk")
    out = prep_set(tmp_path, "t", log=quiet)
    assert not junk.exists()
    assert load_split(out, "train").frames == 400


def test_workers_match_serial(tmp_path: Path) -> None:
    make_set(tmp_path, "t", {"train": 3, "val": 1})
    a = prep_set(tmp_path, "t", out_dir=tmp_path / "a", workers=1, log=quiet)
    b = prep_set(tmp_path, "t", out_dir=tmp_path / "b", workers=2, log=quiet)
    for s in SIGNALS:
        np.testing.assert_array_equal(
            load_split(a, "train").energies[s], load_split(b, "train").energies[s]
        )


def test_missing_set_split_and_features(tmp_path: Path) -> None:
    with pytest.raises(TrainerError, match="datagen mix"):
        prep_set(tmp_path, "nope", log=quiet)
    make_set(tmp_path, "t", {"train": 1})
    with pytest.raises(TrainerError, match="no val split"):
        prep_set(tmp_path, "t", log=quiet)
    with pytest.raises(TrainerError, match="trainer prep"):
        load_manifest(tmp_path / "features/none")


def test_cli_prep(tmp_path: Path) -> None:
    make_set(tmp_path, "t", {"train": 1, "val": 1})
    runner = CliRunner()
    ok = runner.invoke(app, ["prep", "t", "--data-dir", str(tmp_path), "--workers", "1"])
    assert ok.exit_code == 0, ok.output
    bad = runner.invoke(app, ["prep", "nope", "--data-dir", str(tmp_path)])
    assert bad.exit_code == 1 and "error:" in bad.output
