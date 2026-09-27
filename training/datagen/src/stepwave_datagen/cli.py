"""`datagen` command-line interface. Run from the repo root."""

from __future__ import annotations

from collections.abc import Callable
from importlib.metadata import version as package_version
from pathlib import Path
from typing import TypeVar

import typer

from .errors import DatagenError

DATA_DIR = Path("training/data")
CONFIG = Path("training/datagen/configs/cs2.toml")
RULES = Path("training/datagen/rules/cs2.toml")

app = typer.Typer(no_args_is_help=True, help="stepwave synthetic data generator")

T = TypeVar("T")


@app.callback()
def main() -> None:
    """stepwave synthetic data generator."""
    # Forces Typer to keep a subcommand structure even while only one command
    # (`version`) is registered; without this a single-command Typer app
    # collapses into a bare command instead of `datagen <command>`.


def run(fn: Callable[[], T]) -> T:
    """Run a command body, turning DatagenError into a clean stderr message and exit code 1."""
    try:
        return fn()
    except DatagenError as err:
        typer.echo(f"error: {err}", err=True)
        raise typer.Exit(1) from err


@app.command()
def version() -> None:
    """Print package and feature-binding versions."""
    import stepwave_py

    typer.echo(
        f"stepwave-datagen {package_version('stepwave-datagen')}, "
        f"stepwave_py sample rate {stepwave_py.SAMPLE_RATE}"
    )
