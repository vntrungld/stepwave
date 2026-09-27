from __future__ import annotations

from pathlib import Path

import numpy as np
import soundfile as sf

from helpers import CONFIG_PATH, PROFILE, prepped
from stepwave_model.config import load_config
from stepwave_model.evaluate import evaluate, evaluate_real

SR = 48_000


def plus_six(e: dict[str, np.ndarray]) -> np.ndarray:
    return np.full((e["mix"].shape[0], 32), 6.0, np.float32)


def write(path: Path, data: np.ndarray, sr: int = SR) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    sf.write(path, data, sr)


def test_real_recordings_edge_cases(tmp_path: Path) -> None:
    real = tmp_path / "real"
    rng = np.random.default_rng(0)
    write(real / "noise.wav", rng.uniform(-0.1, 0.1, (SR * 2, 2)).astype(np.float32))
    write(real / "silent.flac", np.zeros((SR, 2), np.float32))
    write(real / "short.wav", np.zeros((100, 2), np.float32))
    write(real / "mono.wav", np.zeros(SR, np.float32))
    write(real / "cd.wav", np.zeros((44_100, 2), np.float32), sr=44_100)
    (real / "notes.txt").write_text("ignored")
    logs: list[str] = []
    results = evaluate_real(
        plus_six, real, tmp_path / "out", np.zeros(32, np.float32), 2.0, logs.append
    )
    assert [r["name"] for r in results] == ["noise", "silent"]
    assert results[0]["boost_pct"] == 100.0 and results[0]["seconds"] == 2.0
    for name in ("noise", "silent"):
        for mode in ("off", "eq", "model"):
            y, sr = sf.read(tmp_path / f"out/real/{name}_{mode}.flac")
            assert sr == SR and np.all(np.isfinite(y))
        assert (tmp_path / f"out/real/{name}_gains.png").is_file()
    joined = "\n".join(logs)
    for skipped in ("short.wav", "mono.wav", "cd.wav"):
        assert f"skip {skipped}" in joined


def test_evaluate_includes_real_section(tmp_path: Path) -> None:
    feats = prepped(tmp_path, {"train": 1, "val": 1}, seconds=1.0)
    write(tmp_path / "real/take1.wav", np.random.default_rng(1).uniform(-0.1, 0.1, (SR, 2)))
    cfg = load_config(CONFIG_PATH)
    out = tmp_path / "eval"
    m = evaluate(
        plus_six,
        feats,
        tmp_path / "sets/t",
        out,
        cfg,
        PROFILE,
        real_dir=tmp_path / "real",
        log=lambda _: None,
    )
    assert m["real"][0]["name"] == "take1"
    report = (out / "report.md").read_text()
    assert "## Real recordings" in report and "take1" in report
