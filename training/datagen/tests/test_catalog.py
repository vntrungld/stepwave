from __future__ import annotations

from pathlib import Path

import numpy as np
import soundfile as sf

from stepwave_datagen.catalog import (
    build_catalog,
    load_mono_48k,
    processed_path,
    read_catalog,
    write_catalog,
    write_catalog_report,
)
from stepwave_datagen.rules import load_rules

RULES = """
[[rule]]
pattern = "sounds/player/footsteps/{surface}/**"
class = "footsteps"

[[rule]]
pattern = "sounds/ambient/{map}/**"
class = "ambience"
"""


def write(path: Path, data: np.ndarray, sr: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    sf.write(path, data, sr)


def noise(seconds: float, sr: int, channels: int = 1, amp: float = 0.3) -> np.ndarray:
    rng = np.random.default_rng(0)
    shape = (int(seconds * sr), channels) if channels > 1 else (int(seconds * sr),)
    return (rng.uniform(-amp, amp, shape)).astype(np.float32)


def test_stereo_44k1_is_downmixed_and_resampled(tmp_path: Path) -> None:
    p = tmp_path / "a.wav"
    write(p, noise(0.5, 44_100, channels=2), 44_100)
    x = load_mono_48k(p)
    assert x.ndim == 1 and x.dtype == np.float32
    assert abs(len(x) - 24_000) <= 1


def test_build_catalog_labels_measures_and_drops(tmp_path: Path) -> None:
    raw = tmp_path / "raw"
    write(raw / "sounds/player/footsteps/wood/s1.wav", noise(0.2, 48_000), 48_000)
    write(raw / "sounds/ambient/dust2/bed.wav", noise(1.0, 44_100, channels=2), 44_100)
    write(raw / "sounds/ui/click.wav", noise(0.1, 48_000), 48_000)
    write(raw / "sounds/ui/tiny.wav", noise(0.005, 48_000), 48_000)
    write(raw / "sounds/ui/silent.wav", np.zeros(4800, np.float32), 48_000)
    (raw / "sounds/ui/broken.wav").write_bytes(b"not audio")
    (raw / "sounds/ui/readme.txt").write_text("ignored")
    rules_path = tmp_path / "rules.toml"
    rules_path.write_text(RULES)

    entries, dropped = build_catalog(raw, load_rules(rules_path), workers=1)

    by_path = {e.path: e for e in entries}
    assert set(by_path) == {
        "sounds/ambient/dust2/bed.wav",
        "sounds/player/footsteps/wood/s1.wav",
        "sounds/ui/click.wav",
    }
    step = by_path["sounds/player/footsteps/wood/s1.wav"]
    assert (step.cls, step.surface) == ("footsteps", "wood")
    assert abs(step.duration_s - 0.2) < 1e-3
    assert -15.0 < step.peak_dbfs < -10.0
    assert by_path["sounds/ambient/dust2/bed.wav"].map == "dust2"
    assert by_path["sounds/ui/click.wav"].cls == "other"
    reasons = dict(dropped)
    assert "too short" in reasons["sounds/ui/tiny.wav"]
    assert "silent" in reasons["sounds/ui/silent.wav"]
    assert "undecodable" in reasons["sounds/ui/broken.wav"]
    info = sf.info(processed_path(raw, "sounds/ambient/dust2/bed.wav"))
    assert (info.samplerate, info.channels) == (48_000, 1)


def test_catalog_round_trip_and_report(tmp_path: Path) -> None:
    raw = tmp_path / "raw"
    write(raw / "sounds/player/footsteps/wood/s1.wav", noise(0.2, 48_000), 48_000)
    rules_path = tmp_path / "rules.toml"
    rules_path.write_text(RULES)
    entries, dropped = build_catalog(raw, load_rules(rules_path), workers=1)
    csv_path = tmp_path / "catalog.csv"
    write_catalog(entries, csv_path)
    assert read_catalog(csv_path) == entries
    assert csv_path.read_text().splitlines()[0] == (
        "path,class,surface,map,duration_s,peak_dbfs,rms_dbfs"
    )
    report = tmp_path / "report.md"
    write_catalog_report(entries, dropped, report)
    assert "footsteps" in report.read_text()
