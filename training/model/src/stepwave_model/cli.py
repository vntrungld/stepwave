"""`trainer` command-line interface. Run from the repo root."""

from __future__ import annotations

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
