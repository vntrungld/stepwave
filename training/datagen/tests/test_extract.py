from __future__ import annotations

import json
import os
import stat
from pathlib import Path

import pytest

from stepwave_datagen.errors import DatagenError
from stepwave_datagen.extract import build_command, extract, find_cli

FAKE_CLI = """#!/usr/bin/env python3
import pathlib, sys
args = sys.argv[1:]
out = pathlib.Path(args[args.index("-o") + 1])
(out / "sounds" / "player").mkdir(parents=True, exist_ok=True)
(out / "sounds" / "player" / "step.wav").write_bytes(b"RIFF")
counter = pathlib.Path(__file__).with_suffix(".count")
counter.write_text(str(int(counter.read_text()) + 1 if counter.exists() else 1))
"""


def make_fake_cli(tmp_path: Path, body: str = FAKE_CLI) -> Path:
    cli = tmp_path / "tools" / "Source2Viewer-CLI"
    cli.parent.mkdir(parents=True)
    cli.write_text(body)
    cli.chmod(cli.stat().st_mode | stat.S_IEXEC)
    return cli


def test_find_cli_in_tools_dir(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("PATH", "")
    cli = make_fake_cli(tmp_path)
    assert find_cli(None, tmp_path / "tools") == cli


def test_find_cli_missing_names_the_fix(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("PATH", "")
    with pytest.raises(DatagenError, match="github.com/ValveResourceFormat"):
        find_cli(None, tmp_path / "tools")


def test_command_contains_vpk_and_output(tmp_path: Path) -> None:
    cmd = build_command(Path("/x/cli"), Path("/g/pak01_dir.vpk"), Path("/d/raw"))
    assert cmd[0] == "/x/cli"
    assert "/g/pak01_dir.vpk" in cmd and "/d/raw" in cmd


def test_extract_runs_once_then_skips(tmp_path: Path) -> None:
    cli = make_fake_cli(tmp_path)
    vpk = tmp_path / "pak01_dir.vpk"
    vpk.write_bytes(b"vpk")
    raw = tmp_path / "raw"
    assert extract(vpk, raw, cli) is True
    assert (raw / "sounds/player/step.wav").is_file()
    assert extract(vpk, raw, cli) is False
    assert cli.with_suffix(".count").read_text() == "1"
    os.utime(vpk, ns=(0, 1))  # game update changes mtime
    assert extract(vpk, raw, cli) is True


def test_corrupt_marker_triggers_reextract(tmp_path: Path) -> None:
    cli = make_fake_cli(tmp_path)
    vpk = tmp_path / "pak01_dir.vpk"
    vpk.write_bytes(b"vpk")
    raw = tmp_path / "raw"
    assert extract(vpk, raw, cli) is True
    marker = raw / ".extract.json"
    marker.write_text("{not json")
    assert extract(vpk, raw, cli) is True
    assert cli.with_suffix(".count").read_text() == "2"
    assert json.loads(marker.read_text())


def test_missing_vpk(tmp_path: Path) -> None:
    with pytest.raises(DatagenError, match="VPK not found"):
        extract(tmp_path / "nope.vpk", tmp_path / "raw", make_fake_cli(tmp_path))


def test_cli_failure_is_reported(tmp_path: Path) -> None:
    cli = make_fake_cli(tmp_path, "#!/bin/sh\necho boom >&2\nexit 3\n")
    vpk = tmp_path / "pak01_dir.vpk"
    vpk.write_bytes(b"vpk")
    with pytest.raises(DatagenError, match="boom"):
        extract(vpk, tmp_path / "raw", cli)
