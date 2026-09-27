from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import soundfile as sf

from helpers import make_tiny_data
from stepwave_datagen.generate import generate
from stepwave_datagen.stats import compute_stats

CFG_PATH = Path(__file__).resolve().parents[1] / "configs/cs2.toml"
HOURS = 6 * 2.0 / 3600


def build(tmp_path: Path) -> Path:
    data = tmp_path / "data"
    cfg = make_tiny_data(data, CFG_PATH)
    generate(data, "s", "train", HOURS, 1, cfg, workers=1)
    generate(data, "s", "val", HOURS, 1, cfg, workers=1)
    return data / "sets/s"


def test_clean_set_reports_without_problems(tmp_path: Path) -> None:
    set_dir = build(tmp_path)
    result = compute_stats(set_dir, energy_clips=4, listen=2)
    assert result.leaks == [] and result.sum_failures == []
    text = result.report.read_text()
    assert "Clips" in text and "footsteps" in text
    assert (set_dir / "report/snr.png").is_file()
    assert (set_dir / "report/band_energy.png").is_file()
    assert (set_dir / "report/output_rms.png").is_file()
    assert "Measured mix RMS" in text
    listen = sorted((set_dir / "report").glob("listen_*.flac"))
    assert len(listen) == 2
    assert sf.info(listen[0]).samplerate == 48_000


def test_leak_is_detected(tmp_path: Path) -> None:
    set_dir = build(tmp_path)
    meta_path = next((set_dir / "train").iterdir()) / "meta.json"
    meta = json.loads(meta_path.read_text())
    amb = next(e for e in meta["events"] if e["cls"] == "ambience")
    amb["map"] = "mirage"
    meta_path.write_text(json.dumps(meta))
    assert compute_stats(set_dir, energy_clips=1, listen=0).leaks == [meta_path.parent.name]


def test_sum_failure_is_detected(tmp_path: Path) -> None:
    set_dir = build(tmp_path)
    clip = next((set_dir / "val").iterdir())
    data, sr = sf.read(clip / "footsteps.flac")
    sf.write(clip / "footsteps.flac", np.clip(data + 0.1, -1, 1), sr, subtype="PCM_24")
    assert compute_stats(set_dir, energy_clips=1, listen=0).sum_failures == [clip.name]
