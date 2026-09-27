from __future__ import annotations

import dataclasses
from pathlib import Path

import pytest

from stepwave_datagen.catalog import CatalogEntry
from stepwave_datagen.config import load_config
from stepwave_datagen.errors import DatagenError
from stepwave_datagen.scene import clip_id, clip_seed, make_pools, sample_scene

CFG = load_config(Path(__file__).resolve().parents[1] / "configs/cs2.toml")


def entry(path: str, cls: str, surface: str = "", map_: str = "", dur: float = 0.2) -> CatalogEntry:
    return CatalogEntry(path, cls, surface, map_, dur, -10.0, -20.0)


ENTRIES = [
    entry("f/concrete/1.wav", "footsteps", surface="concrete"),
    entry("f/concrete/2.wav", "footsteps", surface="concrete"),
    entry("f/wood/1.wav", "footsteps", surface="wood"),
    entry("f/bass/1.wav", "footsteps", surface="bass"),
    entry("f/suit/1.wav", "footsteps", surface="suit"),
    entry("a/dust2.wav", "ambience", map_="dust2", dur=5.0),
    entry("a/mirage.wav", "ambience", map_="mirage", dur=5.0),
    entry("g/ak.wav", "gunfire"),
    entry("e/he.wav", "explosions", dur=1.0),
    entry("o/ui.wav", "other"),
]
LAYERS = ("bass", "suit")
HOLD = (("mirage",), ("wood",), LAYERS)


def scene(seed: int, split: str = "train"):
    pools = make_pools(ENTRIES, split, *HOLD)
    return sample_scene(pools, CFG.scene, CFG.render, CFG.bus, seed, clip_id(split, seed))


def test_seed_is_stable_and_distinct() -> None:
    assert clip_seed(1, "train", 0) == clip_seed(1, "train", 0)
    assert len({clip_seed(1, "train", i) for i in range(100)}) == 100
    assert clip_seed(1, "train", 0) != clip_seed(1, "val", 0)
    assert clip_id("val", 7) == "val-000007"


def test_same_seed_same_scene() -> None:
    assert scene(42) == scene(42)
    assert scene(42) != scene(43)


def test_every_scene_has_one_ambience_and_events_inside_clip() -> None:
    for s in (scene(i) for i in range(200)):
        assert sum(e.cls == "ambience" for e in s.events) == 1
        assert all(0.0 <= e.onset_s < s.clip_seconds for e in s.events)


def test_footstep_free_fraction() -> None:
    free = sum(not any(e.cls == "footsteps" for e in scene(i).events) for i in range(500))
    assert abs(free / 500 - 0.3) <= 0.06


def test_snr_only_with_footsteps() -> None:
    for s in (scene(i) for i in range(100)):
        has = any(e.cls == "footsteps" for e in s.events)
        assert (s.footstep_snr_db is not None) == has
        if has:
            assert -30.0 <= s.footstep_snr_db <= 0.0


def test_sequence_keeps_one_surface_and_position() -> None:
    for s in (scene(i) for i in range(100)):
        steps = [e for e in s.events if e.cls == "footsteps"]
        for key in {(e.azimuth_deg, e.distance_m) for e in steps}:
            surfaces = {e.surface for e in steps if (e.azimuth_deg, e.distance_m) == key}
            assert len(surfaces) == 1


def test_own_gunfire_is_front_close_and_dry() -> None:
    own = [e for i in range(300) for e in scene(i).events if e.cls == "gunfire" and not e.spatial]
    assert own
    assert all((e.azimuth_deg, e.distance_m, e.wet) == (0.0, 0.5, 0.0) for e in own)


def test_train_never_uses_holdouts() -> None:
    for s in (scene(i, "train") for i in range(300)):
        for e in s.events:
            assert not (e.cls == "ambience" and e.map == "mirage")
            assert not (e.cls == "footsteps" and e.surface == "wood")


def test_val_uses_only_holdouts_for_steps_and_ambience() -> None:
    for s in (scene(i, "val") for i in range(300)):
        for e in s.events:
            if e.cls == "ambience":
                assert e.map == "mirage"
            if e.cls == "footsteps":
                assert e.surface == "wood"


def test_val_requires_holdouts() -> None:
    with pytest.raises(DatagenError, match="holdout"):
        make_pools(ENTRIES, "val", (), (), LAYERS)


def test_unknown_holdout_is_rejected() -> None:
    with pytest.raises(DatagenError, match="mirgae"):
        make_pools(ENTRIES, "train", ("mirgae",), ("wood",), LAYERS)


@pytest.mark.parametrize("split", ["train", "val"])
def test_layer_surfaces_are_never_sampled_as_surfaces(split: str) -> None:
    pools = make_pools(ENTRIES, split, *HOLD)
    assert not set(pools.footsteps) & set(LAYERS)
    for s in (scene(i, split) for i in range(200)):
        assert not any(e.cls == "footsteps" and e.surface in LAYERS for e in s.events)


def test_holdout_naming_a_layer_is_rejected() -> None:
    with pytest.raises(DatagenError, match="layer"):
        make_pools(ENTRIES, "train", ("mirage",), ("suit",), LAYERS)


def test_empty_required_class_is_named() -> None:
    no_gun = [e for e in ENTRIES if e.cls != "gunfire"]
    with pytest.raises(DatagenError, match="gunfire"):
        make_pools(no_gun, "train", *HOLD)


def test_optional_classes_may_be_empty() -> None:
    minimal = [e for e in ENTRIES if e.cls not in ("explosions", "other")]
    pools = make_pools(minimal, "train", *HOLD)
    cfg = dataclasses.replace(CFG.scene, explosions=(1, 1), other=(1, 1))
    s = sample_scene(pools, cfg, CFG.render, CFG.bus, 1, "train-000001")
    assert not any(e.cls in ("explosions", "other") for e in s.events)
