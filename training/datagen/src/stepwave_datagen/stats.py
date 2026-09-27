"""Dataset report: distributions, per-class ERB energy, leak/stem-sum checks, listening files."""

from __future__ import annotations

import json
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
import soundfile as sf
import stepwave_py
from tqdm import tqdm

from . import CLASSES, SR
from .writer import read_clip

SUM_LIMIT_DB = -80.0


@dataclass
class StatsResult:
    report: Path
    leaks: list[str]
    sum_failures: list[str]


def _clip_dirs(set_dir: Path) -> list[tuple[str, Path]]:
    out = []
    for split in ("train", "val"):
        root = set_dir / split
        if root.is_dir():
            out += [(split, d) for d in sorted(root.iterdir()) if (d / "meta.json").is_file()]
    return out


def _rel_error_db(mix: np.ndarray, stems: dict[str, np.ndarray]) -> float:
    total = np.sum(np.stack([stems[c] for c in CLASSES]), axis=0, dtype=np.float64)
    ref = np.sqrt(np.mean(mix.astype(np.float64) ** 2))
    if ref == 0.0:
        return -np.inf
    err = np.sqrt(np.mean((total - mix) ** 2))
    return float(20 * np.log10(max(err / ref, 1e-15)))


def _mix_rms_dbfs(mix: np.ndarray) -> float | None:
    rms = float(np.sqrt(np.mean(mix.astype(np.float64) ** 2)))
    if rms <= 0.0:
        return None
    return 20 * np.log10(rms)


def _hist(values: list[float], title: str, path: Path, bins: int = 30) -> None:
    fig, ax = plt.subplots(figsize=(6, 3))
    ax.hist(values, bins=bins)
    ax.set_title(title)
    fig.tight_layout()
    fig.savefig(path)
    plt.close(fig)


def compute_stats(
    set_dir: Path, energy_clips: int = 200, listen: int = 5, seed: int = 0
) -> StatsResult:
    plt.switch_backend("Agg")  # headless: files only
    manifest = json.loads((set_dir / "manifest.json").read_text())
    hold_maps = set(manifest["config"]["split"]["holdout_maps"])
    hold_surfaces = set(manifest["config"]["split"]["holdout_surfaces"])
    report_dir = set_dir / "report"
    report_dir.mkdir(exist_ok=True)

    clips = _clip_dirs(set_dir)
    leaks: list[str] = []
    sum_failures: list[str] = []
    snr, snr_final, dist, az, loud, out_rms = [], [], [], [], [], []
    surfaces: Counter[str] = Counter()
    maps: Counter[str] = Counter()
    with_steps = 0
    energy_sum = {c: np.zeros(32) for c in CLASSES}
    energy_n = dict.fromkeys(CLASSES, 0)

    for n, (split, d) in enumerate(tqdm(clips, desc="stats")):
        mix, stems, meta = read_clip(d)
        events = meta["events"]
        if split == "train" and any(
            (e["cls"] == "ambience" and e["map"] in hold_maps)
            or (e["cls"] == "footsteps" and e["surface"] in hold_surfaces)
            for e in events
        ):
            leaks.append(d.name)
        if _rel_error_db(mix, stems) > SUM_LIMIT_DB:
            sum_failures.append(d.name)
        if meta["footstep_snr_db"] is not None:
            with_steps += 1
            snr.append(meta["footstep_snr_db"])
        if meta.get("footstep_snr_db_final") is not None:
            snr_final.append(meta["footstep_snr_db_final"])
        loud.append(meta["loudness_rms_dbfs"])
        measured = _mix_rms_dbfs(mix)
        if measured is not None:
            out_rms.append(measured)
        for e in events:
            if e["spatial"]:
                dist.append(e["distance_m"])
                az.append(e["azimuth_deg"])
            if e["cls"] == "footsteps":
                surfaces[e["surface"]] += 1
            if e["cls"] == "ambience":
                maps[e["map"]] += 1
        if n < energy_clips:
            for c in CLASSES:
                s = stems[c]
                if np.any(s):
                    rows = stepwave_py.band_energies_db(
                        np.ascontiguousarray(s[0]), np.ascontiguousarray(s[1])
                    )
                    energy_sum[c] += (10.0 ** (rows / 10.0)).mean(axis=0)
                    energy_n[c] += 1

    _hist(snr, "footstep SNR (dB, pre-bus)", report_dir / "snr.png")
    _hist(snr_final, "footstep SNR (dB, post-bus, as written)", report_dir / "snr_final.png")
    _hist(dist, "source distance (m)", report_dir / "distance.png")
    _hist(az, "azimuth (deg)", report_dir / "azimuth.png", bins=36)
    _hist(loud, "loudness target (dBFS RMS)", report_dir / "loudness.png")
    _hist(out_rms, "measured mix RMS (dBFS)", report_dir / "output_rms.png")

    centres = stepwave_py.erb_centres_hz()
    fig, ax = plt.subplots(figsize=(7, 4))
    for c in CLASSES:
        if energy_n[c]:
            ax.semilogx(centres, 10 * np.log10(energy_sum[c] / energy_n[c] + 1e-12), label=c)
    ax.set_xlabel("ERB band centre (Hz)")
    ax.set_ylabel("mean band energy (dB)")
    ax.legend()
    fig.tight_layout()
    fig.savefig(report_dir / "band_energy.png")
    plt.close(fig)

    rng = np.random.default_rng(seed)
    train = [d for split, d in clips if split == "train"]
    picks = rng.choice(len(train), size=min(listen, len(train)), replace=False) if train else []
    gap = np.zeros((int(0.5 * SR), 2), np.float32)
    for i, k in enumerate(sorted(int(p) for p in picks)):
        mix, stems, _ = read_clip(train[k])
        audio = np.concatenate([mix.T, gap, stems["footsteps"].T])
        sf.write(report_dir / f"listen_{i:02d}.flac", audio, SR, subtype="PCM_24")

    total = len(clips)
    lines = [
        f"# Dataset report: {set_dir.name}",
        "",
        f"- Clips: {total} ({sum(s == 'train' for s, _ in clips)} train, "
        f"{sum(s == 'val' for s, _ in clips)} val)",
        f"- Hours: {sum(v['hours'] for v in manifest['splits'].values()):.2f}",
        f"- Clips with footsteps: {with_steps} ({with_steps / max(total, 1):.1%})",
        f"- Held out: maps {sorted(hold_maps)}, surfaces {sorted(hold_surfaces)}",
        f"- Leak check: {'FAILED ' + str(leaks) if leaks else 'ok'}",
        f"- Stem-sum check (< {SUM_LIMIT_DB:.0f} dB): "
        f"{'FAILED ' + str(sum_failures) if sum_failures else 'ok'}",
    ]
    if out_rms:
        lines.append(
            f"- Measured mix RMS: min {min(out_rms):.1f} / median "
            f"{float(np.median(out_rms)):.1f} / max {max(out_rms):.1f} dBFS"
        )
    else:
        lines.append("- Measured mix RMS: no non-silent clips")
    lines += [
        "",
        "## Footstep surfaces (events)",
        "",
        "| surface | events |",
        "|---|---|",
        *[f"| {k} | {v} |" for k, v in sorted(surfaces.items())],
        "",
        "## Ambience maps (clips)",
        "",
        "| map | clips |",
        "|---|---|",
        *[f"| {k} | {v} |" for k, v in sorted(maps.items())],
        "",
        "## Plots",
        "",
        "![snr](snr.png) ![snr final](snr_final.png) ![distance](distance.png) "
        "![azimuth](azimuth.png) "
        "![loudness](loudness.png) ![output rms](output_rms.png)",
        "",
        "![band energy per class](band_energy.png)",
        "",
        "## Listening files",
        "",
        "Each file is the mix, 0.5 s of silence, then the footstep stem alone.",
        "",
        *[f"- `{p.name}`" for p in sorted(report_dir.glob("listen_*.flac"))],
    ]
    report = report_dir / "report.md"
    report.write_text("\n".join(lines) + "\n")
    return StatsResult(report, leaks, sum_failures)
