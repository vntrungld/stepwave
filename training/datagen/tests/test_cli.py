from __future__ import annotations

from typer.testing import CliRunner

from stepwave_datagen.cli import app


def test_version_command() -> None:
    result = CliRunner().invoke(app, ["version"])
    assert result.exit_code == 0
    assert "48000" in result.output


def test_extract_without_cli_exits_1(tmp_path, monkeypatch) -> None:
    monkeypatch.setenv("PATH", "")
    vpk = tmp_path / "pak01_dir.vpk"
    vpk.write_bytes(b"x")
    result = CliRunner().invoke(app, ["extract", "--vpk", str(vpk), "--data-dir", str(tmp_path)])
    assert result.exit_code == 1
    assert "Source2Viewer-CLI not found" in all_output(result)


def all_output(result) -> str:
    """stdout plus stderr, whichever way this Click version captures them."""
    try:
        return result.output + result.stderr
    except ValueError:
        return result.output
