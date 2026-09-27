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
