"""`trainer eval`: off / static EQ / model gains through the real core pipeline (M3 spec,
"Evaluation"). Per-stem energies are measured with the limiter off, so gains are linear."""

from __future__ import annotations

import json
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import numpy as np
import torch
from tqdm import tqdm

from . import HOP, NUM_BANDS, SAMPLE_RATE, STEMS, swm_ref
from .config import Config, TargetConfig
from .errors import TrainerError
from .prep import Split, _binding, load_manifest, load_split, map_jobs, read_stereo
from .swm import SwmModel, read_swm
from .targets import active_frames

MODES = ("off", "eq", "model")
GROUPS = ("active", "inactive")
HIST_EDGES = np.linspace(-12.0, 12.0, 49)
GainFn = Callable[[dict[str, np.ndarray]], np.ndarray]
Job = tuple[Path, dict[str, np.ndarray], GainFn, np.ndarray, TargetConfig, float]


class ModelGains:
    """Picklable GainFn: the exported .swm run through the numpy reference."""

    def __init__(self, swm_path: Path) -> None:
        self.path = swm_path
        self._model: SwmModel | None = None

    def __call__(self, e: dict[str, np.ndarray]) -> np.ndarray:
        if self._model is None:
            self._model = read_swm(self.path)
        return swm_ref.gains_from_energies(self._model, e["mix"])


def render(x: np.ndarray, gains: np.ndarray, limiter: bool = True) -> np.ndarray:
    _, sw = _binding()
    left, right = sw.apply_band_gains(
        x[0], x[1], np.ascontiguousarray(gains, np.float32), limiter=limiter
    )
    return np.stack([left, right])


def _rms(x: np.ndarray) -> float:
    return float(np.sqrt(np.mean(np.asarray(x, np.float64) ** 2)))


def loudness_match(renders: dict[str, np.ndarray]) -> dict[str, np.ndarray]:
    """Scale every render to the lowest RMS among them (fair A/B listening).

    Each render already passed through the limiter at <= -1 dBFS peak, so a quieter
    render (e.g. a static-EQ curve that cuts overall level but barely touches its own
    least-attenuated band) can have a much higher crest factor than a louder one;
    matching *up* to the loudest render's RMS can then push its peak past +-1.0. Only
    ever attenuating avoids that: every render's peak can only shrink. Renders that are
    pure silence (RMS 0) are returned unchanged."""
    rms = {k: _rms(v) for k, v in renders.items()}
    positive = [r for r in rms.values() if r > 0.0]
    if not positive:
        return dict(renders)
    target = min(positive)
    return {
        k: (v if rms[k] == 0.0 else (v * (target / rms[k])).astype(np.float32))
        for k, v in renders.items()
    }


def write_flac(path: Path, x: np.ndarray) -> None:
    sf, _ = _binding()
    path.parent.mkdir(parents=True, exist_ok=True)
    sf.write(path, x.T, SAMPLE_RATE, subtype="PCM_24", format="FLAC")


def _hop_energy(x: np.ndarray, frames: int) -> np.ndarray:
    y = np.asarray(x[:, : frames * HOP], np.float64)
    return (y**2).sum(axis=0).reshape(frames, HOP).sum(axis=1)


def _mode_gains(
    e: dict[str, np.ndarray], gain_fn: GainFn, eq_gains: np.ndarray
) -> dict[str, np.ndarray]:
    frames = e["mix"].shape[0]
    return {
        "off": np.zeros((frames, NUM_BANDS), np.float32),
        "eq": np.tile(np.asarray(eq_gains, np.float32), (frames, 1)),
        "model": np.asarray(gain_fn(e), np.float32),
    }


def eval_clip(job: Job) -> dict[str, np.ndarray]:
    """Accumulators for one clip:
    energy [mode, stem, group, in/out], fb [mode, (boosted inactive, inactive)],
    band_sum [group, 32] and band_n [group] of model gains, hist [48] of model gains."""
    clip_dir, e, gain_fn, eq_gains, tcfg, false_boost_db = job
    _, sw = _binding()
    frames = e["mix"].shape[0]
    t = {s: torch.from_numpy(np.asarray(e[s], np.float32)) for s in ("footsteps", "mix")}
    active = active_frames(
        t["footsteps"], t["mix"], tcfg.active_threshold_db, tcfg.active_dilation_frames
    ).numpy()
    groups = (active, ~active)
    gains = _mode_gains(e, gain_fn, eq_gains)
    stems = {c: read_stereo(clip_dir / f"{c}.flac") for c in STEMS}
    energy = np.zeros((len(MODES), len(STEMS), len(GROUPS), 2))
    fb = np.zeros((len(MODES), 2))
    for m, mode in enumerate(MODES):
        smooth = sw.smooth_gains_db(gains[mode])
        fb[m] = ((smooth.max(axis=1) > false_boost_db) & ~active).sum(), (~active).sum()
        for c, name in enumerate(STEMS):
            e_in = _hop_energy(stems[name], frames)
            e_out = (
                e_in
                if mode == "off"
                else _hop_energy(render(stems[name], gains[mode], limiter=False), frames)
            )
            for g, mask in enumerate(groups):
                energy[m, c, g] = e_in[mask].sum(), e_out[mask].sum()
    model = gains["model"]
    return {
        "energy": energy,
        "fb": fb,
        "band_sum": np.stack([model[mask].sum(axis=0) for mask in groups]),
        "band_n": np.array([mask.sum() for mask in groups], float),
        "hist": np.histogram(model, bins=HIST_EDGES)[0].astype(float),
    }


def _db(num: float, den: float) -> float | None:
    """Power ratio in dB, or None when it is undefined (no energy on either side to compare:
    e.g. a stem that never sounds in this group). None (not NaN) keeps metrics.json strictly
    JSON-safe and round-trippable (`None == None`, unlike `nan == nan`)."""
    if num <= 0 or den <= 0:
        return None
    return float(10 * np.log10(num / den))


def summarize(acc: dict[str, np.ndarray]) -> dict[str, dict[str, float | None]]:
    energy, fb = acc["energy"], acc["fb"]
    fs = STEMS.index("footsteps")
    rest = [i for i in range(len(STEMS)) if i != fs]
    out = {}
    for m, mode in enumerate(MODES):
        a = energy[m]
        snr_in = _db(a[fs, 0, 0], a[rest, 0, 0].sum())
        snr_out = _db(a[fs, 0, 1], a[rest, 0, 1].sum())
        snr_improvement = snr_out - snr_in if snr_in is not None and snr_out is not None else None
        metrics: dict[str, float | None] = {
            "footstep_gain_db": _db(a[fs, 0, 1], a[fs, 0, 0]),
            "snr_improvement_db": snr_improvement,
            "false_boost_pct": 100.0 * fb[m, 0] / max(fb[m, 1], 1.0),
        }
        for c in rest:
            for g, group in enumerate(GROUPS):
                metrics[f"{STEMS[c]}_{group}_db"] = _db(a[c, g, 1], a[c, g, 0])
        out[mode] = metrics
    return out


def _job(
    set_dir: Path, split: Split, i: int, gain_fn: GainFn, eq_gains: np.ndarray, cfg: Config
) -> Job:
    """One clip's job: only that clip's energy slices are copied out of the memmapped split."""
    e = {s: np.array(v) for s, v in split.clip(i).items()}
    return (
        set_dir / "val" / split.clips[i],
        e,
        gain_fn,
        eq_gains,
        cfg.target,
        cfg.eval.false_boost_db,
    )


def _jobs(
    set_dir: Path,
    split: Split,
    gain_fn: GainFn,
    eq_gains: np.ndarray,
    cfg: Config,
    limit: int | None = None,
) -> Iterator[Job]:
    """Lazy, one job (and one clip's worth of memory) at a time; `map_jobs` preserves order."""
    n = len(split.clips) if limit is None else min(limit, len(split.clips))
    for i in range(n):
        yield _job(set_dir, split, i, gain_fn, eq_gains, cfg)


def _check_sources(features_dir: Path, set_dir: Path, clips: list[str]) -> None:
    manifest = load_manifest(features_dir)
    src = set_dir / "manifest.json"
    if not src.is_file():
        raise TrainerError(f"no set at {set_dir}")
    source_fp = json.loads(src.read_text()).get("splits", {}).get("val", {}).get("fingerprint")
    if manifest["splits"]["val"]["source_fingerprint"] != source_fp:
        raise TrainerError(f"{set_dir} val split changed since prep: re-run prep")
    missing = [c for c in clips if not (set_dir / "val" / c / "meta.json").is_file()]
    if missing:
        raise TrainerError(
            f"{len(missing)} val clips from the features are missing in {set_dir / 'val'} "
            f"(first: {missing[0]}): re-run prep"
        )


def _plots(acc: dict[str, np.ndarray], out_dir: Path) -> None:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    _, sw = _binding()
    centres = sw.erb_centres_hz()
    fig, ax = plt.subplots(figsize=(8, 4))
    for g, group in enumerate(GROUPS):
        ax.semilogx(centres, acc["band_sum"][g] / max(acc["band_n"][g], 1.0), label=group)
    ax.set(
        xlabel="band centre (Hz)",
        ylabel="mean model gain (dB)",
        title="Model gain per band (footstep-active vs inactive frames)",
    )
    ax.axhline(0.0, color="grey", lw=0.5)
    ax.legend()
    fig.tight_layout()
    fig.savefig(out_dir / "band_gain.png", dpi=100)
    plt.close(fig)
    fig, ax = plt.subplots(figsize=(8, 4))
    ax.bar(HIST_EDGES[:-1], acc["hist"], width=np.diff(HIST_EDGES), align="edge")
    ax.set(
        xlabel="model gain (dB)",
        ylabel="band-frames",
        yscale="log",
        title="Distribution of model gains",
    )
    fig.tight_layout()
    fig.savefig(out_dir / "gain_hist.png", dpi=100)
    plt.close(fig)


AUDIO_EXT = {".wav", ".flac"}


def _heatmap(gains: np.ndarray, seconds: float, path: Path) -> None:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    fig, ax = plt.subplots(figsize=(10, 3))
    im = ax.imshow(
        gains.T,
        aspect="auto",
        origin="lower",
        cmap="RdBu_r",
        vmin=-12,
        vmax=12,
        extent=(0.0, seconds, 0, NUM_BANDS),
    )
    ax.set(xlabel="time (s)", ylabel="ERB band", title=f"{path.stem}: smoothed model gain (dB)")
    fig.colorbar(im, ax=ax)
    fig.tight_layout()
    fig.savefig(path, dpi=100)
    plt.close(fig)


def evaluate_real(
    gain_fn: GainFn,
    real_dir: Path,
    out_dir: Path,
    eq_gains: np.ndarray,
    false_boost_db: float,
    log: Callable[[str], None] = print,
) -> list[dict[str, Any]]:
    """A/B/C renders and a gain heatmap for each 48 kHz stereo WAV/FLAC in real_dir."""
    sf, sw = _binding()
    if not real_dir.is_dir():
        return []
    results = []
    for path in sorted(p for p in real_dir.iterdir() if p.suffix.lower() in AUDIO_EXT):
        info = sf.info(str(path))
        if info.samplerate != SAMPLE_RATE or info.channels != 2:
            log(
                f"skip {path.name}: {info.samplerate} Hz, {info.channels} ch "
                f"(need {SAMPLE_RATE} Hz stereo)"
            )
            continue
        x = read_stereo(path)
        if x.shape[1] < HOP:
            log(f"skip {path.name}: shorter than one hop ({HOP} samples)")
            continue
        e = {"mix": sw.band_energies_db(x[0], x[1])}
        gains = _mode_gains(e, gain_fn, eq_gains)
        matched = loudness_match({mode: render(x, gains[mode]) for mode in MODES})
        for mode in MODES:
            write_flac(out_dir / "real" / f"{path.stem}_{mode}.flac", matched[mode])
        smooth = sw.smooth_gains_db(gains["model"])
        seconds = x.shape[1] / SAMPLE_RATE
        _heatmap(smooth, seconds, out_dir / "real" / f"{path.stem}_gains.png")
        boost_pct = float(100.0 * np.mean(smooth.max(axis=1) > false_boost_db))
        results.append({"name": path.stem, "seconds": seconds, "boost_pct": boost_pct})
        log(f"real {path.name}: model boosting {boost_pct:.1f}% of the time")
    return results


def _real_section(results: list[dict[str, Any]]) -> str:
    lines = ["## Real recordings", ""]
    if not results:
        return "\n".join(lines + ["No recordings found (see README: recording guide).", ""])
    lines += ["| Recording | seconds | model boosting (% of frames) |", "|---|---|---|"]
    for r in results:
        lines.append(f"| {r['name']} | {r['seconds']:.1f} | {r['boost_pct']:.1f} |")
    lines += ["", "Files: `real/<name>_{off,eq,model}.flac` and `real/<name>_gains.png`.", ""]
    return "\n".join(lines)


ROWS = [
    ("Footstep gain (dB)", "footstep_gain_db", "goal_footstep_gain_db", ">="),
    ("SNR improvement (dB)", "snr_improvement_db", "goal_snr_improvement_db", ">="),
    ("False-boost rate (%)", "false_boost_pct", "goal_false_boost_pct", "<"),
]


def _fmt(value: float | None) -> str:
    return "n/a" if value is None else f"{value:.2f}"


def _report(
    metrics: dict[str, dict[str, float | None]],
    cfg: Config,
    label: str,
    clips: int,
    frames: int,
    active_pct: float,
    listen: int,
) -> str:
    lines = [
        "# stepwave model evaluation",
        "",
        f"Model: `{label}`  ",
        f"Val set: {clips} clips, {frames} frames ({active_pct:.1f}% footstep-active)",
        "",
        "| Metric | off | eq | model | goal |",
        "|---|---|---|---|---|",
    ]
    for title, key, goal_key, op in ROWS:
        goal = getattr(cfg.eval, goal_key)
        value = metrics["model"][key]
        ok = (value >= goal if op == ">=" else value < goal) if value is not None else False
        cells = " | ".join(_fmt(metrics[m][key]) for m in MODES)
        lines.append(f"| {title} | {cells} | {op} {goal:g} {'✅' if ok else '❌'} |")
    lines += ["", "| Class change (dB) | off | eq | model |", "|---|---|---|---|"]
    for stem in STEMS[1:]:
        for group in GROUPS:
            key = f"{stem}_{group}_db"
            cells = " | ".join(_fmt(metrics[m][key]) for m in MODES)
            lines.append(f"| {stem}, {group} | {cells} |")
    lines += [
        "",
        "![band gain](band_gain.png)",
        "",
        "![gain histogram](gain_hist.png)",
        "",
        f"Listening examples: `val_00..{listen - 1:02d}_{{off,eq,model}}.flac` "
        "(limiter on, loudness-matched to the quietest of the three).",
        "",
    ]
    return "\n".join(lines)


def evaluate(
    gain_fn: GainFn,
    features_dir: Path,
    set_dir: Path,
    out_dir: Path,
    cfg: Config,
    profile_path: Path,
    workers: int = 1,
    label: str = "model",
    real_dir: Path | None = None,
    log: Callable[[str], None] = print,
) -> dict[str, Any]:
    _, sw = _binding()
    split = load_split(features_dir, "val")
    _check_sources(features_dir, set_dir, split.clips)
    try:
        eq_gains = sw.static_eq_gains_db(profile_path.read_text())
    except (OSError, ValueError) as err:
        raise TrainerError(f"profile {profile_path}: {err}") from err
    n_clips = len(split.clips)
    acc: dict[str, np.ndarray] = {}
    jobs = _jobs(set_dir, split, gain_fn, eq_gains, cfg)
    for part in tqdm(map_jobs(eval_clip, jobs, workers), total=n_clips, desc="eval"):
        for k, v in part.items():
            acc[k] = acc[k] + v if k in acc else v
    metrics = summarize(acc)
    out_dir.mkdir(parents=True, exist_ok=True)
    listen = min(cfg.eval.listen_examples, n_clips)
    for i, (clip_dir, e, *_rest) in enumerate(
        _jobs(set_dir, split, gain_fn, eq_gains, cfg, listen)
    ):
        mix = read_stereo(clip_dir / "mix.flac")
        gains = _mode_gains(e, gain_fn, eq_gains)
        matched = loudness_match({mode: render(mix, gains[mode]) for mode in MODES})
        for mode in MODES:
            write_flac(out_dir / f"val_{i:02d}_{mode}.flac", matched[mode])
    _plots(acc, out_dir)
    inactive = acc["fb"][0, 1]
    active_pct = 100.0 * (1.0 - inactive / max(split.frames, 1))
    real = (
        evaluate_real(gain_fn, real_dir, out_dir, eq_gains, cfg.eval.false_boost_db, log)
        if real_dir is not None
        else []
    )
    (out_dir / "metrics.json").write_text(
        json.dumps({**metrics, "real": real}, indent=2, sort_keys=True)
    )
    report = _report(metrics, cfg, label, n_clips, split.frames, active_pct, listen)
    (out_dir / "report.md").write_text(report + "\n" + _real_section(real))
    log(f"report: {out_dir / 'report.md'}")
    return {**metrics, "real": real}
