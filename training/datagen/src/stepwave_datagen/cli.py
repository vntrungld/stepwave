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


@app.command()
def extract(
    vpk: Path | None = typer.Option(None, help="pak01_dir.vpk (default: config 'vpk')"),
    cli: Path | None = typer.Option(None, help="Source2Viewer-CLI binary"),
    data_dir: Path = typer.Option(DATA_DIR),
    config: Path = typer.Option(CONFIG),
) -> None:
    """Export CS2 sounds/ from the VPK into <data-dir>/raw."""
    from . import extract as ex
    from .config import load_config

    def body() -> None:
        vpk_path = vpk or Path(load_config(config).vpk).expanduser()
        tool = ex.find_cli(cli, data_dir / "tools")
        ran = ex.extract(vpk_path, data_dir / "raw", tool)
        typer.echo("extracted" if ran else "raw/ is up to date with the VPK; nothing to do")

    run(body)
