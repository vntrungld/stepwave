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


def test_loudness_match_avoids_clipping(tmp_path: Path) -> None:
    """A loud, bursty input: a sustained low-frequency tone (heavily cut by cs2.json's
    fallback EQ) plus a brief tone near the EQ's least-attenuated band (~2.7 kHz). The EQ
    render's peak survives almost unattenuated while its RMS is dragged down by the cut
    low end, so it has a much higher crest factor than bypass; matching its RMS up to
    bypass's must never push any sample past +-1.0 (evaluate.py:54-59, Important finding
    #2)."""
    from stepwave_model.prep import _binding

    _, sw = _binding()
    real = tmp_path / "real"
    n = SR * 2
    t = np.arange(n) / SR
    low = 0.2 * np.sin(2 * np.pi * 80 * t)
    x = np.stack([low, low], axis=1).astype(np.float64)
    m = int(n * 0.02)
    burst = 0.85 * np.sin(2 * np.pi * 2700 * t[:m])
    x[:m, 0] += burst
    x[:m, 1] += burst
    write(real / "loud.wav", np.clip(x, -0.999, 0.999).astype(np.float32))

    def unity(e: dict[str, np.ndarray]) -> np.ndarray:
        return np.zeros((e["mix"].shape[0], 32), np.float32)

    eq_gains = sw.static_eq_gains_db(PROFILE.read_text()).astype(np.float32)
    evaluate_real(unity, real, tmp_path / "out", eq_gains, 2.0, lambda _: None)
    peak, rms = {}, {}
    for mode in ("off", "eq", "model"):
        y, sr = sf.read(tmp_path / f"out/real/loud_{mode}.flac")
        assert sr == SR
        peak[mode] = float(np.max(np.abs(y)))
        rms[mode] = float(np.sqrt(np.mean(np.asarray(y, np.float64) ** 2)))
    assert all(p < 1.0 for p in peak.values())
    levels_db = [20 * np.log10(r) for r in rms.values() if r > 0]
    assert max(levels_db) - min(levels_db) < 0.1


def test_real_recordings_skip_corrupt_file(tmp_path: Path) -> None:
    """A corrupt real recording must be logged and skipped, not crash the whole real-eval
    pass (Minor finding #5)."""
    real = tmp_path / "real"
    real.mkdir(parents=True)
    (real / "corrupt.wav").write_bytes(b"not actually a wav file" * 10)
    write(real / "noise.wav", np.random.default_rng(0).uniform(-0.1, 0.1, (SR, 2)))
    logs: list[str] = []
    results = evaluate_real(
        plus_six, real, tmp_path / "out", np.zeros(32, np.float32), 2.0, logs.append
    )
    assert [r["name"] for r in results] == ["noise"]
    assert any(line.startswith("skip corrupt.wav:") for line in logs)


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
