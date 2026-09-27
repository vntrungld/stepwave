from __future__ import annotations

import json
from pathlib import Path

import pytest

from stepwave_datagen.config import config_to_dict, load_config
from stepwave_datagen.errors import DatagenError

DEFAULT = Path(__file__).resolve().parents[1] / "configs/cs2.toml"


def test_default_config_loads_with_tuples() -> None:
    cfg = load_config(DEFAULT)
    assert cfg.scene.clip_seconds == 10.0
    assert cfg.scene.footstep_sequences == (1, 3)
    assert cfg.bus.loudness_rms_dbfs == (-35.0, -15.0)
    assert cfg.split.holdout_maps == ()
    assert cfg.hrtf.file.endswith(".sofa")


def test_snapshot_is_json_serialisable() -> None:
    snap = config_to_dict(load_config(DEFAULT))
    assert json.loads(json.dumps(snap))["scene"]["footstep_probability"] == 0.7


def test_missing_key_is_a_datagen_error(tmp_path: Path) -> None:
    bad = tmp_path / "bad.toml"
    bad.write_text(DEFAULT.read_text().replace("ratio = 3.0\n", ""))
    with pytest.raises(DatagenError, match="ratio"):
        load_config(bad)


def test_invalid_toml_is_a_datagen_error(tmp_path: Path) -> None:
    bad = tmp_path / "bad.toml"
    bad.write_text("vpk = [unterminated")
    with pytest.raises(DatagenError, match="invalid TOML"):
        load_config(bad)
