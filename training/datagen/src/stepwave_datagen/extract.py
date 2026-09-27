"""Export CS2 sound assets with Source2Viewer-CLI."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

from .errors import DatagenError

CLI_NAME = "Source2Viewer-CLI"
RELEASES_URL = "https://github.com/ValveResourceFormat/ValveResourceFormat/releases"


def find_cli(explicit: Path | None, tools_dir: Path) -> Path:
    candidates = [explicit] if explicit else [tools_dir / CLI_NAME, shutil.which(CLI_NAME)]
    for candidate in candidates:
        if candidate and Path(candidate).is_file() and os.access(candidate, os.X_OK):
            return Path(candidate)
    raise DatagenError(
        f"{CLI_NAME} not found. Download the Linux CLI build from {RELEASES_URL}, "
        f"unzip it into {tools_dir}/ and make it executable (or pass --cli)."
    )


def build_command(cli: Path, vpk: Path, out_dir: Path) -> list[str]:
    # -d decompiles .vsnd_c to playable audio; --vpk_filepath limits export to sounds/.
    return [str(cli), "-i", str(vpk), "-o", str(out_dir), "-d", "--vpk_filepath", "sounds/"]


def extract(vpk: Path, raw_dir: Path, cli: Path) -> bool:
    if not vpk.is_file():
        raise DatagenError(f"VPK not found: {vpk} (set 'vpk' in the config or pass --vpk)")
    marker = raw_dir / ".extract.json"
    stamp = {"vpk": str(vpk.resolve()), "mtime_ns": vpk.stat().st_mtime_ns}
    if marker.is_file() and json.loads(marker.read_text()) == stamp:
        return False
    raw_dir.mkdir(parents=True, exist_ok=True)
    proc = subprocess.run(build_command(cli, vpk, raw_dir), capture_output=True, text=True)
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout).strip()[-2000:]
        raise DatagenError(f"{CLI_NAME} failed with exit code {proc.returncode}: {detail}")
    marker.write_text(json.dumps(stamp))
    return True
