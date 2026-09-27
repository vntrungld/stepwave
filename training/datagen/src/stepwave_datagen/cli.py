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


@app.command()
def catalog(
    data_dir: Path = typer.Option(DATA_DIR),
    rules: Path = typer.Option(RULES),
    workers: int | None = typer.Option(None, help="processes (default: all cores)"),
) -> None:
    """Label, resample and measure every extracted sound; write catalog.csv + report."""
    from . import catalog as cat
    from .rules import load_rules

    def body() -> None:
        raw = data_dir / "raw"
        if not raw.is_dir():
            raise DatagenError(f"{raw} not found; run `datagen extract` first")
        entries, dropped = cat.build_catalog(raw, load_rules(rules), workers)
        cat.write_catalog(entries, data_dir / "catalog.csv")
        cat.write_catalog_report(entries, dropped, data_dir / "catalog-report.md")
        typer.echo(
            f"{len(entries)} files catalogued, {len(dropped)} dropped; see catalog-report.md"
        )

    run(body)


@app.command()
def hrtf(data_dir: Path = typer.Option(DATA_DIR), config: Path = typer.Option(CONFIG)) -> None:
    """Download the configured SOFA HRTF into <data-dir>/hrtf and check it loads."""
    from .config import load_config
    from .hrtf import download, load_sofa

    def body() -> None:
        cfg = load_config(config)
        dest = data_dir / "hrtf" / cfg.hrtf.file
        fetched = download(cfg.hrtf.url, dest)
        h = load_sofa(dest)
        typer.echo(
            f"{'downloaded' if fetched else 'already present'}: {dest} "
            f"({h.ir.shape[0]} directions, {h.ir.shape[2]} taps at 48 kHz)"
        )

    run(body)


@app.command()
def mix(
    name: str = typer.Argument(..., help="dataset name (folder under <data-dir>/sets)"),
    split: str = typer.Option("train", help="train or val"),
    hours: float | None = typer.Option(None, help="default: config split.<split>_hours"),
    seed: int = typer.Option(1),
    workers: int | None = typer.Option(None, help="processes (default: all cores)"),
    data_dir: Path = typer.Option(DATA_DIR),
    config: Path = typer.Option(CONFIG),
) -> None:
    """Generate seeded clips for one split; resumes where a previous run stopped."""
    import os

    from .config import load_config
    from .generate import generate

    def body() -> None:
        cfg = load_config(config)
        h = hours if hours is not None else getattr(cfg.split, f"{split}_hours", None)
        if h is None:
            raise DatagenError(f"unknown split {split!r}; expected 'train' or 'val'")
        result = generate(data_dir, name, split, h, seed, cfg, workers or os.cpu_count() or 1)
        typer.echo(
            f"written {result.written}, skipped {result.skipped}, failed {len(result.failed)}"
        )
        for cid, err in result.failed:
            typer.echo(f"  {cid}: {err}", err=True)
        if result.failed:
            raise typer.Exit(1)

    run(body)
