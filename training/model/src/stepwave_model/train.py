"""`trainer train`: fits BandMaskNet on prepped features (any torch device)."""

from __future__ import annotations

import json
import math
import subprocess
import time
from collections.abc import Callable
from pathlib import Path

import numpy as np
import torch

from .config import Config, config_to_dict
from .dataset import Batches
from .errors import TrainerError
from .model import BandMaskNet, false_boost_frames, mask_loss
from .prep import load_manifest, load_split

LOG_HEADER = "epoch,train_loss,val_loss,false_boost_pct,seconds\n"


def pick_device(name: str) -> torch.device:
    if name == "auto":
        return torch.device("cuda" if torch.cuda.is_available() else "cpu")
    if name == "cuda" and not torch.cuda.is_available():
        raise TrainerError(
            "CUDA requested but torch.cuda.is_available() is False: on Windows run `uv sync` "
            "in training/model to get the CUDA build, or pass --device cpu"
        )
    if name not in ("cpu", "cuda"):
        raise TrainerError(f"unknown device {name!r}: use auto, cpu or cuda")
    return torch.device(name)


@torch.no_grad()
def validate(model: BandMaskNet, batches: Batches, cfg: Config) -> tuple[float, float]:
    """(mean loss over full val clips, % of inactive frames with a raw boost > threshold)."""
    model.eval()
    loss_sum, clips, boosted, inactive = 0.0, 0, 0, 0
    for x, target, active in batches.clips():
        pred, _ = model(x)
        loss_sum += float(mask_loss(pred, target, active, cfg.train.false_boost_weight)) * len(x)
        clips += len(x)
        b, n = false_boost_frames(pred, active, cfg.eval.false_boost_db)
        boosted += b
        inactive += n
    model.train()
    return loss_sum / max(clips, 1), 100.0 * boosted / max(inactive, 1)


def _git_commit() -> str | None:
    try:
        out = subprocess.run(
            ["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=True
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    return out.stdout.strip() or None


def train(
    features_dir: Path,
    out_dir: Path,
    cfg: Config,
    device: str = "auto",
    resume: bool = False,
    max_steps: int | None = None,
    log: Callable[[str], None] = print,
) -> Path:
    dev = pick_device(device)
    manifest = load_manifest(features_dir)
    feature = manifest["feature"]
    train_b = Batches(load_split(features_dir, "train"), feature, cfg.target, dev)
    val_b = Batches(load_split(features_dir, "val"), feature, cfg.target, dev)
    tc = cfg.train
    torch.manual_seed(tc.seed)
    model = BandMaskNet(cfg.target.gain_db_range).to(dev)
    opt = torch.optim.AdamW(model.parameters(), lr=tc.lr, weight_decay=tc.weight_decay)
    steps_per_epoch = max(1, train_b.split.frames // (tc.batch_size * tc.seq_frames))
    sched = torch.optim.lr_scheduler.CosineAnnealingLR(opt, T_max=tc.epochs * steps_per_epoch)
    out_dir.mkdir(parents=True, exist_ok=True)
    last, best_path = out_dir / "last.pt", out_dir / "best.pt"
    start_epoch, best, bad = 0, math.inf, 0
    if resume:
        if not last.is_file():
            raise TrainerError(f"--resume: {last} not found")
        ck = torch.load(last, map_location=dev, weights_only=False)
        if ck["features_fingerprint"] != manifest["fingerprint"]:
            raise TrainerError(
                "--resume: features changed since this run started (fingerprint mismatch); "
                "start a new run directory"
            )
        model.load_state_dict(ck["model"])
        opt.load_state_dict(ck["optimizer"])
        sched.load_state_dict(ck["scheduler"])
        start_epoch, best, bad = ck["epoch"] + 1, ck["best_val"], ck["bad_epochs"]
    else:
        info = {
            "config": config_to_dict(cfg),
            "features": manifest,
            "features_dir": str(features_dir),
            "git_commit": _git_commit(),
            "device": str(dev),
            "torch": torch.__version__,
            "steps_per_epoch": steps_per_epoch,
        }
        (out_dir / "train.json").write_text(json.dumps(info, indent=2))
        (out_dir / "log.csv").write_text(LOG_HEADER)

    step = start_epoch * steps_per_epoch
    for epoch in range(start_epoch, tc.epochs):
        rng = np.random.default_rng([tc.seed, epoch])
        t0 = time.perf_counter()
        loss_sum, done = 0.0, 0
        for _ in range(steps_per_epoch):
            if max_steps is not None and step >= max_steps:
                break
            x, target, active = train_b.window(rng, tc.batch_size, tc.seq_frames)
            pred, _ = model(x)
            loss = mask_loss(pred, target, active, tc.false_boost_weight)
            if not torch.isfinite(loss):
                raise TrainerError(
                    f"loss became {loss.item()} at step {step}; last.pt keeps the previous epoch"
                )
            opt.zero_grad(set_to_none=True)
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), tc.grad_clip)
            opt.step()
            sched.step()
            loss_sum += loss.item()
            done += 1
            step += 1
        if done == 0:
            break
        val_loss, fb = validate(model, val_b, cfg)
        seconds = time.perf_counter() - t0
        train_loss = loss_sum / done
        with (out_dir / "log.csv").open("a") as f:
            f.write(f"{epoch + 1},{train_loss:.6f},{val_loss:.6f},{fb:.4f},{seconds:.2f}\n")
        log(
            f"epoch {epoch + 1}/{tc.epochs}: train {train_loss:.4f}  val {val_loss:.4f}  "
            f"false-boost {fb:.2f}%  ({seconds:.1f} s, {dev})"
        )
        improved = val_loss < best
        best, bad = (val_loss, 0) if improved else (best, bad + 1)
        ck = {
            "model": model.state_dict(),
            "optimizer": opt.state_dict(),
            "scheduler": sched.state_dict(),
            "epoch": epoch,
            "best_val": best,
            "bad_epochs": bad,
            "config": config_to_dict(cfg),
            "features_fingerprint": manifest["fingerprint"],
            "feature": feature,
            "val_loss": val_loss,
            "false_boost_pct": fb,
        }
        torch.save(ck, last)
        if improved:
            torch.save(ck, best_path)
        if bad >= tc.early_stop_patience:
            log(f"early stop: no val improvement for {bad} epochs")
            break
        if max_steps is not None and step >= max_steps:
            break
    return best_path
