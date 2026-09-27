"""`trainer` command-line interface. Run from the repo root."""

from __future__ import annotations

import os
from collections.abc import Callable
from importlib.metadata import version as package_version
from pathlib import Path
from typing import TypeVar

import typer

from .errors import TrainerError

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
