from __future__ import annotations

import dataclasses
import json
from pathlib import Path

import pytest

from helpers import CONFIG_PATH, PROFILE
from stepwave_model.config import config_from_dict, config_to_dict, load_config
from stepwave_model.errors import TrainerError


def test_default_config_loads() -> None:
    cfg = load_config(CONFIG_PATH)
    assert cfg.target.gain_db_range == (-12.0, 12.0)
    assert cfg.target.stem_db() == {
        "footsteps": 6.0,
        "gunfire": -6.0,
        "explosions": -6.0,
        "ambience": -3.0,
        "other": 0.0,
    }
    assert cfg.train.seq_frames == 400 and cfg.train.batch_size == 128
    assert cfg.eval.false_boost_db == 2.0


def test_strength_matches_profile() -> None:
    profile = json.loads(PROFILE.read_text())
    assert load_config(CONFIG_PATH).target.strength_db == profile["strength_db"]


def test_round_trip_through_dict() -> None:
    cfg = load_config(CONFIG_PATH)
    assert config_from_dict(json.loads(json.dumps(config_to_dict(cfg)))) == cfg


def _write(tmp_path: Path, text: str) -> Path:
    path = tmp_path / "c.toml"
    path.write_text(text)
    return path


def test_unknown_and_missing_keys(tmp_path: Path) -> None:
    text = CONFIG_PATH.read_text().replace("seed = 0", "seed = 0\nbogus = 1")
    with pytest.raises(TrainerError, match="bogus"):
        load_config(_write(tmp_path, text))
    text = CONFIG_PATH.read_text().replace("seed = 0", "")
    with pytest.raises(TrainerError, match="seed"):
        load_config(_write(tmp_path, text))


def test_missing_table_bad_toml_and_missing_file(tmp_path: Path) -> None:
    with pytest.raises(TrainerError, match=r"\[eval\]"):
        load_config(_write(tmp_path, CONFIG_PATH.read_text().split("[eval]")[0]))
    with pytest.raises(TrainerError, match="invalid TOML"):
        load_config(_write(tmp_path, "[target"))
    with pytest.raises(TrainerError, match="not found"):
        load_config(tmp_path / "nope.toml")


def test_bad_gain_range(tmp_path: Path) -> None:
    text = CONFIG_PATH.read_text().replace("[-12.0, 12.0]", "[12.0, -12.0]")
    with pytest.raises(TrainerError, match="gain_db_range"):
        load_config(_write(tmp_path, text))


def test_config_is_frozen() -> None:
    cfg = load_config(CONFIG_PATH)
    with pytest.raises(dataclasses.FrozenInstanceError):
        cfg.train.seed = 1  # type: ignore[misc]
