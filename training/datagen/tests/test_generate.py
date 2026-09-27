from __future__ import annotations

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
    manifest = json.loads((data / "sets/s/manifest.json").read_text())
    assert manifest["splits"]["train"] == {"clips": 5, "hours": HOURS_5_CLIPS, "seed": 1}
    assert len(manifest["catalog_sha256"]) == 64


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
    text = text.replace("holdout_maps = []", 'holdout_maps = ["mirage"]')
    text = text.replace("holdout_surfaces = []", 'holdout_surfaces = ["wood"]')
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
