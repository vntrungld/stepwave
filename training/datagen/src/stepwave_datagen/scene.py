"""Seeded sampling of what happens in a clip: which sounds, when, where, how loud."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass

import numpy as np

from .catalog import CatalogEntry
from .config import BusConfig, RenderConfig, SceneConfig
from .errors import DatagenError

REQUIRED_CLASSES = ("footsteps", "ambience", "gunfire")


@dataclass(frozen=True)
class Event:
    source: str
    cls: str
    surface: str
    map: str
    onset_s: float
    azimuth_deg: float
    elevation_deg: float
    distance_m: float
    spatial: bool
    wet: float
    offset_s: float = 0.0


@dataclass(frozen=True)
class Scene:
    clip_id: str
    seed: int
    clip_seconds: float
    events: tuple[Event, ...]
    rt60_s: float
    rir_seed: int
    footstep_snr_db: float | None
    loudness_rms_dbfs: float


@dataclass(frozen=True)
class Pools:
    footsteps: dict[str, list[CatalogEntry]]
    ambience: list[CatalogEntry]
    gunfire: list[CatalogEntry]
    explosions: list[CatalogEntry]
    other: list[CatalogEntry]


def clip_seed(set_seed: int, split: str, index: int) -> int:
    digest = hashlib.sha256(f"{set_seed}:{split}:{index}".encode()).digest()
    return int.from_bytes(digest[:8], "little")


def clip_id(split: str, index: int) -> str:
    return f"{split}-{index:06d}"


def make_pools(
    entries: list[CatalogEntry],
    split: str,
    holdout_maps: tuple[str, ...],
    holdout_surfaces: tuple[str, ...],
) -> Pools:
    if split not in ("train", "val"):
        raise DatagenError(f"unknown split {split!r}; expected 'train' or 'val'")
    if split == "val" and (not holdout_maps or not holdout_surfaces):
        raise DatagenError(
            "the val split needs split.holdout_maps and split.holdout_surfaces in the config"
        )
    maps = {e.map for e in entries if e.cls == "ambience"}
    surfaces = {e.surface for e in entries if e.cls == "footsteps"}
    for name in holdout_maps:
        if name not in maps:
            raise DatagenError(f"holdout map {name!r} has no ambience files; known: {sorted(maps)}")
    for name in holdout_surfaces:
        if name not in surfaces:
            raise DatagenError(
                f"holdout surface {name!r} has no footstep files; known: {sorted(surfaces)}"
            )
    val = split == "val"
    hm, hs = set(holdout_maps), set(holdout_surfaces)
    footsteps: dict[str, list[CatalogEntry]] = {}
    for e in entries:
        if e.cls == "footsteps" and (e.surface in hs) == val:
            footsteps.setdefault(e.surface, []).append(e)
    pools = Pools(
        footsteps=footsteps,
        ambience=[e for e in entries if e.cls == "ambience" and (e.map in hm) == val],
        gunfire=[e for e in entries if e.cls == "gunfire"],
        explosions=[e for e in entries if e.cls == "explosions"],
        other=[e for e in entries if e.cls == "other"],
    )
    for name in REQUIRED_CLASSES:
        if not getattr(pools, name):
            raise DatagenError(
                f"no '{name}' files available for the {split} split; check rules and holdouts"
            )
    return pools


def sample_scene(
    pools: Pools,
    cfg: SceneConfig,
    render_cfg: RenderConfig,
    bus_cfg: BusConfig,
    seed: int,
    clip_id: str,
) -> Scene:
    rng = np.random.default_rng(seed)
    length = cfg.clip_seconds

    def uniform(r: tuple[float, float]) -> float:
        return float(rng.uniform(r[0], r[1]))

    def count(r: tuple[int, int]) -> int:
        return int(rng.integers(r[0], r[1] + 1))

    def pick(pool: list[CatalogEntry]) -> CatalogEntry:
        return pool[int(rng.integers(len(pool)))]

    def position() -> tuple[float, float, float]:
        return uniform(cfg.azimuth_deg), uniform(cfg.elevation_deg), uniform(cfg.distance_m)

    rt60 = uniform(render_cfg.rt60_s)
    rir_seed = int(rng.integers(2**63))
    loudness = uniform(bus_cfg.loudness_rms_dbfs)
    events: list[Event] = []

    amb = pick(pools.ambience)
    events.append(
        Event(
            amb.path,
            "ambience",
            amb.surface,
            amb.map,
            0.0,
            uniform((0.0, 360.0)),
            0.0,
            1.0,
            True,
            uniform(render_cfg.wet),
            offset_s=uniform((0.0, amb.duration_s)),
        )
    )

    if rng.random() < cfg.footstep_probability:
        surfaces = sorted(pools.footsteps)
        for _ in range(count(cfg.footstep_sequences)):
            surface = surfaces[int(rng.integers(len(surfaces)))]
            az, el, dist = position()
            wet = uniform(render_cfg.wet)
            span = min(uniform(cfg.sequence_span_s), length)
            t = uniform((0.0, length - span))
            end = t + span
            while t < end and t < length:
                e = pick(pools.footsteps[surface])
                events.append(
                    Event(e.path, "footsteps", e.surface, e.map, t, az, el, dist, True, wet)
                )
                t += uniform(cfg.step_interval_s)

    for _ in range(count(cfg.gunfire_bursts)):
        e = pick(pools.gunfire)
        if rng.random() < cfg.own_gunfire_probability:
            az, el, dist, spatial, wet = 0.0, 0.0, 0.5, False, 0.0
        else:
            az, el = uniform(cfg.azimuth_deg), uniform(cfg.elevation_deg)
            dist, spatial, wet = uniform(cfg.enemy_distance_m), True, uniform(render_cfg.wet)
        t = uniform((0.0, length))
        interval = uniform(cfg.shot_interval_s)
        for _ in range(count(cfg.shots_per_burst)):
            if t >= length:
                break
            events.append(Event(e.path, "gunfire", e.surface, e.map, t, az, el, dist, spatial, wet))
            t += interval

    for cls, rng_count, pool in (
        ("explosions", cfg.explosions, pools.explosions),
        ("other", cfg.other, pools.other),
    ):
        if not pool:
            continue
        for _ in range(count(rng_count)):
            e = pick(pool)
            az, el, dist = position()
            events.append(
                Event(
                    e.path,
                    cls,
                    e.surface,
                    e.map,
                    uniform((0.0, length)),
                    az,
                    el,
                    dist,
                    True,
                    uniform(render_cfg.wet),
                )
            )

    has_steps = any(e.cls == "footsteps" for e in events)
    snr = uniform(cfg.footstep_snr_db) if has_steps else None
    return Scene(clip_id, seed, length, tuple(events), rt60, rir_seed, snr, loudness)
