"""`trainer` command-line interface. Run from the repo root."""

from __future__ import annotations

import dataclasses
import os
from collections.abc import Callable
from importlib.metadata import version as package_version
from pathlib import Path
from typing import TYPE_CHECKING, Any, TypeVar

import typer

from .errors import TrainerError

if TYPE_CHECKING:
    from .config import TargetConfig

DATA_DIR = Path("training/data")
CONFIG = Path("training/model/configs/cs2.toml")

app = typer.Typer(no_args_is_help=True, help="stepwave model trainer")

T = TypeVar("T")


@app.callback()
def main() -> None:
    """stepwave model trainer."""
    # Keeps Typer in subcommand mode even while only one command is registered.


def run(fn: Callable[[], T]) -> T:
    """Run a command body, turning TrainerError into a clean stderr message and exit code 1."""
    try:
        return fn()
    except TrainerError as err:
        typer.echo(f"error: {err}", err=True)
        raise typer.Exit(1) from err


@app.command()
def version() -> None:
    """Print package and torch versions."""
    import torch

    typer.echo(f"stepwave-model {package_version('stepwave-model')}, torch {torch.__version__}")


@app.command()
def prep(
    set_name: str = typer.Argument(..., help="set name under <data-dir>/sets"),
    data_dir: Path = typer.Option(DATA_DIR),
    out: Path | None = typer.Option(None, help="default <data-dir>/features/<set>"),
    workers: int = typer.Option(os.cpu_count() or 1),
    force: bool = typer.Option(False, "--force", help="rebuild every split"),
) -> None:
    """Band energies of every clip (mix + stems) → <data-dir>/features/<set>."""
    from .prep import prep_set

    path = run(lambda: prep_set(data_dir, set_name, out, workers, force, log=typer.echo))
    typer.echo(f"features: {path}")


@app.command("train")
def train_cmd(
    features: Path = typer.Argument(..., help="features dir from `trainer prep`"),
    out: Path = typer.Option(..., help="run directory (checkpoints, logs)"),
    config: Path = typer.Option(CONFIG),
    device: str = typer.Option("auto", help="auto, cpu or cuda"),
    resume: bool = typer.Option(False, "--resume", help="continue from <out>/last.pt"),
    max_steps: int | None = typer.Option(None, help="stop after this many steps (smoke tests)"),
) -> None:
    """Train BandMaskNet; writes best.pt, last.pt, train.json and log.csv."""
    from .config import load_config
    from .train import train

    def body() -> Path:
        return train(features, out, load_config(config), device, resume, max_steps, typer.echo)

    typer.echo(f"best checkpoint: {run(body)}")


@app.command("export")
def export_cmd(
    run_dir: Path = typer.Argument(..., help="run directory from `trainer train`"),
    out: Path = typer.Option(Path("models/cs2.swm")),
    which: str = typer.Option("best", help="best or last checkpoint"),
) -> None:
    """Write the model as .swm v1 and check it against the numpy reference."""
    from .export import export

    header = run(lambda: export(run_dir, out, which))
    typer.echo(f"{out}: {header['param_count']} params, epoch {header['source']['epoch']}")


def _eval_target(
    header: dict[str, Any],
    fallback: TargetConfig,
    config_path: Path,
    log: Callable[[str], None],
) -> TargetConfig:
    """The target the model was actually trained with, baked into the .swm header by
    `trainer export` (key `target`); falls back to --config's target, with a logged
    warning, for older .swm files exported before this."""
    from .config import target_from_dict

    if "target" in header:
        return target_from_dict(header["target"])
    log(
        f"warning: {config_path} used for the eval target; the model header has no "
        "'target' (exported by an older `trainer export`)"
    )
    return fallback


@app.command("eval")
def eval_cmd(
    swm: Path = typer.Argument(..., help=".swm file from `trainer export`"),
    set_name: str = typer.Option("cs2", "--set", help="set name under <data-dir>/sets"),
    data_dir: Path = typer.Option(DATA_DIR),
    features: Path | None = typer.Option(None, help="default <data-dir>/features/<set>"),
    out: Path | None = typer.Option(None, help="default <data-dir>/eval/<swm stem>"),
    config: Path = typer.Option(CONFIG),
    profile: Path = typer.Option(Path("profiles/cs2.json")),
    workers: int = typer.Option(os.cpu_count() or 1),
    real: Path | None = typer.Option(None, help="real recordings dir (default <data-dir>/real)"),
) -> None:
    """Metrics (off / static EQ / model) on the val set, plus listening files."""
    from .config import load_config
    from .evaluate import ModelGains, evaluate
    from .swm import read_swm

    def body() -> dict:
        model = read_swm(swm)  # fail fast on a bad file
        cfg = load_config(config)
        target = _eval_target(model.header, cfg.target, config, typer.echo)
        cfg = dataclasses.replace(cfg, target=target)
        return evaluate(
            ModelGains(swm),
            features or data_dir / "features" / set_name,
            data_dir / "sets" / set_name,
            out or data_dir / "eval" / swm.stem,
            cfg,
            profile,
            workers,
            label=str(swm),
            real_dir=real or data_dir / "real",
            log=typer.echo,
        )

    m = run(body)["model"]
    typer.echo(
        f"model: footsteps {m['footstep_gain_db']:+.2f} dB, "
        f"SNR {m['snr_improvement_db']:+.2f} dB, false-boost {m['false_boost_pct']:.2f}%"
    )
