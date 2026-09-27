from __future__ import annotations

import dataclasses
import json
from pathlib import Path

import numpy as np
import pytest
from typer.testing import CliRunner

from helpers import make_tiny_data
from stepwave_datagen import CLASSES
from stepwave_datagen.catalog import read_catalog
from stepwave_datagen.cli import app
from stepwave_datagen.errors import DatagenError
from stepwave_datagen.generate import AudioLoader, generate, make_clip
from stepwave_datagen.hrtf import load_sofa
from stepwave_datagen.render import active_rms
from stepwave_datagen.scene import make_pools
from stepwave_datagen.writer import is_complete, read_clip

CFG_PATH = Path(__file__).resolve().parents[1] / "configs/cs2.toml"
HOURS_5_CLIPS = 5 * 2.0 / 3600


@pytest.fixture
def tiny(tmp_path: Path):
    data = tmp_path / "data"
    return data, make_tiny_data(data, CFG_PATH)


def test_make_clip_is_deterministic(tiny) -> None:
    data, cfg = tiny
    pools = make_pools(read_catalog(data / "catalog.csv"), "train", ("mirage",), ("wood",))
    hrtf = load_sofa(data / "hrtf" / cfg.hrtf.file)
    load = AudioLoader(data / "raw")
    a = make_clip(pools, load, hrtf, cfg, 123, "train-000000", "train", "s")
    b = make_clip(pools, load, hrtf, cfg, 123, "train-000000", "train", "s")
    assert np.array_equal(a[0], b[0]) and a[2] == b[2]
    assert a[2]["split"] == "train" and a[2]["set"] == "s"


def test_audio_loader_cache_is_bounded_by_bytes(tmp_path: Path) -> None:
    import soundfile as sf

    from stepwave_datagen.catalog import processed_path

    sizes = {"a.wav": 1000, "b.wav": 3000, "c.wav": 2000, "big.wav": 20_000}
    for rel, n in sizes.items():
        out = processed_path(tmp_path, rel)
        out.parent.mkdir(parents=True, exist_ok=True)
        sf.write(out, np.zeros(n, np.float32), 48_000, subtype="PCM_24", format="FLAC")
    load = AudioLoader(tmp_path, max_bytes=5 * 4000)  # float32: 4 bytes/sample
    for rel in ["a.wav", "b.wav", "c.wav", "a.wav", "big.wav", "c.wav", "b.wav", "a.wav"]:
        assert load(rel).shape == (sizes[rel],)
        assert load.cached_bytes <= load.max_bytes
        assert load.cached_bytes == sum(x.nbytes for x in load._cache.values())
    assert "big.wav" not in load._cache  # larger than the cap: returned, never cached
    assert list(load._cache) == ["b.wav", "a.wav"]  # LRU order after evictions


def test_generate_writes_valid_clips(tiny) -> None:
    data, cfg = tiny
    result = generate(data, "s", "train", HOURS_5_CLIPS, 1, cfg, workers=1)
    assert (result.written, result.skipped, result.failed) == (5, 0, [])
    clips = sorted((data / "sets/s/train").iterdir())
    assert [c.name for c in clips] == [f"train-{i:06d}" for i in range(5)]
    for clip in clips:
        mix, stems, meta = read_clip(clip)
        assert mix.shape == (2, 96_000)
        total = np.sum(np.stack([stems[c] for c in CLASSES]), axis=0)
        err = np.sqrt(np.mean((total - mix) ** 2)) / max(np.sqrt(np.mean(mix**2)), 1e-12)
        assert 20 * np.log10(max(err, 1e-12)) < -80
        assert np.abs(mix).max() <= 10 ** (-1 / 20)
        assert meta["clip_id"] == clip.name
        rms_db = 20 * np.log10(np.sqrt(np.mean(mix.astype(np.float64) ** 2)))
        assert abs(meta["mix_rms_dbfs"] - rms_db) < 0.01
        assert meta["bus_scale"] > 0.0
    manifest = json.loads((data / "sets/s/manifest.json").read_text())
    split = manifest["splits"]["train"]
    assert {k: split[k] for k in ("clips", "hours", "seed")} == {
        "clips": 5,
        "hours": HOURS_5_CLIPS,
        "seed": 1,
    }
    assert len(split["fingerprint"]) == 64
    for clip in clips:
        assert json.loads((clip / "meta.json").read_text())["fingerprint"] == split["fingerprint"]
    assert len(manifest["catalog_sha256"]) == 64


def test_meta_records_post_bus_footstep_snr(tiny) -> None:
    data, cfg = tiny
    generate(data, "s", "train", 20 * 2.0 / 3600, 1, cfg, workers=1)
    seen = 0
    for clip in sorted((data / "sets/s/train").iterdir()):
        _, stems, meta = read_clip(clip)
        if meta["footstep_snr_db"] is None:
            assert meta["footstep_snr_db_final"] is None
            continue
        seen += 1
        rest = sum(stems[c] for c in CLASSES if c != "footsteps")
        measured = 20 * np.log10(active_rms(stems["footsteps"]) / active_rms(rest))
        assert abs(meta["footstep_snr_db_final"] - measured) < 0.5
    assert seen > 0


def test_generate_is_byte_identical_across_runs(tiny, tmp_path: Path) -> None:
    # Two different set names are required here so the second call doesn't just skip
    # already-complete clips from the first (resumable generation). Every clip's
    # meta.json legitimately records its own set name (per the writer contract), so
    # that one field is expected to differ ("a" vs "b"); everything else, including
    # all six audio files and the rest of meta.json, must be byte-for-byte identical
    # regardless of worker count.
    data, cfg = tiny
    generate(data, "a", "train", HOURS_5_CLIPS, 7, cfg, workers=1)
    generate(data, "b", "train", HOURS_5_CLIPS, 7, cfg, workers=2)
    for clip in (data / "sets/a/train").iterdir():
        for f in clip.iterdir():
            other = data / "sets/b/train" / clip.name / f.name
            if f.name == "meta.json":
                meta_a = json.loads(f.read_text())
                meta_b = json.loads(other.read_text())
                assert meta_a["set"] == "a" and meta_b["set"] == "b"
                del meta_a["set"], meta_b["set"]
                assert meta_a == meta_b
            else:
                assert f.read_bytes() == other.read_bytes()


def test_rerun_skips_complete_clips(tiny) -> None:
    data, cfg = tiny
    generate(data, "s", "train", HOURS_5_CLIPS, 1, cfg, workers=1)
    again = generate(data, "s", "train", HOURS_5_CLIPS, 1, cfg, workers=1)
    assert (again.written, again.skipped) == (0, 5)


def _other_cfg(cfg):
    return dataclasses.replace(cfg, scene=dataclasses.replace(cfg.scene, footstep_probability=0.5))


def test_changed_config_refuses_to_resume(tiny) -> None:
    data, cfg = tiny
    generate(data, "s", "train", HOURS_5_CLIPS, 1, cfg, workers=1)
    with pytest.raises(DatagenError, match="--force"):
        generate(data, "s", "train", HOURS_5_CLIPS, 1, _other_cfg(cfg), workers=1)


@pytest.mark.parametrize("seed, hours", [(2, HOURS_5_CLIPS), (1, 3 * 2.0 / 3600)])
def test_changed_seed_or_hours_refuses_to_resume(tiny, seed: int, hours: float) -> None:
    data, cfg = tiny
    generate(data, "s", "train", HOURS_5_CLIPS, 1, cfg, workers=1)
    with pytest.raises(DatagenError, match="new set name"):
        generate(data, "s", "train", hours, seed, cfg, workers=1)


def test_interrupted_run_without_manifest_still_detects_drift(tiny) -> None:
    data, cfg = tiny
    generate(data, "s", "train", HOURS_5_CLIPS, 1, cfg, workers=1)
    (data / "sets/s/manifest.json").unlink()
    with pytest.raises(DatagenError, match="--force"):
        generate(data, "s", "train", HOURS_5_CLIPS, 1, _other_cfg(cfg), workers=1)


def test_force_regenerates_split(tiny) -> None:
    data, cfg = tiny
    generate(data, "s", "train", HOURS_5_CLIPS, 1, cfg, workers=1)
    generate(data, "s", "val", HOURS_5_CLIPS, 1, cfg, workers=1)
    stray = data / "sets/s/train/train-000099"
    stray.mkdir()
    again = generate(data, "s", "train", HOURS_5_CLIPS, 1, _other_cfg(cfg), workers=1, force=True)
    assert (again.written, again.skipped) == (5, 0)
    assert not stray.exists()
    manifest = json.loads((data / "sets/s/manifest.json").read_text())
    fp = manifest["splits"]["train"]["fingerprint"]
    for clip in (data / "sets/s/train").iterdir():
        assert json.loads((clip / "meta.json").read_text())["fingerprint"] == fp
    assert is_complete(data / "sets/s/val/val-000000")  # other split untouched


def test_partial_clip_is_regenerated(tiny) -> None:
    data, cfg = tiny
    generate(data, "s", "train", HOURS_5_CLIPS, 1, cfg, workers=1)
    clip = data / "sets/s/train/train-000002"
    (clip / "meta.json").unlink()
    assert not is_complete(clip)
    again = generate(data, "s", "train", HOURS_5_CLIPS, 1, cfg, workers=1)
    assert (again.written, again.skipped) == (1, 4)
    assert is_complete(clip)


def test_val_split_uses_holdouts(tiny) -> None:
    data, cfg = tiny
    generate(data, "s", "val", HOURS_5_CLIPS, 1, cfg, workers=1)
    for clip in (data / "sets/s/val").iterdir():
        meta = json.loads((clip / "meta.json").read_text())
        for e in meta["events"]:
            if e["cls"] == "ambience":
                assert e["map"] == "mirage"
            if e["cls"] == "footsteps":
                assert e["surface"] == "wood"


def test_missing_catalog_names_the_command(tmp_path: Path) -> None:
    from stepwave_datagen.config import load_config

    with pytest.raises(DatagenError, match="datagen catalog"):
        generate(tmp_path, "s", "train", 0.01, 1, load_config(CFG_PATH), workers=1)


def test_mix_command(tiny, tmp_path: Path) -> None:
    data, cfg = tiny
    cfg_file = tmp_path / "cfg.toml"
    text = CFG_PATH.read_text().replace("clip_seconds = 10.0", "clip_seconds = 2.0")
    text = text.replace('holdout_maps = ["overpass"]', 'holdout_maps = ["mirage"]')
    text = text.replace('holdout_surfaces = ["tile"]', 'holdout_surfaces = ["wood"]')
    cfg_file.write_text(text)
    result = CliRunner().invoke(
        app,
        [
            "mix",
            "cli",
            "--hours",
            str(HOURS_5_CLIPS),
            "--workers",
            "1",
            "--data-dir",
            str(data),
            "--config",
            str(cfg_file),
        ],
    )
    assert result.exit_code == 0, result.output
    assert len(list((data / "sets/cli/train").iterdir())) == 5
    args = ["mix", "cli", "--hours", str(HOURS_5_CLIPS), "--workers", "1"]
    args += ["--data-dir", str(data), "--config", str(cfg_file), "--seed", "2"]
    refused = CliRunner().invoke(app, args)
    assert refused.exit_code == 1
    forced = CliRunner().invoke(app, [*args, "--force"])
    assert forced.exit_code == 0, forced.output
    assert "written 5" in forced.output
