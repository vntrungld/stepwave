"""Typed view of configs/<game>.toml: target mask, training and evaluation parameters."""

from __future__ import annotations

import dataclasses
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .errors import TrainerError


@dataclass(frozen=True)
class TargetConfig:
    strength_db: float
    gunfire_db: float
    explosions_db: float
    ambience_db: float
    other_db: float
    active_threshold_db: float
    active_dilation_frames: int
    gain_db_range: tuple[float, float]

    def stem_db(self) -> dict[str, float]:
        """Desired level change per stem while footsteps are active."""
        return {
            "footsteps": self.strength_db,
            "gunfire": self.gunfire_db,
            "explosions": self.explosions_db,
            "ambience": self.ambience_db,
            "other": self.other_db,
        }


@dataclass(frozen=True)
class TrainConfig:
    seq_frames: int
    batch_size: int
    epochs: int
    lr: float
    weight_decay: float
    grad_clip: float
    false_boost_weight: float
    early_stop_patience: int
    seed: int


@dataclass(frozen=True)
class EvalConfig:
    false_boost_db: float
    goal_footstep_gain_db: float
    goal_snr_improvement_db: float
    goal_false_boost_pct: float
    listen_examples: int


@dataclass(frozen=True)
class Config:
    target: TargetConfig
    train: TrainConfig
    eval: EvalConfig


def _section(cls: type, data: dict[str, Any], name: str) -> Any:
    table = data.get(name)
    if not isinstance(table, dict):
        raise TrainerError(f"config: missing [{name}] table")
    fields = {f.name for f in dataclasses.fields(cls)}
    unknown, missing = set(table) - fields, fields - set(table)
    if unknown or missing:
        raise TrainerError(
            f"config [{name}]: unknown keys {sorted(unknown)}, missing keys {sorted(missing)}"
        )
    values = dict(table)
    if "gain_db_range" in values:
        rng = values["gain_db_range"]
        if not (isinstance(rng, list | tuple) and len(rng) == 2 and rng[0] < rng[1]):
            raise TrainerError(f"config [{name}]: gain_db_range must be [low, high], got {rng}")
        values["gain_db_range"] = (float(rng[0]), float(rng[1]))
    return cls(**values)


def target_from_dict(data: dict[str, Any]) -> TargetConfig:
    """A `TargetConfig` from a plain dict, e.g. an .swm header's `target` field (written by
    `trainer export` from `config_to_dict(cfg)["target"]`)."""
    return _section(TargetConfig, {"target": data}, "target")


def config_from_dict(data: dict[str, Any]) -> Config:
    return Config(
        target=_section(TargetConfig, data, "target"),
        train=_section(TrainConfig, data, "train"),
        eval=_section(EvalConfig, data, "eval"),
    )


def config_to_dict(cfg: Config) -> dict[str, Any]:
    return dataclasses.asdict(cfg)


def load_config(path: Path) -> Config:
    try:
        data = tomllib.loads(path.read_text())
    except FileNotFoundError as err:
        raise TrainerError(f"config not found: {path}") from err
    except tomllib.TOMLDecodeError as err:
        raise TrainerError(f"invalid TOML in {path}: {err}") from err
    return config_from_dict(data)
