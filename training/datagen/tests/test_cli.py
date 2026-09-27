from __future__ import annotations

from typer.testing import CliRunner

from stepwave_datagen.cli import app


def test_version_command() -> None:
    result = CliRunner().invoke(app, ["version"])
    assert result.exit_code == 0
    assert "48000" in result.output
