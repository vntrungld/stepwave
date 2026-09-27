"""Typed loading of `configs/*.toml`."""

from __future__ import annotations

import dataclasses
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .errors import DatagenError

Range = tuple[float, float]
IntRange = tuple[int, int]


@dataclass(frozen=True)
class HrtfConfig:
    url: str
    file: str


@dataclass(frozen=True)
class SplitConfig:
    holdout_maps: tuple[str, ...]
    holdout_surfaces: tuple[str, ...]
    layer_surfaces: tuple[str, ...]
    train_hours: float
    val_hours: float


@dataclass(frozen=True)
class SceneConfig:
    clip_seconds: float
    footstep_probability: float
    footstep_sequences: IntRange
    step_interval_s: Range
    sequence_span_s: Range
    gunfire_bursts: IntRange
    own_gunfire_probability: float
    shots_per_burst: IntRange
    shot_interval_s: Range
    enemy_distance_m: Range
    explosions: IntRange
    other: IntRange
    azimuth_deg: Range
    elevation_deg: Range
    distance_m: Range
    footstep_snr_db: Range


@dataclass(frozen=True)
class RenderConfig:
    rt60_s: Range
    wet: Range
    lpf_min_hz: float
    lpf_max_hz: float


@dataclass(frozen=True)
class BusConfig:
    threshold_dbfs: float
    ratio: float
    attack_ms: float
    release_ms: float
    loudness_rms_dbfs: Range
    peak_ceiling_dbfs: float


@dataclass(frozen=True)
class Config:
    vpk: str
    hrtf: HrtfConfig
    split: SplitConfig
    scene: SceneConfig
    render: RenderConfig
    bus: BusConfig


def _build(cls: type, table: dict[str, Any], where: str) -> Any:
    values = {}
    for field in dataclasses.fields(cls):
        if field.name not in table:
            raise DatagenError(f"config: missing key '{where}.{field.name}'")
        value = table[field.name]
        values[field.name] = tuple(value) if isinstance(value, list) else value
    return cls(**values)


def load_config(path: Path) -> Config:
    try:
        data = tomllib.loads(path.read_text())
    except FileNotFoundError as err:
        raise DatagenError(f"config file not found: {path}") from err
    except tomllib.TOMLDecodeError as err:
        raise DatagenError(f"{path}: invalid TOML ({err}); fix the syntax and re-run") from err
    if "vpk" not in data:
        raise DatagenError("config: missing key 'vpk'")
    sections = {
        "hrtf": HrtfConfig,
        "split": SplitConfig,
        "scene": SceneConfig,
        "render": RenderConfig,
        "bus": BusConfig,
    }
    built = {}
    for name, cls in sections.items():
        if name not in data:
            raise DatagenError(f"config: missing section '[{name}]'")
        built[name] = _build(cls, data[name], name)
    return Config(vpk=data["vpk"], **built)


def config_to_dict(cfg: Config) -> dict[str, Any]:
    return dataclasses.asdict(cfg)
